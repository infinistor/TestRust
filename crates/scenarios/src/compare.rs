//! `Test/CompareTest.cs`: 소스/타겟 버킷의 목록·메타데이터·태그를 비교한다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - `VersionCheck`이면 전체 버전(삭제 마커 포함 개수, 비교는 삭제 마커 제외), 아니면 현재 버전 목록만 페이지 단위로 비교한다.
//!   소스·타겟 목록은 같은 마커(소스 응답의 `NextMarker`)로 조회한다(TESTCore `c83e35f`에서 고친 판).
//! - `HeadObject`는 `ChecksumMode` 없이 호출하므로 서버가 체크섬 헤더를 주지 않으면 체크섬은 양쪽 모두 `null`이라 같다.
//! - 목록 항목의 버킷 이름은 원본 `S3Object.BucketName`(SDK가 응답의 `Name`을 채운다)을 쓴다.
//! - 예외는 잡지 않는다. S3 오류와 아래 `NullReferenceException`은 호출자(최상위)로 전파된다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - AWSSDK v4는 항목이 하나도 없는 목록(`S3Objects`, `Versions`)과 태그 목록을 `null`로 돌려준다. 원본이 `.Count`를 부르는
//!   지점(소스 개수 합산, 현재 버전 목록의 타겟 개수 비교)에서 `NullReferenceException`이 난다. 빈 버킷끼리 비교해도 마찬가지다.
//!   전체 버전 목록의 타겟이 `null`이면 `Versions.Where`에서 `ArgumentNullException`(Parameter 'source')이다.
//! - `ReplicationCheck`에서 `x-amz-replication-status` 헤더가 없으면 `ReplicationStatus.Equals`가 `NullReferenceException`이다.
//!   값 비교는 `ConstantClass.Equals`라 대소문자를 구분하지 않는다.
//! - 소스에만 있는 메타데이터 키를 타겟에서 읽으면 빈 문자열이다(`KeyNotFoundException`이 아니다). 메타데이터 키 순서는 .NET이 응답 헤더 순서지만
//!   SDK 모델에서는 잃어 이름순으로 비교한다.
//! - `Metadata(${item})`, `Tagging(${index})`의 `$`는 보간이 아니라 글자 그대로 출력된다.
//! - `Versions`는 `Version`과 `DeleteMarker`를 한 목록으로 담지만 SDK 응답에서는 문서 순서를 잃어 버전 다음에 삭제 마커 순이다.
//! - `Size`·`ContentLength` 등 `null`일 수 있는 값은 로그에서 빈 문자열이 된다.

use aws_sdk_s3::types::{ChecksumType, Tag};
use chrono::{DateTime, Utc};
use testrust_config::{CompareConfig, UserData};
use testrust_s3::S3Client;
use testrust_s3::s3_client::ListVersions;
use tracing::{error, info};

use crate::ScenarioError;
use crate::input::null_reference;

/// 원본 `new S3Client(user)`: 재시도 3회, 요청 체크섬 계산 안 함.
fn client_of(user: &UserData) -> S3Client {
    S3Client::from_user(user, false, 3, false)
}

/// .NET `DateTime.ToString()`(ko-KR, `yyyy-MM-dd tt h:mm:ss`). 시각은 UTC 그대로.
pub(crate) fn ko_kr_datetime(time: DateTime<Utc>) -> String {
    testrust_common::dotnet_format::ko_kr_datetime(&time)
}

/// SDK 시각을 [`ko_kr_datetime`]으로. `DateTime?`가 `null`이면 빈 문자열(문자열 보간).
pub(crate) fn ko_kr_sdk_time(time: Option<&aws_sdk_s3::primitives::DateTime>) -> String {
    time.and_then(|t| DateTime::<Utc>::from_timestamp(t.secs(), t.subsec_nanos()))
        .map(ko_kr_datetime)
        .unwrap_or_default()
}

/// 문자열 보간에서 `null`은 빈 문자열.
fn text(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or_default()
}

/// 문자열 보간에서 `long?`·`int?`의 `null`은 빈 문자열.
fn number<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// 문자열 보간에서 `bool?`는 `True`/`False`/빈 문자열.
fn boolean(value: Option<bool>) -> String {
    value
        .map(|v| if v { "True" } else { "False" }.to_string())
        .unwrap_or_default()
}

/// 목록 응답의 항목 하나(원본 `S3Object`·`S3ObjectVersion`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ListedObject {
    /// SDK가 응답의 `Name`으로 채우는 `BucketName`.
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub e_tag: Option<String>,
    pub size: Option<i64>,
    pub version_id: Option<String>,
    pub is_latest: Option<bool>,
    pub last_modified: Option<aws_sdk_s3::primitives::DateTime>,
    /// 원본 `IsDeleteMarker`(`Version`은 `false`, `DeleteMarker`는 `true`).
    pub is_delete_marker: bool,
}

/// 원본 `ListVersionsResponse.Versions`: 버전과 삭제 마커를 응답 문서 순서로. 하나도 없으면 원본은 `null`이라 `None`.
pub(crate) fn version_entries(output: &ListVersions) -> Option<Vec<ListedObject>> {
    let bucket = output.name().map(str::to_string);
    let all: Vec<ListedObject> = output
        .entries()?
        .iter()
        .map(|entry| ListedObject {
            bucket: bucket.clone(),
            key: entry.key().map(str::to_string),
            e_tag: entry.e_tag().map(str::to_string),
            size: entry.size(),
            version_id: entry.version_id().map(str::to_string),
            is_latest: entry.is_latest(),
            last_modified: entry.last_modified().copied(),
            is_delete_marker: entry.is_delete_marker(),
        })
        .collect();
    (!all.is_empty()).then_some(all)
}

/// 원본 `CompareTest`.
pub struct CompareTest {
    config: CompareConfig,
    source_client: S3Client,
    target_client: S3Client,
}

impl CompareTest {
    /// 동일 시스템 안에서 비교한다(소스·타겟이 같은 클라이언트).
    pub fn new(config: CompareConfig, main_user: &UserData) -> Self {
        let source_client = client_of(main_user);
        Self {
            config,
            target_client: source_client.clone(),
            source_client,
        }
    }

    /// 서로 다른 시스템(계정) 사이를 비교한다.
    pub fn with_alt(config: CompareConfig, main_user: &UserData, alt_user: &UserData) -> Self {
        Self {
            config,
            source_client: client_of(main_user),
            target_client: client_of(alt_user),
        }
    }

    /// 원본 `Start`: 일치하면 0, 아니면 -1.
    pub async fn start(&self) -> Result<i32, ScenarioError> {
        let (matched, count) = if self.config.version_check {
            self.all().await?
        } else {
            self.current().await?
        };
        if matched {
            info!(
                "{}({count}) is Match {}",
                self.config.source_bucket, self.config.target_bucket
            );
            Ok(0)
        } else {
            error!(
                "{} is not Match {}",
                self.config.source_bucket, self.config.target_bucket
            );
            Ok(-1)
        }
    }

    /// 원본 `Current(out count)`. 반환은 (일치 여부, 비교한 개수).
    async fn current(&self) -> Result<(bool, usize), ScenarioError> {
        let mut count = 0usize;
        let mut is_truncated = true;
        let mut next_marker = String::new();
        while is_truncated {
            let source = self
                .source_client
                .list_objects(
                    &self.config.source_bucket,
                    None,
                    Some(&next_marker),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &["NoSuchBucket"]))?;
            let target = self
                .target_client
                .list_objects(
                    &self.config.target_bucket,
                    None,
                    Some(&next_marker),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &["NoSuchBucket"]))?;
            let source_bucket = source.output.name().map(str::to_string);
            let target_bucket = target.output.name().map(str::to_string);
            let to_listed = |output: &aws_sdk_s3::operation::list_objects::ListObjectsOutput,
                             bucket: &Option<String>| {
                output
                    .contents()
                    .iter()
                    .map(|o| ListedObject {
                        bucket: bucket.clone(),
                        key: o.key().map(str::to_string),
                        e_tag: o.e_tag().map(str::to_string),
                        size: o.size(),
                        ..ListedObject::default()
                    })
                    .collect::<Vec<_>>()
            };
            let source_keys = to_listed(&source.output, &source_bucket);
            let target_keys = to_listed(&target.output, &target_bucket);

            // `sourceResponse.S3Objects`가 `null`
            if source_keys.is_empty() {
                return Err(null_reference());
            }
            count += source_keys.len();
            if source.output.is_truncated() == Some(true) {
                // .NET SDK는 `NextMarker`가 없으면 마지막 키를 쓴다.
                next_marker = source
                    .output
                    .next_marker()
                    .or_else(|| source.output.contents().last().and_then(|o| o.key()))
                    .unwrap_or_default()
                    .to_string();
                info!("Next : {next_marker}");
            } else {
                is_truncated = false;
            }

            // `targetResponse.S3Objects`가 `null`
            if target_keys.is_empty() {
                return Err(null_reference());
            }
            if source_keys.len() != target_keys.len() {
                error!(
                    "Source : Target != {} : {}",
                    source_keys.len(),
                    target_keys.len()
                );
                return Ok((false, count));
            }
            for (a, b) in source_keys.iter().zip(&target_keys) {
                if !self.compare_listed(a, b).await? {
                    return Ok((false, count));
                }
            }
        }
        Ok((true, count))
    }

    /// 원본 `All(out count)`. 반환은 (일치 여부, 비교한 개수).
    async fn all(&self) -> Result<(bool, usize), ScenarioError> {
        let mut count = 0usize;
        let mut is_truncated = true;
        let mut next_key_marker: Option<String> = None;
        let mut next_version_id_marker: Option<String> = None;
        while is_truncated {
            let source = self
                .source_client
                .list_versions(
                    &self.config.source_bucket,
                    None,
                    next_key_marker.as_deref(),
                    next_version_id_marker.as_deref(),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            let target = self
                .target_client
                .list_versions(
                    &self.config.target_bucket,
                    None,
                    next_key_marker.as_deref(),
                    next_version_id_marker.as_deref(),
                    1000,
                    None,
                )
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;

            // `sourceResponse.Versions`가 `null`
            let source_versions = version_entries(&source.output).ok_or_else(null_reference)?;
            count += source_versions.len();
            if source.output.is_truncated() == Some(true) {
                next_key_marker = source.output.next_key_marker().map(str::to_string);
                next_version_id_marker = source.output.next_version_id_marker().map(str::to_string);
                info!(
                    "Next : {}, {}",
                    next_key_marker.as_deref().unwrap_or_default(),
                    next_version_id_marker.as_deref().unwrap_or_default()
                );
            } else {
                is_truncated = false;
            }

            // `targetResponse.Versions.Where(...)`: 목록이 `null`이면 LINQ가 `ArgumentNullException`을 던진다.
            let target_versions = version_entries(&target.output).ok_or_else(|| {
                ScenarioError::new(
                    "System.ArgumentNullException",
                    "Value cannot be null. (Parameter 'source')",
                )
            })?;
            let source_keys: Vec<&ListedObject> = source_versions
                .iter()
                .filter(|v| !v.is_delete_marker)
                .collect();
            let target_keys: Vec<&ListedObject> = target_versions
                .iter()
                .filter(|v| !v.is_delete_marker)
                .collect();

            // 갯수가 일치하는지 비교
            if source_keys.len() != target_keys.len() {
                error!(
                    "Source : Target != {} : {}",
                    source_keys.len(),
                    target_keys.len()
                );
                return Ok((false, count));
            }
            // 목록이 일치하는지 비교
            for (a, b) in source_keys.iter().zip(&target_keys) {
                if !self.compare_version(a, b).await? {
                    return Ok((false, count));
                }
            }
        }
        Ok((true, count))
    }

    /// 원본 `CompareAtoB(S3Object a, S3Object b)`.
    async fn compare_listed(
        &self,
        a: &ListedObject,
        b: &ListedObject,
    ) -> Result<bool, ScenarioError> {
        let Some(a_key) = &a.key else {
            return Err(null_reference());
        };
        if a.key != b.key {
            error!("{a_key} != {}", text(&b.key));
            return Ok(false);
        }
        if self.config.etag_check && a.e_tag != b.e_tag {
            error!(
                "{a_key} ETag does not match! {} != {}",
                text(&a.e_tag),
                text(&b.e_tag)
            );
            return Ok(false);
        }
        if a.size != b.size {
            error!(
                "{a_key} Size does not match! {} != {}",
                number(a.size),
                number(b.size)
            );
            return Ok(false);
        }

        // MetadataCheck=false면 List 결과만 비교하고 HeadObject는 생략한다.
        if self.config.metadata_check
            && !self
                .compare_head(
                    text(&a.bucket),
                    a_key,
                    None,
                    text(&b.bucket),
                    text(&b.key),
                    None,
                )
                .await?
        {
            return Ok(false);
        }

        if self.config.tag_check {
            return self
                .compare_tags(
                    text(&a.bucket),
                    a_key,
                    None,
                    text(&b.bucket),
                    text(&b.key),
                    None,
                )
                .await;
        }
        Ok(true)
    }

    /// 원본 `CompareAtoB(S3ObjectVersion a, S3ObjectVersion b)`.
    async fn compare_version(
        &self,
        a: &ListedObject,
        b: &ListedObject,
    ) -> Result<bool, ScenarioError> {
        let Some(a_key) = &a.key else {
            return Err(null_reference());
        };
        if a.key != b.key {
            error!("{a_key} != {}", text(&b.key));
            return Ok(false);
        }
        if self.config.etag_check && a.e_tag != b.e_tag {
            error!(
                "{a_key} ETag does not match! {} != {}",
                text(&a.e_tag),
                text(&b.e_tag)
            );
            return Ok(false);
        }
        if a.version_id != b.version_id {
            error!(
                "{a_key} VersionId does not match! {} != {}",
                text(&a.version_id),
                text(&b.version_id)
            );
            return Ok(false);
        }
        if a.is_latest != b.is_latest {
            error!(
                "{a_key} IsLatest does not match! {} != {}",
                boolean(a.is_latest),
                boolean(b.is_latest)
            );
            return Ok(false);
        }
        if a.size != b.size {
            error!(
                "{a_key} Size does not match! {} != {}",
                number(a.size),
                number(b.size)
            );
            return Ok(false);
        }

        // MetadataCheck=false면 List 결과만 비교하고 HeadObject는 생략한다.
        if self.config.metadata_check
            && !self
                .compare_head(
                    text(&a.bucket),
                    a_key,
                    a.version_id.as_deref(),
                    text(&b.bucket),
                    text(&b.key),
                    b.version_id.as_deref(),
                )
                .await?
        {
            return Ok(false);
        }

        if self.config.tag_check {
            return self
                .compare_tags(
                    text(&a.bucket),
                    a_key,
                    a.version_id.as_deref(),
                    text(&b.bucket),
                    text(&b.key),
                    b.version_id.as_deref(),
                )
                .await;
        }
        Ok(true)
    }

    /// 원본 `CompareAtoB(sourceBucket, sourceKey, sourceVersionId, targetBucket, targetKey, targetVersionId)`:
    /// `HeadObject`로 읽은 메타데이터를 비교한다.
    async fn compare_head(
        &self,
        source_bucket: &str,
        source_key: &str,
        source_version_id: Option<&str>,
        target_bucket: &str,
        target_key: &str,
        target_version_id: Option<&str>,
    ) -> Result<bool, ScenarioError> {
        // 메타데이터 읽기
        let source = self
            .source_client
            .head_object(source_bucket, source_key, source_version_id, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?
            .output;
        let target = self
            .target_client
            .head_object(target_bucket, target_key, target_version_id, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?
            .output;

        // Replication 체크
        if self.config.replication_check {
            let Some(status) = source.replication_status() else {
                return Err(null_reference());
            };
            if !status.as_str().eq_ignore_ascii_case("COMPLETED") {
                error!("{source_key} ReplicationStatus {}!", status.as_str());
                return Ok(false);
            }
        }

        // 메타데이터 비교
        // .NET `ContentLength`는 `long`이라 헤더가 없으면 0이다.
        let (source_length, target_length) = (
            source.content_length().unwrap_or(0),
            target.content_length().unwrap_or(0),
        );
        if source_length != target_length {
            error!("{source_key} Does not match! ContentLength {source_length} != {target_length}");
            return Ok(false);
        }
        if source.last_modified() != target.last_modified() {
            error!(
                "{source_key} Does not match! LastModified {} != {}",
                ko_kr_sdk_time(source.last_modified()),
                ko_kr_sdk_time(target.last_modified())
            );
            return Ok(false);
        }
        if self.config.checksum_check {
            let checks: [(&str, Option<&str>, Option<&str>); 5] = [
                (
                    "ChecksumCRC32",
                    source.checksum_crc32(),
                    target.checksum_crc32(),
                ),
                (
                    "ChecksumCRC32C",
                    source.checksum_crc32_c(),
                    target.checksum_crc32_c(),
                ),
                (
                    "ChecksumCRC64NVME",
                    source.checksum_crc64_nvme(),
                    target.checksum_crc64_nvme(),
                ),
                (
                    "ChecksumSHA1",
                    source.checksum_sha1(),
                    target.checksum_sha1(),
                ),
                (
                    "ChecksumSHA256",
                    source.checksum_sha256(),
                    target.checksum_sha256(),
                ),
            ];
            for (name, a, b) in checks {
                if a != b {
                    error!(
                        "{source_key} Does not match! {name} {} != {}",
                        a.unwrap_or_default(),
                        b.unwrap_or_default()
                    );
                    return Ok(false);
                }
            }
            if source.checksum_type() != target.checksum_type() {
                fn name(t: Option<&ChecksumType>) -> &str {
                    t.map(|t| t.as_str()).unwrap_or_default()
                }
                error!(
                    "{source_key} Does not match! ChecksumType {} != {}",
                    name(source.checksum_type()),
                    name(target.checksum_type())
                );
                return Ok(false);
            }
        }

        let empty = std::collections::HashMap::new();
        let source_metadata = source.metadata().unwrap_or(&empty);
        let target_metadata = target.metadata().unwrap_or(&empty);
        if source_metadata.len() != target_metadata.len() {
            error!(
                "{source_key} Does not match! Metadata.Count {} != {}",
                source_metadata.len(),
                target_metadata.len()
            );
            return Ok(false);
        }
        let mut keys: Vec<&String> = source_metadata.keys().collect();
        // .NET `Metadata.Keys`는 받은 순서가 아니라 헤더 이름의 서수(ordinal) 순서다(오라클로 확인,
        // `sdk-metadata-header-order`). 다만 .NET은 서버가 보낸 대소문자를 그대로 두고, Rust(hyper)는 헤더 이름을
        // 소문자로 바꾸므로 대문자가 섞인 메타데이터 헤더는 이름과 순서가 다를 수 있다.
        keys.sort();
        for item in keys {
            // `MetadataCollection`의 키는 `x-amz-meta-` 접두사가 붙는다.
            let name = format!("x-amz-meta-{item}");
            // 타겟에 없는 키를 읽으면 `MetadataCollection` 인덱서는 빈 문자열을 돌려준다.
            let target_value = target_metadata.get(item).map_or("", String::as_str);
            let source_value = &source_metadata[item];
            if source_value != target_value {
                error!(
                    "{source_key} Does not match! Metadata(${name}) {source_value} != {target_value}"
                );
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// 원본 `CompareTagAtoB(...)`: `GetObjectTagging`으로 읽은 태그의 개수와 Key/Value를 비교한다.
    async fn compare_tags(
        &self,
        source_bucket: &str,
        source_key: &str,
        source_version_id: Option<&str>,
        target_bucket: &str,
        target_key: &str,
        target_version_id: Option<&str>,
    ) -> Result<bool, ScenarioError> {
        // 태그 정보 읽기
        let source = self
            .source_client
            .get_object_tagging(source_bucket, source_key, source_version_id)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?
            .output;
        let target = self
            .target_client
            .get_object_tagging(target_bucket, target_key, target_version_id)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?
            .output;
        let source_tags: &[Tag] = source.tag_set();
        let target_tags: &[Tag] = target.tag_set();

        // null 체크(태그가 하나도 없으면 원본은 `Tagging`이 `null`)
        if source_tags.is_empty() {
            if target_tags.is_empty() {
                return Ok(true); // 둘 다 null이면 같음
            }
            error!("{source_key} Does not match! Source tagging is null but target is not");
            return Ok(false);
        }
        if target_tags.is_empty() {
            error!("{source_key} Does not match! Target tagging is null but source is not");
            return Ok(false);
        }

        if source_tags.len() != target_tags.len() {
            error!(
                "{source_key} Does not match! Tagging.Count {} != {}",
                source_tags.len(),
                target_tags.len()
            );
            return Ok(false);
        }
        for (index, (a, b)) in source_tags.iter().zip(target_tags).enumerate() {
            if a.key() != b.key() {
                error!(
                    "{source_key} Does not match! Tagging(${index}).Key {} != {}",
                    a.key(),
                    b.key()
                );
                return Ok(false);
            }
            if a.value() != b.value() {
                error!(
                    "{source_key} Does not match! Tagging(${index}).Value {} != {}",
                    a.value(),
                    b.value()
                );
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ko_kr_format() {
        let time = DateTime::<Utc>::from_timestamp(1_704_067_200, 0).unwrap();
        assert_eq!(ko_kr_datetime(time), "2024-01-01 오전 12:00:00");
        let time = DateTime::<Utc>::from_timestamp(1_704_067_200 + 13 * 3600 + 5, 0).unwrap();
        assert_eq!(ko_kr_datetime(time), "2024-01-01 오후 1:00:05");
        assert_eq!(ko_kr_sdk_time(None), "");
    }
}
