//! 객체 설정 쓰기: `PutObjectLegalHold`, `PutObjectLock`, `PutObjectRetention`, `PutObjectTagging`.

use std::time::Instant;

use aws_sdk_s3::primitives::DateTime as SdkDateTime;
use aws_sdk_s3::types::{
    DefaultRetention, ObjectLockConfiguration, ObjectLockEnabled, ObjectLockRetention,
    ObjectLockRetentionMode, ObjectLockRule,
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use testrust_common::dotnet_http::status_name;
use tracing::{error, info};

use super::input::{LegalHold, TaggingInput, parse};
use super::{S3Result, bucket_name, key_name};
use crate::dispatch::{CommandContext, CommandResult};
use testrust_scenarios::files::read_all_text;

pub(super) async fn put_object_legal_hold(ctx: &CommandContext) -> CommandResult {
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    let text = read_all_text(ctx.options.file_path.as_deref().unwrap_or_default())?;
    // JSON `null`이면 원본은 본문 없이 보낸다. SDK 형식은 본문이 필수라 빈 `LegalHold`로 보낸다.
    let setting = parse::<LegalHold>(&text)?.unwrap_or_default();

    info!("PutObjectLegalHold Start");
    let hold = setting.to_sdk()?;
    let started = Instant::now();
    // 원본은 `--version-id`를 넘기지 않는다.
    let response = ctx
        .client()
        .put_object_legal_hold(bucket, key, hold, None)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        info!("{key} Put Object LegalHold! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Put Object LegalHold failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn put_object_lock(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let bucket = bucket_name(ctx);
    // 원본 `key`(`--key`)는 검증하지 않고 로그에 쓴다(없으면 빈 문자열).
    let key = key_name(ctx);
    // 둘 다 입력되지 않으면 안됨
    if o.years == -1 && o.days == -1 {
        println!("year, days 중 하나 이상 입력해야 합니다.");
    }
    // 둘 다 입력되면 안됨(안내만 하고 그대로 보낸다)
    if o.years != -1 && o.days != -1 {
        println!("year, days 중 하나만 입력해야 합니다.");
    }

    let mode = if o.lock_mode.as_deref() == Some("Compliance") {
        ObjectLockRetentionMode::Compliance
    } else {
        ObjectLockRetentionMode::Governance
    };
    let retention = DefaultRetention::builder()
        .mode(mode)
        .set_days((o.days != -1).then_some(o.days))
        .set_years((o.years != -1).then_some(o.years))
        .build();
    let setting = ObjectLockConfiguration::builder()
        .object_lock_enabled(ObjectLockEnabled::Enabled)
        .rule(
            ObjectLockRule::builder()
                .default_retention(retention)
                .build(),
        )
        .build();

    info!("PutObjectLockConfiguration Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .put_object_lock_configuration(bucket, setting)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        info!("{key} Put Object Lock! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Put Object Lock failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

/// `DateTime.TryParse(text, out var dateTime)`의 부분집합. 오프셋(`Z`, `+09:00`)이 있으면 그 시각, 없으면
/// 이 PC의 현지 시각으로 본다. 지원하는 형식은 `yyyy-MM-dd`, `yyyy/MM/dd`, `yyyy.MM.dd` 뒤에 선택적으로
/// ` HH:mm[:ss[.fff]]`(구분자 `T` 가능)과 오프셋이 오는 모양이다(원본은 문화권별 형식을 더 받는다).
fn try_parse_date(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&Utc));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f %#z",
        "%Y-%m-%dT%H:%M%#z",
        "%Y-%m-%d %H:%M%#z",
        "%Y-%m-%d %H:%M %#z",
        "%Y/%m/%d %H:%M:%S%.f%#z",
        "%Y/%m/%d %H:%M:%S%.f %#z",
    ] {
        if let Ok(time) = DateTime::parse_from_str(text, format) {
            return Some(time.with_timezone(&Utc));
        }
    }
    let normalized = text.replace(['/', '.'], "-");
    let local = |naive: NaiveDateTime| {
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
    };
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, format) {
            return local(naive);
        }
    }
    let date = NaiveDate::parse_from_str(&normalized, "%Y-%m-%d").ok()?;
    local(date.and_hms_opt(0, 0, 0)?)
}

pub(super) async fn put_object_retention(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    let Some(date) = try_parse_date(o.date.as_deref().unwrap_or_default()) else {
        println!("--date 보관 만료 날짜를 입력해야 합니다.");
        return Ok(-1);
    };

    let mode = if o.lock_mode.as_deref() == Some("Governance") {
        ObjectLockRetentionMode::Governance
    } else {
        ObjectLockRetentionMode::Compliance
    };
    let setting = ObjectLockRetention::builder()
        .mode(mode)
        .retain_until_date(SdkDateTime::from_secs_and_nanos(
            date.timestamp(),
            date.timestamp_subsec_nanos(),
        ))
        .build();

    info!("PutObjectRetention Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .put_object_retention(
            bucket,
            key,
            setting,
            None,
            o.version_id.as_deref(),
            o.bypass,
        )
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        info!("{key} Put Object Retention! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Put Object Retention failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn put_object_tagging(ctx: &CommandContext) -> CommandResult {
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    let text = read_all_text(ctx.options.file_path.as_deref().unwrap_or_default())?;
    // JSON `null`이면 원본은 본문 없이 보낸다. SDK 형식은 본문이 필수라 빈 `Tagging`으로 보낸다.
    let setting = parse::<TaggingInput>(&text)?.unwrap_or_default();

    info!("PutObjectTagging Start");
    let tagging = setting.to_sdk()?;
    let started = Instant::now();
    let response = ctx
        .client()
        .put_object_tagging(bucket, key, tagging)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        info!("{key} Put Object Tagging! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Put Object Tagging failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_with_offsets_are_exact() {
        let expected = Utc.with_ymd_and_hms(2030, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(try_parse_date("2030-01-02T03:04:05Z"), Some(expected));
        assert_eq!(
            try_parse_date(" 2030-01-02T12:04:05+09:00 "),
            Some(expected)
        );
        assert_eq!(try_parse_date("2030-01-02 12:04:05 +09:00"), Some(expected));
    }

    #[test]
    fn dates_without_zone_are_local() {
        let expected = Local
            .with_ymd_and_hms(2030, 1, 2, 0, 0, 0)
            .earliest()
            .map(|t| t.with_timezone(&Utc));
        assert_eq!(try_parse_date("2030-01-02"), expected);
        assert_eq!(try_parse_date("2030/01/02"), expected);
    }

    #[test]
    fn invalid_dates_are_rejected() {
        assert_eq!(try_parse_date("not-a-date"), None);
        assert_eq!(try_parse_date(""), None);
        assert_eq!(try_parse_date("2030-13-40"), None);
    }
}
