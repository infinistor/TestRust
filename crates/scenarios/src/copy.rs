//! `Test/CopyTest.cs`: 원본 오브젝트를 읽어 대상에 올린다(`RangeReadCopy` 메뉴).
//!
//! 원본과 같게 맞춘 동작
//!
//! - `Start(true)`: `HeadObject` → `CreateMultipartUpload` → 1GiB 범위마다 `GetObject(Range)` → `UploadPart` →
//!   `CompleteMultipartUpload`. 마지막에 `CopyTest End. {ms}ms`를 남긴다.
//! - 실패해도 멀티파트 업로드를 중단(`AbortMultipartUpload`)하지 않고, 예외는 잡지 않아 호출자(최상위)로 전파된다.
//! - 버킷·키가 비어 있으면(`null` 또는 빈 문자열) 요청을 만들 때 `ArgumentException`이라 요청을 보내지 않는다
//!   (`HeadObject`는 버킷 → 키, `CreateMultipartUpload`도 버킷 → 키 순서).
//! - 범위 응답 본문은 메모리로 모두 읽은 뒤(`Utility.GetBodySplit`) `UploadPart`에 넘긴다. 원본은 응답 스트림을 그대로 넘겨
//!   .NET SDK가 `PartialWrapperStream` 예외를 던지던 버그를 TESTCore `3c4b0ea`에서 고쳤다(사용자 결정).
//! - 응답에 `PartNumber`가 없으면 .NET SDK는 요청한 번호로 채운다.
//! - `Start(false)`(`GetObject` → `PutObject`)는 호출하는 곳이 없어 옮기지 않았다.

use std::time::Instant;

use testrust_config::{CopyConfig, UserData};
use testrust_s3::S3Client;
use testrust_s3::s3_client::{PartETag, PutBody};
use tracing::info;

use crate::ScenarioError;
use crate::files::read_error;

/// 원본 `Utility.GiB`.
const GIB: i64 = 1024 * 1024 * 1024;

/// 요청을 만들 때 필수 값이 비어 있으면 SDK가 던지는 `ArgumentException`.
fn required(value: &str, property: &str, request: &str) -> Result<(), ScenarioError> {
    Ok(testrust_s3::s3_client::error::required(
        value, property, request,
    )?)
}

/// 원본 `CopyTest`.
pub struct CopyTest {
    config: CopyConfig,
    main_user: UserData,
    /// 서로 다른 시스템 사이면 대상 사용자.
    alt_user: Option<UserData>,
}

impl CopyTest {
    /// 동일 시스템 안에서 복사한다.
    pub fn new(config: CopyConfig, main_user: UserData) -> Self {
        Self {
            config,
            main_user,
            alt_user: None,
        }
    }

    /// 서로 다른 시스템(계정) 사이를 복사한다.
    pub fn with_alt(config: CopyConfig, main_user: UserData, alt_user: UserData) -> Self {
        Self {
            config,
            main_user,
            alt_user: Some(alt_user),
        }
    }

    /// 원본 `Start(rangeRead: true)`.
    pub async fn start(&self) -> Result<(), ScenarioError> {
        let started = Instant::now();
        let config = &self.config;

        // 클라이언트 생성
        let source_client = S3Client::from_user(&self.main_user, false, 3, false);
        let target_client = match &self.alt_user {
            Some(alt) => S3Client::from_user(alt, false, 3, false),
            None => source_client.clone(),
        };

        // Metadata 가져오기
        required(
            &config.source_bucket,
            "BucketName",
            "GetObjectMetadataRequest",
        )?;
        required(&config.source_object, "Key", "GetObjectMetadataRequest")?;
        let metadata = source_client
            .head_object(&config.source_bucket, &config.source_object, None, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        let size = metadata.output.content_length().unwrap_or(0);

        let mut parts: Vec<PartETag> = Vec::new();

        required(
            &config.target_bucket,
            "BucketName",
            "InitiateMultipartUploadRequest",
        )?;
        required(
            &config.target_object,
            "Key",
            "InitiateMultipartUploadRequest",
        )?;
        let init = target_client
            .initiate_multipart_upload(&config.target_bucket, &config.target_object)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        let upload_id = init.output.upload_id().unwrap_or_default().to_string();

        let mut part_number = 1;
        let mut start = 0i64;
        let part_size = GIB;
        while start < size {
            let mut end = start + part_size;
            if end > size {
                end = size;
            }

            let part = source_client
                .get_object(
                    &config.source_bucket,
                    &config.source_object,
                    None,
                    Some((start, end - 1)),
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &["NoSuchKey", "InvalidObjectState"]))?;
            // UploadPart는 탐색 가능한 스트림이 필요하므로 응답 본문을 메모리로 읽어 둔다.
            let body = part.output.body.collect().await.map_err(read_error)?;

            let part_response = target_client
                .upload_part(
                    &config.target_bucket,
                    &config.target_object,
                    &upload_id,
                    part_number,
                    PutBody::Bytes(body.into_bytes().to_vec()),
                    0,
                    -1,
                    true,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            parts.push(PartETag {
                part_number: Some(part_number),
                e_tag: part_response.output.e_tag().map(str::to_string),
                ..PartETag::default()
            });
            part_number += 1;

            start += part_size;
        }
        target_client
            .complete_multipart_upload(
                &config.target_bucket,
                &config.target_object,
                &upload_id,
                &parts,
            )
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;

        info!("CopyTest End. {}ms", started.elapsed().as_millis());
        Ok(())
    }
}
