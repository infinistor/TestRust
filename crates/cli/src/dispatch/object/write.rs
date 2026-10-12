//! 객체 쓰기·삭제·복사: `PutObject`, `PutObjects`, `CopyObject`, `DeleteObject`, `DeleteObjects`,
//! `DeleteObjectTagging`, `RestoreObject`, `StorageMove`.

use std::time::Instant;

use aws_sdk_s3::types::{ChecksumAlgorithm, Tag};
use testrust_common::dotnet_http::status_name;
use testrust_s3::S3Error;
use testrust_s3::ksan::KsanClient;
use testrust_s3::s3_client::PutBody;
use testrust_s3::s3_client::PutObjectRequest;
use tracing::{error, info};

use super::input::{KeyVersionList, parse};
use super::{S3Result, blank, bucket_name, key_name};
use crate::dispatch::output::print_json;
use crate::dispatch::{CommandContext, CommandError, CommandResult};
use crate::usage;
use testrust_scenarios::files::{
    file_exists, file_list, file_md5_base64, read_all_text, string_md5_base64,
};

pub(super) async fn copy_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    let source_key = o.source_key.as_deref().unwrap_or_default();
    let source = if blank(&o.source) {
        info!("원본과 복제본이 같은 버킷으로 복사됩니다.");
        bucket
    } else {
        o.source.as_deref().unwrap_or_default()
    };

    info!("CopyObject Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .copy_object(source, source_key, bucket, key, o.version_id.as_deref())
        .await
        .modeled(&["ObjectNotInActiveTierError"])?;
    let elapsed = started.elapsed().as_millis();

    if response.status == 200 {
        info!("Success! complete time = {elapsed}ms");
    } else {
        error!("Copy failed");
    }
    Ok(0)
}

pub(super) async fn delete_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("DeleteObject Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .delete_object(bucket, key, o.version_id.as_deref(), o.bypass)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 204 {
        info!("{key} Delete Object! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Delete Object failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn delete_objects(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let bucket = bucket_name(ctx);
    // 원본 `key`(`--key`)는 검증하지 않고 로그에 쓴다(없으면 빈 문자열).
    let key = key_name(ctx);
    let text = read_all_text(o.file_path.as_deref().unwrap_or_default())?;
    let key_list = parse::<KeyVersionList>(&text)?;

    info!("DeleteObject Start");
    let objects = key_list
        .iter()
        .flat_map(|list| list.0.iter().flatten())
        .map(|item| item.to_identifier())
        .collect::<Result<Vec<_>, S3Error>>()?;
    let started = Instant::now();
    let response = ctx
        .client()
        .delete_object_identifiers(bucket, objects, o.bypass, Some(o.flag))
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        info!("{key} Delete Object! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Delete Object failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn delete_object_tagging(ctx: &CommandContext) -> CommandResult {
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("DeleteObjectTagging Start");
    let started = Instant::now();
    // 원본은 `--version-id`를 넘기지 않는다.
    let response = ctx
        .client()
        .delete_object_tagging(bucket, key)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 204 {
        info!("{key} Delete Object Tagging! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Delete Object Tagging failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn restore_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("RestoreObject Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .restore_object(bucket, key, o.version_id.as_deref(), o.days)
        .await
        .modeled(&["ObjectAlreadyInActiveTierError"])?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 202 {
        info!("{key} Restore Object Accepted! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Restore Object Accepted failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

/// `tags.Split(',').Select(tag => new Tag { Key = tag.Split('=')[0], Value = tag.Split('=')[1] })`.
/// `=`이 없는 항목은 `IndexOutOfRangeException`.
fn parse_tag_set(tags: &str) -> Result<Vec<Tag>, CommandError> {
    tags.split(',')
        .map(|tag| {
            let mut parts = tag.split('=');
            let key = parts.next().unwrap_or_default();
            let Some(value) = parts.next() else {
                return Err(CommandError::new(
                    "System.IndexOutOfRangeException",
                    "Index was outside the bounds of the array.",
                ));
            };
            Tag::builder()
                .key(key)
                .value(value)
                .build()
                .map_err(|e| CommandError::from(S3Error::Request(e.to_string())))
        })
        .collect()
}

pub(super) async fn put_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let mut request = PutObjectRequest {
        bucket_name: bucket_name(ctx).to_string(),
        storage_class: o.storage_class.clone(),
        ..PutObjectRequest::default()
    };
    if o.file_size != -1 {
        request.content_length = Some(o.file_size);
    }

    if !blank(&o.file_path) {
        let path = o.file_path.as_deref().unwrap_or_default();
        if !file_exists(path) {
            println!("{}", usage::ERROR_FILE);
        } else {
            request.body = Some(PutBody::File(path.into()));
        }

        // md5sum 옵션이 활성화된 경우 또는 lock 모드 업로드시
        if o.md5sum || o.flag {
            let md5 = file_md5_base64(path)?;
            println!("File MD5Digest = {md5}");
            request.content_md5 = Some(md5);
        }
    } else if !blank(&o.body) {
        let body = o.body.clone().unwrap_or_default();
        // md5sum 옵션이 활성화된 경우 또는 lock 모드 업로드시
        if o.md5sum || o.flag {
            let md5 = string_md5_base64(&body);
            println!("Body MD5Digest = {md5}");
            request.content_md5 = Some(md5);
        }
        request.body = Some(PutBody::Text(body));
    }

    // 객체의 이름 설정시
    if !blank(&o.key) {
        request.key = o.key.clone();
    }
    // ACL 추가시
    if !blank(&o.str_acl) {
        request.canned_acl = o.str_acl.clone();
    }
    // sse-c 키 추가시
    if !blank(&o.encryption_key) {
        request.sse_customer_key = o.encryption_key.clone();
    }
    // 태그 추가시
    if !blank(&o.tags) {
        request.tag_set = Some(parse_tag_set(o.tags.as_deref().unwrap_or_default())?);
    }

    request.use_chunk_encoding = o.checksum;
    if o.checksum_type != testrust_s3::ChecksumAlgorithm::None {
        request.checksum_algorithm = Some(ChecksumAlgorithm::from(o.checksum_type.name()));
    }

    info!("PutObject Start");
    let started = Instant::now();
    let response = ctx.client().put_object_request(request).await.plain()?;
    let elapsed = started.elapsed().as_millis();
    let key = key_name(ctx);
    if response.status == 200 {
        if o.print {
            print_json(&response.output);
        }
        info!("{key} PutObject Success! complete time = {elapsed}ms");
    } else {
        error!("{key} PutObject failed({})", status_name(response.status));
    }
    Ok(0)
}

pub(super) async fn put_objects(ctx: &CommandContext) -> CommandResult {
    let bucket = bucket_name(ctx);
    let file_path = ctx.options.file_path.as_deref().unwrap_or_default();
    info!("PutObjects Start");
    let files = file_list(file_path)?;
    info!("Total File Count = {}", files.len());

    let started = Instant::now();
    for file in &files {
        // 원본: `client.PutObject(bucketName, File.Replace(":", "/"), File)`(청크 서명 업로드).
        ctx.client()
            .put_object(
                bucket,
                &file.replace(':', "/"),
                PutBody::File(file.into()),
                true,
                None,
            )
            .await
            .plain()?;
    }
    let elapsed = started.elapsed().as_millis();

    info!(
        "{file_path} Upload {} fils. complete time = {elapsed}ms",
        files.len()
    );
    Ok(0)
}

pub(super) async fn storage_move(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("StorageMove Start");
    let started = Instant::now();
    let ksan = KsanClient::from_user(&ctx.config().main_user, o.debug).map_err(ksan_error)?;
    ksan.storage_move(
        bucket,
        key,
        o.storage_class.as_deref().unwrap_or_default(),
        o.version_id.as_deref(),
    )
    .await
    .map_err(ksan_error)?;
    let elapsed = started.elapsed().as_millis();

    info!("Success! complete time = {elapsed}ms");
    Ok(0)
}

fn ksan_error(error: testrust_s3::ksan::KsanError) -> CommandError {
    CommandError::new(error.dotnet_type(), error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_split_on_comma_and_equals() {
        let tags = parse_tag_set("a=1,b=x=y,c=").unwrap();
        let pairs: Vec<_> = tags.iter().map(|t| (t.key(), t.value())).collect();
        // `Split('=')[1]`은 두 번째 `=` 앞까지다.
        assert_eq!(pairs, [("a", "1"), ("b", "x"), ("c", "")]);
        let error = parse_tag_set("a=1,b").unwrap_err();
        assert_eq!(error.dotnet_type, "System.IndexOutOfRangeException");
    }
}
