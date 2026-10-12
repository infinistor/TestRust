//! 객체·객체 설정 조회: `GetObject`, `HeadObject`, `GetObjectLegalHold`, `GetObjectLock`, `GetObjectRetention`,
//! `GetObjectTagging`, `GetPresignedUrl`.

use std::time::Instant;

use aws_sdk_s3::operation::get_object::GetObjectOutput;
use aws_sdk_s3::operation::head_object::HeadObjectOutput;
use aws_sdk_s3::types::ChecksumMode;
use chrono::{Duration, Utc};
use testrust_common::dotnet_http::status_name;
use testrust_s3::checksum::{ChecksumAlgorithm, ChecksumError, calculate_checksum};
use testrust_s3::s3_client::HttpVerb;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

use super::{S3Result, bucket_name, key_name};
use crate::dispatch::output::print_json;
use crate::dispatch::{CommandContext, CommandError, CommandResult};
use testrust_scenarios::files::{full_path, io_error, read_error, save_file};

pub(super) async fn get_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("GetObject Start");
    // 원본은 `startByte >= 0 || endByte > 0`일 때만 범위를 건다(시작이 음수면 0).
    let mut start_byte = o.start_byte;
    let range = if start_byte >= 0 || o.end_byte > 0 {
        if start_byte < 0 {
            start_byte = 0;
        }
        Some((start_byte, o.end_byte))
    } else {
        None
    };

    let started = Instant::now();
    let response = ctx
        .client()
        .get_object_with_key(
            bucket,
            key,
            o.version_id.as_deref(),
            range,
            o.encryption_key.as_deref(),
        )
        .await
        .modeled(&["NoSuchKey", "InvalidObjectState"])?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 || response.status == 206 {
        let output = response.output;
        let file_path = o.file_path.as_deref();
        if o.checksum {
            verify_checksums(file_path, output).await?;
        } else if let Some(path) = file_path {
            if o.file_size == -1 {
                save_file(Some(path), output.body).await;
            } else {
                save_limited(path, o.file_size, output).await?;
            }
        } else {
            // 원본 `Utility.GetBodySplit`: 본문을 메모리로 모두 읽어 버린다.
            output.body.collect().await.map_err(read_error)?;
        }
        info!("{key} Get Object! complete time = {elapsed}ms");
    } else {
        error!("{key} Get Object failed({})", status_name(response.status));
    }
    Ok(0)
}

/// `--checksum`: 파일로 저장한 뒤 응답 헤더의 체크섬과 파일에서 계산한 값을 비교한다.
async fn verify_checksums(
    file_path: Option<&str>,
    output: GetObjectOutput,
) -> Result<(), CommandError> {
    let checks: [(&str, Option<String>, ChecksumAlgorithm); 5] = [
        (
            "CRC32",
            output.checksum_crc32().map(str::to_string),
            ChecksumAlgorithm::Crc32,
        ),
        (
            "CRC32C",
            output.checksum_crc32_c().map(str::to_string),
            ChecksumAlgorithm::Crc32c,
        ),
        (
            "CRC64NVME",
            output.checksum_crc64_nvme().map(str::to_string),
            ChecksumAlgorithm::Crc64Nvme,
        ),
        (
            "SHA1",
            output.checksum_sha1().map(str::to_string),
            ChecksumAlgorithm::Sha1,
        ),
        (
            "SHA256",
            output.checksum_sha256().map(str::to_string),
            ChecksumAlgorithm::Sha256,
        ),
    ];
    // 파일로 저장(실패해도 원본은 결과를 보지 않는다).
    save_file(file_path, output.body).await;
    for (name, expected, algorithm) in checks {
        let Some(expected) = expected else {
            continue;
        };
        let calculated = checksum_of(file_path, algorithm)?;
        if calculated != expected {
            error!("{name} 체크섬 검증 실패 : {calculated} != {expected}");
        } else {
            info!("{name} 체크섬 검증 성공 : {calculated} == {expected}");
        }
    }
    Ok(())
}

/// `ChecksumCalculator.CalculateChecksum(filePath, algorithm)`. 파일이 없으면 `FileNotFoundException`.
fn checksum_of(
    file_path: Option<&str>,
    algorithm: ChecksumAlgorithm,
) -> Result<String, CommandError> {
    let not_found = || {
        // `new FileNotFoundException("파일을 찾을 수 없습니다.", filePath)`: 파일 이름이 있으면 `File name:` 줄이 붙는다.
        let mut message = "파일을 찾을 수 없습니다.".to_string();
        if let Some(path) = file_path {
            message.push_str(&format!("\nFile name: '{path}'"));
        }
        CommandError::new("System.IO.FileNotFoundException", message)
    };
    let Some(path) = file_path else {
        return Err(not_found());
    };
    match calculate_checksum(path, algorithm) {
        Ok(value) => Ok(value),
        Err(ChecksumError::NotFound(_)) => Err(not_found()),
        Err(ChecksumError::Io(e)) => Err(io_error(&full_path(path), &e)),
    }
}

/// `--size`: 파일을 만들고 본문을 한 번 읽어 최대 `file_size`바이트만 쓴다(원본의 `Read(buffer, 0, size)`).
/// 디렉터리는 만들지 않는다(`File.Create`).
async fn save_limited(
    path: &str,
    file_size: i64,
    output: GetObjectOutput,
) -> Result<(), CommandError> {
    let full = full_path(path);
    let mut file = tokio::fs::File::create(&full)
        .await
        .map_err(|e| io_error(&full, &e))?;
    // `new byte[fileSize]`
    let size = usize::try_from(file_size).map_err(|_| {
        CommandError::new(
            "System.OverflowException",
            "Arithmetic operation resulted in an overflow.",
        )
    })?;
    let data = output
        .body
        .collect()
        .await
        .map_err(read_error)?
        .into_bytes();
    let read = size.min(data.len());
    file.write_all(&data[..read])
        .await
        .map_err(|e| io_error(&full, &e))?;
    file.flush().await.map_err(|e| io_error(&full, &e))?;
    Ok(())
}

/// `JsonSerializer.Serialize(response.Headers)`에 해당하는 값(응답 헤더 모음).
#[derive(Debug)]
#[allow(dead_code)]
struct ResponseHeaders {
    cache_control: Option<String>,
    content_disposition: Option<String>,
    content_encoding: Option<String>,
    content_language: Option<String>,
    content_length: Option<i64>,
    content_type: Option<String>,
    e_tag: Option<String>,
    expires_string: Option<String>,
    last_modified: Option<aws_sdk_s3::primitives::DateTime>,
    x_amz_version_id: Option<String>,
    x_amz_storage_class: Option<String>,
}

impl ResponseHeaders {
    fn of(output: &HeadObjectOutput) -> Self {
        Self {
            cache_control: output.cache_control().map(str::to_string),
            content_disposition: output.content_disposition().map(str::to_string),
            content_encoding: output.content_encoding().map(str::to_string),
            content_language: output.content_language().map(str::to_string),
            content_length: output.content_length(),
            content_type: output.content_type().map(str::to_string),
            e_tag: output.e_tag().map(str::to_string),
            expires_string: output.expires_string().map(str::to_string),
            last_modified: output.last_modified().copied(),
            x_amz_version_id: output.version_id().map(str::to_string),
            x_amz_storage_class: output.storage_class().map(|s| s.as_str().to_string()),
        }
    }
}

pub(super) async fn head_object(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    let checksum_mode = o.checksum.then_some(ChecksumMode::Enabled);
    info!("HeadObject Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .head_object_with_key(
            bucket,
            key,
            o.version_id.as_deref(),
            checksum_mode,
            o.encryption_key.as_deref(),
        )
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        if o.print {
            print_json(&response.output);
            print_json(&ResponseHeaders::of(&response.output));
        }
        info!("{key} Get Object Metadata! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Get Object Metadata failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn get_object_legal_hold(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("GetObjectLegalHold Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .get_object_legal_hold(bucket, key, o.version_id.as_deref())
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        if o.print {
            print_json(&response.output.legal_hold());
        }
        info!("{key} Get Object LegalHold! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Get Object LegalHold failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn get_object_lock(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let bucket = bucket_name(ctx);
    // 원본은 `key`(없으면 빈 문자열)를 로그에 쓴다.
    let key = key_name(ctx);
    info!("GetObjectLockConfiguration({bucket}) Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .get_object_lock_configuration(bucket)
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        if o.print {
            print_json(&response.output.object_lock_configuration());
        }
        info!("{key} Get Object Lock Configuration! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Get Object Lock Configuration failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn get_object_retention(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("GetObjectRetention Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .get_object_retention(bucket, key, o.version_id.as_deref())
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        if o.print {
            print_json(&response.output.retention());
        }
        info!("{key} Get Object Retention! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Get Object Retention failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn get_object_tagging(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    info!("GetObjectTagging Start");
    let started = Instant::now();
    let response = ctx
        .client()
        .get_object_tagging(bucket, key, o.version_id.as_deref())
        .await
        .plain()?;
    let elapsed = started.elapsed().as_millis();
    if response.status == 200 {
        if o.print {
            print_json(response.output.tag_set());
        }
        info!("{key} Get Object Tagging! complete time = {elapsed}ms");
    } else {
        error!(
            "{key} Get Object Tagging failed({})",
            status_name(response.status)
        );
    }
    Ok(0)
}

pub(super) async fn get_presigned_url(ctx: &CommandContext) -> CommandResult {
    let o = &ctx.options;
    let (bucket, key) = (bucket_name(ctx), key_name(ctx));
    if o.days < 1 {
        println!("만료일은 1일 이상이어야 합니다.");
        return Ok(0);
    }
    info!("GetPresignedUrl Start");
    let expired = Utc::now() + Duration::days(i64::from(o.days));
    let url = ctx
        .client()
        .generate_presigned_url(bucket, key, expired, HttpVerb::Get, None, None)
        .await
        .plain()?;
    info!("{url}");
    Ok(0)
}
