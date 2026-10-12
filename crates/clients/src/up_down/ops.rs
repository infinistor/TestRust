//! 원본 `#region Utility`의 정적 메서드: 연산 하나를 보내고 성공 여부를 돌려준다.
//! 실패하면 원본과 같은 문구로 ERROR 로그를 남긴다.

use std::path::Path;

use aws_sdk_s3::operation::list_objects::ListObjectsOutput;
use aws_sdk_s3::types::Tag;
use testrust_common::dotnet_http::status_name;
use testrust_s3::s3_client::{PartETag, PutBody};
use testrust_s3::{S3Client, S3Error};
use tracing::error;

use super::UpDownClient;
use crate::file_util::bytes_etag;

/// 실패 로그의 대상 설명(`PutObject(bucket, key)` 같은 머리말).
pub struct OperationLog<'a> {
    pub name: &'a str,
    pub args: String,
}

/// 원본 `catch` 블록: `AmazonS3Exception`이면 상태·오류 코드를 쓰고, 그 밖의 예외는 `log.Error(e)`.
fn log_error(head: &OperationLog<'_>, error: &S3Error) {
    match error {
        S3Error::Service { status, code, .. } => error!(
            "{}({}) StatusCode : {}, ErrorCode : {code}. {error}",
            head.name,
            head.args,
            status_name(*status)
        ),
        other => error!("{}: {other}", other.dotnet_type()),
    }
}

fn log(name: &str, args: String) -> OperationLog<'_> {
    OperationLog { name, args }
}

/// 원본 `IsObject`: Head가 200이고 크기가 같으면 `true`. 예외는 `false`(로그 없음).
pub async fn is_object(client: &S3Client, bucket: &str, key: &str, file_size: i64) -> bool {
    match client.head_object(bucket, key, None, None).await {
        Ok(response) => {
            response.status == 200 && response.output.content_length().unwrap_or(0) == file_size
        }
        Err(_) => false,
    }
}

/// 원본 `PutObject(client, bucket, key, filePath, useChunkEncoding)`.
pub async fn put_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    file_path: &Path,
    use_chunk_encoding: bool,
) -> bool {
    let body = PutBody::File(file_path.to_path_buf());
    match client
        .put_object(bucket, key, body, use_chunk_encoding, None)
        .await
    {
        Ok(response) if response.status == 200 => true,
        Ok(response) => {
            error!(
                "PutObject Failed({}). S3://{bucket}/{key}",
                status_name(response.status)
            );
            false
        }
        Err(e) => {
            log_error(&log("PutObject", format!("{bucket}, {key}")), &e);
            false
        }
    }
}

/// 원본 `PutDirectory`: 빈 본문, 청크 인코딩 없이.
pub async fn put_directory(client: &S3Client, bucket: &str, key: &str) -> bool {
    match client
        .put_object(bucket, key, PutBody::Text(String::new()), false, None)
        .await
    {
        Ok(response) if response.status == 200 => true,
        Ok(response) => {
            error!(
                "PutDirectory Failed({}). S3://{bucket}/{key}",
                status_name(response.status)
            );
            false
        }
        Err(e) => {
            log_error(&log("PutDirectory", format!("{bucket}, {key}")), &e);
            false
        }
    }
}

/// 원본 `ToDirectoryKey`.
pub fn to_directory_key(key: String) -> String {
    if key.ends_with('/') {
        key
    } else {
        format!("{key}/")
    }
}

/// 원본 `UploadObject`: TransferUtility 업로드.
pub async fn upload_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    file_path: &Path,
    part_size: i64,
) -> bool {
    match client
        .upload(
            bucket,
            key,
            Some(file_path),
            part_size,
            10,
            None,
            None,
            None,
        )
        .await
    {
        Ok(()) => true,
        Err(e) => {
            log_error(&log("UploadObject", format!("{bucket}, {key}")), &e);
            false
        }
    }
}

/// 원본 `HeadObject`: 성공하면 응답의 버전 ID(헤더가 없으면 `None` = .NET `null`)를, 실패하면 원본의 초기값
/// `string.Empty`(`Some("")`)를 돌려준다.
pub async fn head_object(client: &S3Client, bucket: &str, key: &str) -> (bool, Option<String>) {
    let failed = || (false, Some(String::new()));
    match client.head_object(bucket, key, None, None).await {
        Ok(response) if response.status == 200 => {
            (true, response.output.version_id().map(str::to_string))
        }
        Ok(response) => {
            error!(
                "HeadObject Failed({}). S3://{bucket}/{key}",
                status_name(response.status)
            );
            failed()
        }
        Err(e) => {
            log_error(&log("HeadObject", format!("{bucket}, {key}")), &e);
            failed()
        }
    }
}

/// 원본 `GetObject`: 상태 200, 크기 일치, (`etag`가 있으면) MD5 일치를 확인한다.
/// 응답 본문은 어느 경우든 끝까지 읽는다(원본 `GetETag`·`GetBodySplit`). 측정 구간에 본문 수신이 들어간다.
pub async fn get_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    file_size: i64,
    etag: Option<&str>,
) -> bool {
    let head = || log("GetObject", format!("{bucket}, {key}"));
    let response = match client.get_object(bucket, key, None, None).await {
        Ok(response) => response,
        Err(e) => {
            log_error(&head(), &e);
            return false;
        }
    };
    if response.status != 200 {
        error!(
            "GetObject Failed({}). S3://{bucket}/{key}",
            status_name(response.status)
        );
        return false;
    }
    let content_length = response.output.content_length().unwrap_or(0);
    if file_size != content_length {
        error!(
            "GetObject Failed. S3://{bucket}/{key} - FileSize does not match!({file_size} != {content_length})"
        );
        return false;
    }
    let body = match response.output.body.collect().await {
        Ok(body) => body.into_bytes(),
        Err(e) => {
            error!("System.IO.IOException: {e}");
            return false;
        }
    };
    if let Some(etag) = etag {
        let actual = bytes_etag(&body);
        if !etag.eq_ignore_ascii_case(&actual) {
            error!(
                "GetObject Failed. S3://{bucket}/{key} - md5sum does not match!({etag} != {actual})"
            );
            return false;
        }
    }
    true
}

/// 원본 `DownloadObject`: TransferUtility 다운로드.
pub async fn download_object(client: &S3Client, bucket: &str, key: &str, file_path: &Path) -> bool {
    match client.download(bucket, key, file_path, None).await {
        Ok(()) => true,
        Err(e) => {
            log_error(&log("DownloadObject", format!("{bucket}, {key}")), &e);
            false
        }
    }
}

/// 원본 `PutObjectTag`: 태그 하나를 붙여 업로드(청크 인코딩 없이).
pub async fn put_object_tag(
    client: &S3Client,
    bucket: &str,
    key: &str,
    file_path: &Path,
    tag: Tag,
) -> bool {
    let body = PutBody::File(file_path.to_path_buf());
    match client
        .put_object(bucket, key, body, false, Some(vec![tag]))
        .await
    {
        Ok(response) if response.status == 200 => true,
        Ok(response) => {
            error!(
                "PutObjectTag Failed({}). S3://{bucket}/{key}",
                status_name(response.status)
            );
            false
        }
        Err(e) => {
            log_error(&log("PutObjectTag", format!("{bucket}, {key}")), &e);
            false
        }
    }
}

/// 원본 `DeleteObject`: 204면 성공.
pub async fn delete_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    version_id: Option<&str>,
) -> bool {
    match client.delete_object(bucket, key, version_id, None).await {
        Ok(response) if response.status == 204 => true,
        Ok(response) => {
            error!(
                "DeleteObject Failed({}). S3://{bucket}/{key}_{}",
                status_name(response.status),
                version_id.unwrap_or_default()
            );
            false
        }
        Err(e) => {
            log_error(
                &log(
                    "DeleteObject",
                    format!("{bucket}, {key}, {}", version_id.unwrap_or_default()),
                ),
                &e,
            );
            false
        }
    }
}

/// 원본 `ListObjects`: 실패하면 `None`.
pub async fn list_objects(
    client: &S3Client,
    bucket: &str,
    prefix: Option<&str>,
    marker: Option<&str>,
    delimiter: Option<&str>,
) -> Option<ListObjectsOutput> {
    match client
        .list_objects(
            bucket,
            prefix,
            marker,
            testrust_s3::s3_client::S3_MAX_KEYS,
            delimiter,
        )
        .await
    {
        Ok(response) if response.status == 200 => Some(response.output),
        Ok(response) => {
            error!(
                "ListObjects Failed({}). S3://{bucket}/{}",
                status_name(response.status),
                prefix.unwrap_or_default()
            );
            None
        }
        Err(e) => {
            log_error(
                &log(
                    "ListObjects",
                    format!(
                        "{bucket}, {}, {}, {}",
                        prefix.unwrap_or_default(),
                        marker.unwrap_or_default(),
                        delimiter.unwrap_or_default()
                    ),
                ),
                &e,
            );
            None
        }
    }
}

impl UpDownClient {
    /// 원본 `MultipartUpload`: 파트를 하나씩 올린다. 중간에 `Quit`이면 업로드를 중단(abort)하고 `false`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn multipart_upload(
        &self,
        client: &S3Client,
        bucket: &str,
        key: &str,
        file_path: &Path,
        file_size: i64,
        part_size: i64,
        use_chunk_encoding: bool,
    ) -> bool {
        if self.quit.get() {
            return false;
        }
        let result: Result<bool, S3Error> = async {
            let init = client.initiate_multipart_upload(bucket, key).await?;
            if init.status != 200 {
                error!(
                    "MultipartUpload Failed({}). S3://{bucket}/{key}",
                    status_name(init.status)
                );
                return Ok(false);
            }
            let upload_id = init.output.upload_id().unwrap_or_default().to_string();
            let mut part_number = 1;
            let mut start = 0i64;
            let mut parts = Vec::new();
            while start < file_size && !self.quit.get() {
                let response = client
                    .upload_part(
                        bucket,
                        key,
                        &upload_id,
                        part_number,
                        PutBody::File(file_path.to_path_buf()),
                        start,
                        part_size,
                        use_chunk_encoding,
                    )
                    .await?;
                if response.status != 200 {
                    error!(
                        "MultipartUpload Failed({}). S3://{bucket}/{key}",
                        status_name(response.status)
                    );
                    client
                        .abort_multipart_upload(bucket, key, &upload_id)
                        .await?;
                    return Ok(false);
                }
                parts.push(PartETag::new(part_number, response.output.e_tag()));
                start += part_size;
                part_number += 1;
                // 원본 `onPartUploaded`: `Stats.Write.Part++`
                self.stats.write.add_part(1);
            }
            if self.quit.get() {
                client
                    .abort_multipart_upload(bucket, key, &upload_id)
                    .await?;
                return Ok(false);
            }
            let complete = client
                .complete_multipart_upload(bucket, key, &upload_id, &parts)
                .await?;
            if complete.status == 200 {
                return Ok(true);
            }
            error!(
                "MultipartUpload Failed({}). S3://{bucket}/{key}",
                status_name(complete.status)
            );
            Ok(false)
        }
        .await;
        match result {
            Ok(done) => done,
            Err(e) => {
                log_error(&log("MultipartUpload", format!("{bucket}, {key}")), &e);
                false
            }
        }
    }
}
