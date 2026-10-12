//! `Test/LifecycleTest.cs`: 버킷의 수명주기(Lifecycle) 규칙대로 지워지지 않은 오브젝트가 있는지 검사한다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - `GetBucketVersioning`, `GetLifecycleConfiguration` 순으로 읽고(여기서 난 S3 오류는 호출자로 전파), 활성(`Enabled`) 규칙마다
//!   필터(접두어·태그·And)를 풀어 만료 날짜/일수(`ListObjects`), 삭제 마커(`ListVersions`), 이전 버전(`ListVersions`),
//!   미완료 멀티파트(`ListMultipartUploads`)를 확인한다. 태그 필터는 `GetObjectTagging`으로 비교한다.
//! - 확인 함수(`ExpiredCheck` 등)는 모든 예외를 `ERROR` 로그(`형식: 메시지`)로 남기고 그때까지 센 개수를 돌려준다.
//! - 기간 계산은 `DateTime.UtcNow.AddDays(-n)`이다. 날짜 로그는 ko-KR `DateTime.ToString()` 형식이다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - `rule.Expiration.Date != DateTime.MinValue`에서 `Date`는 `DateTime?`라 값이 없어도 참이다. 그래서 `Expiration`이 있는
//!   모든 규칙은 `Expiration Date : `(비어 있음)을 남기고 날짜 없이(`null`) `ExpiredCheck`를 부른다(목록만 읽고 `LastModified < null`은
//!   거짓이라 개수는 늘지 않는다). `Days` 분기(`else if`)는 실행되지 않는다.
//! - 이전 버전 검사는 현재 버전과 삭제 마커도 센다. 삭제 마커 검사의 `LastVersionDeleteMarkerCheck`는 키를 접두어로 목록을
//!   읽어 같은 접두어의 다른 키까지 센다.
//! - AWSSDK v4는 비어 있는 목록(`S3Objects`, `Versions`, `MultipartUploads`, `Tagging`)을 `null`로 돌려준다. 원본이 `.Count`를
//!   부르는 지점에서 `NullReferenceException`이 나 로그로 남고 그 검사가 끝난다. 태그 필터가 있는데 태그가 없는 오브젝트를 만나도
//!   마찬가지다.
//! - `ConstantClass.Equals`라 `Enabled` 비교는 대소문자를 구분하지 않는다. 한 번도 켠 적 없는 버킷의 버저닝 `Status`는 `Off`다.
//! - 수명주기 설정이 없거나(404 등) 응답 본문이 비어 있어도 `Configuration`은 `null`이 아니라 규칙 없는 설정이다. 그래서
//!   `Lifecycle 설정을 불러오지 못했습니다.` 분기는 실행되지 않고 `Lifecycle 규칙이 비어있습니다.`가 남는다.
//! - `Versions`는 버전 다음에 삭제 마커 순이다(문서 순서 손실).

use aws_sdk_s3::primitives::DateTime as SdkTime;
use aws_sdk_s3::types::{LifecycleExpiration, LifecycleRuleFilter};
use chrono::{Duration, Utc};
use testrust_s3::{S3Client, S3Error};
use tracing::{error, info};

use crate::ScenarioError;
use crate::compare::{ko_kr_sdk_time, version_entries};
use crate::input::null_reference;

/// 규칙 필터의 태그(키, 값).
type TagList = Vec<(String, String)>;

/// `DateTime.UtcNow.AddDays(-days)`.
fn utc_days_ago(days: i32) -> SdkTime {
    let time = Utc::now() - Duration::days(i64::from(days));
    SdkTime::from_secs_and_nanos(time.timestamp(), time.timestamp_subsec_nanos())
}

/// `DateTime? < DateTime?`: 한쪽이 `null`이면 거짓.
fn is_before(time: Option<&SdkTime>, expired: Option<&SdkTime>) -> bool {
    matches!((time, expired), (Some(t), Some(e)) if t < e)
}

/// `ConstantClass.Equals(string)`: 대소문자를 구분하지 않는다.
fn is_enabled(value: &str) -> bool {
    value.eq_ignore_ascii_case("Enabled")
}

/// 원본 `LifecycleTest`.
pub struct LifecycleTest {
    client: S3Client,
}

impl LifecycleTest {
    pub fn new(client: S3Client) -> Self {
        Self { client }
    }

    /// 원본 `Start(bucketName)`.
    pub async fn start(&self, bucket_name: &str) -> Result<(), ScenarioError> {
        info!("LifecycleTest Start");
        // 버킷의 Versioning 설정을 가져온다.
        let versioning = self
            .client
            .get_bucket_versioning(bucket_name)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        // 한 번도 켠 적 없는 버킷은 `Status`가 없고 .NET SDK는 `Off`로 채운다.
        let is_versioning = versioning
            .output
            .status()
            .is_some_and(|status| is_enabled(status.as_str()));

        // 버킷의 Lifecycle 룰을 가져온다.
        // .NET은 2xx와 404(`NoSuchLifecycleConfiguration` 등)를 응답으로 돌려주고, 이때도 `Configuration`은 `null`이 아니라 규칙이
        // 없는 설정이다(그래서 `Lifecycle 설정을 불러오지 못했습니다.` 분기는 실행되지 않는다).
        let lifecycle = match self.client.get_lifecycle_configuration(bucket_name).await {
            Ok(response) => Some(response.output),
            Err(S3Error::Service { status, .. })
                if (200..300).contains(&status) || status == 404 =>
            {
                None
            }
            Err(e) => return Err(ScenarioError::s3(e, &[])),
        };

        // 버킷의 수명주기 규칙이 비어있거나 재대로 불러오지 못할 경우 스킵
        let rules = lifecycle.as_ref().map(|l| l.rules()).unwrap_or_default();
        if rules.is_empty() {
            error!("Lifecycle 규칙이 비어있습니다.");
            return Ok(());
        }

        // 룰정보 가져오기
        let mut count = 0usize;

        // 수명주기 규칙을 확인한다.
        for rule in rules {
            // 버킷의 수명주기 설정이 활성화 되어있지 않을 경우 스킵
            if !is_enabled(rule.status().as_str()) {
                continue;
            }

            // 수명주기 규칙의 Filter 정보를 가져온다.
            let (prefix, tags) = rule_filter(rule.filter());
            let prefix = prefix.as_deref();
            let tags = tags.as_deref();

            // Current 버전의 수명주기 설정이 되어있을 경우
            if let Some(expiration) = rule.expiration() {
                count += self
                    .check_expiration(bucket_name, expiration, is_versioning, prefix, tags)
                    .await;
            }

            // Noncurrent 버전의 수명주기 설정이 되어있을 경우
            if let Some(noncurrent) = rule.noncurrent_version_expiration()
                && is_versioning
                && noncurrent.noncurrent_days().is_some_and(|d| d > 0)
            {
                let days = noncurrent.noncurrent_days().unwrap_or(0);
                info!("NoncurrentVersionExpiration Days : {days}");
                let expired_date = utc_days_ago(days);
                count += self
                    .expired_noncurrent_check(bucket_name, &expired_date, prefix, tags)
                    .await;
            }

            // MultiPart의 수명주기 설정이 되어있을 경우
            if let Some(abort) = rule.abort_incomplete_multipart_upload()
                && abort.days_after_initiation().is_some_and(|d| d > 0)
            {
                let days = abort.days_after_initiation().unwrap_or(0);
                info!("AbortIncompleteMultipartUpload Days : {days}");
                let expired_date = utc_days_ago(days);
                count += self
                    .expired_multipart_upload_check(bucket_name, &expired_date, prefix, tags)
                    .await;
            }
        }

        if count > 0 {
            error!("Expired {count} Objects.");
        } else {
            info!("All Objects are Valid.");
        }
        Ok(())
    }

    /// `rule.Expiration`이 있을 때의 확인(날짜 또는 일수, 삭제 마커).
    async fn check_expiration(
        &self,
        bucket_name: &str,
        expiration: &LifecycleExpiration,
        is_versioning: bool,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
    ) -> usize {
        let mut count = 0;
        // `Date`는 `DateTime?`라 `Date != DateTime.MinValue`는 값이 없어도 참이다(`Days` 분기는 도달하지 못한다).
        info!("Expiration Date : {}", ko_kr_sdk_time(expiration.date()));
        count += self
            .expired_check(bucket_name, expiration.date(), prefix, tags)
            .await;

        // DeleteMarker가 설정되어있을 경우
        if expiration.expired_object_delete_marker().unwrap_or(false) && is_versioning {
            info!(
                "DeleteMarker : {}",
                if expiration.expired_object_delete_marker().unwrap_or(false) {
                    "True"
                } else {
                    "False"
                }
            );
            count += self.expired_delete_marker_check(bucket_name).await;
        }
        count
    }

    /// 원본 `ExpiredCheck`: Current의 만료 개수.
    async fn expired_check(
        &self,
        bucket_name: &str,
        expired_date: Option<&SdkTime>,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
    ) -> usize {
        let mut count = 0;
        if let Err(e) = self
            .expired_check_inner(bucket_name, expired_date, prefix, tags, &mut count)
            .await
        {
            error!("{e}");
        }
        count
    }

    async fn expired_check_inner(
        &self,
        bucket_name: &str,
        expired_date: Option<&SdkTime>,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
        count: &mut usize,
    ) -> Result<(), ScenarioError> {
        let mut next_marker = String::new();
        loop {
            // 버킷의 오브젝트 목록을 가져온다.
            let resource = self
                .client
                .list_objects(bucket_name, prefix, Some(&next_marker), 1000, None)
                .await
                .map_err(|e| ScenarioError::s3(e, &["NoSuchBucket"]))?;
            let objects = resource.output.contents();
            // `Resource.S3Objects`가 `null`
            if objects.is_empty() {
                return Err(null_reference());
            }

            // 오브젝트의 만료일을 확인한다.
            for object in objects {
                // 태그가 설정되어있을 경우 태그를 확인한다.
                // 태그가 일치하지 않을 경우 다음 오브젝트로 이동
                if let Some(tags) = tags
                    && !tags.is_empty()
                    && !self
                        .tag_check(bucket_name, object.key().unwrap_or_default(), tags)
                        .await?
                {
                    continue;
                }

                // 만료일이 지난 오브젝트일 경우 카운트 증가
                if is_before(object.last_modified(), expired_date) {
                    info!(
                        "{} is Not Deleted. LastModified : {} / ExpiredDate : {}",
                        object.key().unwrap_or_default(),
                        ko_kr_sdk_time(object.last_modified()),
                        ko_kr_sdk_time(expired_date)
                    );
                    *count += 1;
                }
            }

            // 다음 페이지가 있을 경우 다음 페이지로 이동
            if resource.output.is_truncated() == Some(true) {
                // .NET SDK는 `NextMarker`가 없으면 마지막 키를 쓴다.
                next_marker = resource
                    .output
                    .next_marker()
                    .or_else(|| objects.last().and_then(|o| o.key()))
                    .unwrap_or_default()
                    .to_string();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// 원본 `ExpiredNoncurrentCheck`: Noncurrent의 만료 개수.
    async fn expired_noncurrent_check(
        &self,
        bucket_name: &str,
        expired_date: &SdkTime,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
    ) -> usize {
        let mut count = 0;
        if let Err(e) = self
            .expired_noncurrent_inner(bucket_name, expired_date, prefix, tags, &mut count)
            .await
        {
            error!("{e}");
        }
        count
    }

    async fn expired_noncurrent_inner(
        &self,
        bucket_name: &str,
        expired_date: &SdkTime,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
        count: &mut usize,
    ) -> Result<(), ScenarioError> {
        let mut next_key_marker = String::new();
        let mut next_version_id_marker = String::new();
        loop {
            // 버킷의 오브젝트 목록을 가져온다.
            let response = self
                .client
                .list_versions(
                    bucket_name,
                    prefix,
                    Some(&next_key_marker),
                    Some(&next_version_id_marker),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            // `response.Versions`가 `null`
            let versions = version_entries(&response.output).ok_or_else(null_reference)?;

            // 오브젝트의 만료일을 확인한다.
            for key in &versions {
                let key_name = key.key.as_deref().unwrap_or_default();
                // 태그가 설정되어있을 경우 태그를 확인한다.
                // 태그가 일치하지 않을 경우 다음 오브젝트로 이동
                if let Some(tags) = tags
                    && !tags.is_empty()
                    && !self.tag_check(bucket_name, key_name, tags).await?
                {
                    continue;
                }

                // 만료일이 지난 오브젝트일 경우 실패
                if is_before(key.last_modified.as_ref(), Some(expired_date)) {
                    info!(
                        "{key_name}({}) is Not Deleted. LastModified : {} / expiredDate : {}",
                        key.version_id.as_deref().unwrap_or_default(),
                        ko_kr_sdk_time(key.last_modified.as_ref()),
                        ko_kr_sdk_time(Some(expired_date))
                    );
                    *count += 1;
                }
            }

            // 다음 페이지가 있을 경우 다음 페이지로 이동
            if response.output.is_truncated() == Some(true) {
                next_key_marker = response
                    .output
                    .next_key_marker()
                    .unwrap_or_default()
                    .to_string();
                next_version_id_marker = response
                    .output
                    .next_version_id_marker()
                    .unwrap_or_default()
                    .to_string();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// 원본 `ExpiredDeleteMarkerCheck`: 삭제 마커만 남은 키의 개수.
    async fn expired_delete_marker_check(&self, bucket_name: &str) -> usize {
        let mut count = 0;
        if let Err(e) = self
            .expired_delete_marker_inner(bucket_name, &mut count)
            .await
        {
            error!("{e}");
        }
        count
    }

    async fn expired_delete_marker_inner(
        &self,
        bucket_name: &str,
        count: &mut usize,
    ) -> Result<(), ScenarioError> {
        let mut next_key_marker = String::new();
        let mut next_version_id_marker = String::new();
        loop {
            // 버킷의 오브젝트 목록을 가져온다.
            let response = self
                .client
                .list_versions(
                    bucket_name,
                    None,
                    Some(&next_key_marker),
                    Some(&next_version_id_marker),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            // `response.Versions`가 `null`
            let versions = version_entries(&response.output).ok_or_else(null_reference)?;

            // 오브젝트의 만료일을 확인한다.
            for key in &versions {
                // DeleteMarker이고 마지막 버전일 경우 카운트 증가
                if key.is_delete_marker
                    && self
                        .last_version_delete_marker_check(
                            bucket_name,
                            key.key.as_deref().unwrap_or_default(),
                        )
                        .await
                {
                    *count += 1;
                }
            }

            // 다음 페이지가 있을 경우 다음 페이지로 이동
            if response.output.is_truncated() == Some(true) {
                next_key_marker = response
                    .output
                    .next_key_marker()
                    .unwrap_or_default()
                    .to_string();
                next_version_id_marker = response
                    .output
                    .next_version_id_marker()
                    .unwrap_or_default()
                    .to_string();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// 원본 `ExpiredMultipartUploadCheck`: 미완료 멀티파트의 만료 개수.
    async fn expired_multipart_upload_check(
        &self,
        bucket_name: &str,
        expired_date: &SdkTime,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
    ) -> usize {
        let mut count = 0;
        if let Err(e) = self
            .expired_multipart_inner(bucket_name, expired_date, prefix, tags, &mut count)
            .await
        {
            error!("{e}");
        }
        count
    }

    async fn expired_multipart_inner(
        &self,
        bucket_name: &str,
        expired_date: &SdkTime,
        prefix: Option<&str>,
        tags: Option<&[(String, String)]>,
        count: &mut usize,
    ) -> Result<(), ScenarioError> {
        let mut next_key_marker = String::new();
        let mut next_upload_id_marker = String::new();
        loop {
            // 버킷의 오브젝트 목록을 가져온다.
            let response = self
                .client
                .list_multipart_uploads(
                    bucket_name,
                    prefix,
                    Some(&next_upload_id_marker),
                    Some(&next_key_marker),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            let uploads = response.output.uploads();
            // `response.MultipartUploads`가 `null`
            if uploads.is_empty() {
                return Err(null_reference());
            }

            // 오브젝트의 만료일을 확인한다.
            for upload in uploads {
                let key_name = upload.key().unwrap_or_default();
                // 태그가 설정되어있을 경우 태그를 확인한다.
                // 태그가 일치하지 않을 경우 다음 오브젝트로 이동
                if let Some(tags) = tags
                    && !tags.is_empty()
                    && !self.tag_check(bucket_name, key_name, tags).await?
                {
                    continue;
                }
                // 만료일이 지난 오브젝트일 경우 실패
                if is_before(upload.initiated(), Some(expired_date)) {
                    error!(
                        "MultipartUpload({key_name}) is Not Deleted. Initiated : {} + expiredDate : {}",
                        ko_kr_sdk_time(upload.initiated()),
                        ko_kr_sdk_time(Some(expired_date))
                    );
                    *count += 1;
                }
            }

            // 다음 페이지가 있을 경우 다음 페이지로 이동
            if response.output.is_truncated() == Some(true) {
                next_key_marker = response
                    .output
                    .next_key_marker()
                    .unwrap_or_default()
                    .to_string();
                next_upload_id_marker = response
                    .output
                    .next_upload_id_marker()
                    .unwrap_or_default()
                    .to_string();
            } else {
                break;
            }
        }
        Ok(())
    }

    /// 원본 `TagCheck`: 오브젝트의 태그가 규칙의 태그와 같은지. 태그가 하나도 없으면(`Tagging`이 `null`) `NullReferenceException`.
    async fn tag_check(
        &self,
        bucket_name: &str,
        key: &str,
        tags: &[(String, String)],
    ) -> Result<bool, ScenarioError> {
        // 오브젝트의 태그를 가져온다.
        let key_tags = self
            .client
            .get_object_tagging(bucket_name, key, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        let tag_set = key_tags.output.tag_set();
        // 태그가 설정되어있지 않을 경우(`Tagging`이 `null`이라 `.Count`에서 예외)
        if tag_set.is_empty() {
            return Err(null_reference());
        }

        // 태그가 설정되어있을 경우 태그를 확인한다.
        let mut check_count = 0;
        for tag in tag_set {
            // 태그가 일치하지 않을 경우
            if !tags.iter().any(|(k, v)| k == tag.key() && v == tag.value()) {
                return Ok(false);
            }
            check_count += 1;
        }

        // 태그가 일치하지 않을 경우
        Ok(check_count == tags.len())
    }

    /// 원본 `LastVersionDeleteMarkerCheck`: 키를 접두어로 읽은 목록이 삭제 마커 하나뿐이면 참. 예외는 로그만 남기고 거짓.
    async fn last_version_delete_marker_check(&self, bucket_name: &str, key: &str) -> bool {
        let result: Result<bool, ScenarioError> = async {
            let response = self
                .client
                .list_versions(bucket_name, Some(key), None, None, 1000, None)
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            // `response.Versions`가 `null`
            let versions = version_entries(&response.output).ok_or_else(null_reference)?;
            Ok(versions.len() == 1 && versions[0].is_delete_marker)
        }
        .await;
        match result {
            Ok(value) => value,
            Err(e) => {
                error!("{e}");
                false
            }
        }
    }
}

/// 원본 `GetRule2Filter`: 규칙 필터의 접두어와 태그.
fn rule_filter(filter: Option<&LifecycleRuleFilter>) -> (Option<String>, Option<TagList>) {
    let mut prefix = None;
    let mut tags: Option<TagList> = None;
    let Some(filter) = filter else {
        return (prefix, tags);
    };

    // Prefix가 설정되어있을 경우
    if let Some(p) = filter.prefix() {
        prefix = Some(p.to_string());
    }
    // Tag가 설정되어있을 경우
    else if let Some(tag) = filter.tag() {
        tags = Some(vec![(tag.key().to_string(), tag.value().to_string())]);
    }
    // And가 설정되어있을 경우
    else if let Some(and) = filter.and() {
        // Prefix가 설정되어있을 경우
        if let Some(p) = and.prefix() {
            prefix = Some(p.to_string());
        }
        // Tag가 설정되어있을 경우
        if !and.tags().is_empty() {
            tags = Some(
                and.tags()
                    .iter()
                    .map(|t| (t.key().to_string(), t.value().to_string()))
                    .collect(),
            );
        }
    }
    (prefix, tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_ignores_case() {
        assert!(is_enabled("Enabled"));
        assert!(is_enabled("enabled"));
        assert!(!is_enabled("Disabled"));
    }

    #[test]
    fn null_dates_never_expire() {
        let a = SdkTime::from_secs(10);
        let b = SdkTime::from_secs(20);
        assert!(is_before(Some(&a), Some(&b)));
        assert!(!is_before(Some(&b), Some(&a)));
        assert!(!is_before(Some(&a), None));
        assert!(!is_before(None, Some(&b)));
    }

    #[test]
    fn filter_parts() {
        use aws_sdk_s3::types::{LifecycleRuleAndOperator, Tag};
        let tag = Tag::builder().key("k").value("v").build().unwrap();
        let and = LifecycleRuleAndOperator::builder()
            .prefix("p/")
            .tags(tag.clone())
            .build();
        let filter = LifecycleRuleFilter::builder().and(and).build();
        let (prefix, tags) = rule_filter(Some(&filter));
        assert_eq!(prefix.as_deref(), Some("p/"));
        assert_eq!(tags.unwrap(), [("k".to_string(), "v".to_string())]);
        let filter = LifecycleRuleFilter::builder().tag(tag).build();
        let (prefix, tags) = rule_filter(Some(&filter));
        assert!(prefix.is_none());
        assert_eq!(tags.unwrap().len(), 1);
        assert_eq!(rule_filter(None), (None, None));
    }
}
