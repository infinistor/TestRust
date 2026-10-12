//! `Test/ClearTest.cs`: 버킷·객체 비우기.
//!
//! 원본과 같게 맞춘 동작
//!
//! - 삭제 작업(`Task.Run`)은 동시에 실행한다. `AllClear`의 버킷별 `Clear`는 목록 조회가 끝나면 작업을 모두 기다린
//!   뒤 로그를 남기지만, `BucketClear`·`CurrentClear`·`NoncurrentClear`·`MarkerClear`는 작업을 모았다가
//!   `WaitTasks`에서 기다린다. 원본은 작업이 이미 돌고 있어 완료 로그가 `WaitTasks` 로그와 섞여 나오지만(경쟁),
//!   여기서는 `WaitTasks(N)` 로그를 먼저 남기고 모은 작업을 한꺼번에 실행해 순서를 고정한다.
//! - 예외는 모두 잡아 `ERROR`로 기록하고 그때까지 센 개수를 돌려준다(`AllClear`의 버킷 목록 조회만 예외를 던진다).
//! - `ListObjects` 응답이 잘렸는데 `NextMarker`가 없으면 .NET SDK처럼 마지막 키를 다음 마커로 쓴다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - `CurrentClear`는 객체가 하나도 없는 응답에서 `S3Objects`가 `null`이라 `NullReferenceException`을 로그로 남긴다.
//! - `ObjectClear`의 접미사 비교(`EndsWith`)는 문화권 비교이지만 여기서는 서수 비교다.
//! - 버전과 삭제 마커를 한 목록(`Versions`)으로 다루지만, SDK 응답에서는 따로 와서 버전 다음에 삭제 마커 순으로 합친다.

use std::sync::Arc;

use aws_sdk_s3::types::{AccessControlPolicy, Grant, Grantee, ObjectLockEnabled, Permission, Type};
use testrust_common::dotnet_format::bool_text as dotnet_bool;
use testrust_common::dotnet_http::status_name;
use testrust_s3::{S3Client, S3Error};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::ScenarioError;
use crate::input::null_reference;

/// 삭제할 (키, 버전 ID).
type Keys = Vec<(String, Option<String>)>;

/// 원본 `ClearTest`. `_tasks`는 `WaitTasks`에서 실행할 삭제 작업이다.
pub struct ClearTest {
    client: S3Client,
    tasks: Vec<(String, Keys)>,
}

impl ClearTest {
    pub fn new(client: S3Client) -> Self {
        Self {
            client,
            tasks: Vec::new(),
        }
    }

    /// 원본 `AllClear`
    pub async fn all_clear(
        &mut self,
        prefix: Option<&str>,
        max_keys: i32,
        is_delete: bool,
        thread_count: i32,
    ) -> Result<(), ScenarioError> {
        info!(
            "AllClear({}, {max_keys}, {}, {thread_count})",
            prefix.unwrap_or_default(),
            dotnet_bool(is_delete)
        );
        let mut continuation_token: Option<String> = None;
        let mut total_deleted = 0usize;
        loop {
            let response = self
                .client
                .list_buckets(prefix, 10000, continuation_token.as_deref())
                .await
                .map_err(|e| ScenarioError::s3(e, &[]))?;
            let buckets: Vec<String> = response
                .output
                .buckets()
                .iter()
                .filter_map(|b| b.name().map(str::to_string))
                .collect();

            let semaphore = Arc::new(Semaphore::new(
                usize::try_from(thread_count).unwrap_or(1).max(1),
            ));
            let mut set = JoinSet::new();
            for bucket in &buckets {
                let client = self.client.clone();
                let bucket = bucket.clone();
                let semaphore = semaphore.clone();
                set.spawn(async move {
                    let _permit = semaphore.acquire().await;
                    clear(&client, &bucket, max_keys, is_delete).await
                });
            }
            while let Some(count) = set.join_next().await {
                total_deleted += count.unwrap_or(0);
            }

            if buckets.len() == 1000 {
                continuation_token = response.output.continuation_token().map(str::to_string);
            } else {
                break;
            }
        }
        info!("AllClear completed - Total deleted objects: {total_deleted}");
        Ok(())
    }

    /// 원본 `BucketClear`
    pub async fn bucket_clear(
        &mut self,
        bucket: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_keys: i32,
        is_delete: bool,
    ) {
        info!(
            "BucketClear({bucket}, {}, {}, {max_keys})",
            prefix.unwrap_or_default(),
            suffix.unwrap_or_default()
        );
        let total_deleted = self.object_clear(bucket, prefix, suffix, max_keys).await;
        self.wait_tasks(bucket).await;
        if is_delete {
            delete_bucket(&self.client, bucket).await;
        }
        info!("BucketClear completed - Total deleted objects: {total_deleted}");
    }

    /// 원본 `CurrentClear(bucketName)`
    pub async fn current_clear(&mut self, bucket: &str) {
        info!("CurrentClear({bucket})");
        let total_deleted = self.current_object_clear(bucket).await;
        self.wait_tasks(bucket).await;
        info!("CurrentClear completed - Total deleted objects: {total_deleted}");
    }

    /// 원본 `NoncurrentClear(bucketName)`
    pub async fn noncurrent_clear(&mut self, bucket: &str) {
        info!("NoncurrentClear({bucket})");
        let total_deleted = self.versions_clear(bucket, false).await;
        self.wait_tasks(bucket).await;
        info!("NoncurrentClear completed - Total deleted objects: {total_deleted}");
    }

    /// 원본 `MarkerClear(bucketName)`
    pub async fn marker_clear(&mut self, bucket: &str) {
        info!("MarkerClear({bucket})");
        let total_deleted = self.versions_clear(bucket, true).await;
        self.wait_tasks(bucket).await;
        info!("MarkerClear completed - Total deleted objects: {total_deleted}");
    }

    /// 원본 `ObjectClear`
    async fn object_clear(
        &mut self,
        bucket: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_keys: i32,
    ) -> usize {
        let mut total_deleted = 0;
        let mut next_marker: Option<String> = None;
        let mut next_version_id_marker: Option<String> = None;
        loop {
            let response = match self
                .client
                .list_versions(
                    bucket,
                    prefix,
                    next_marker.as_deref(),
                    next_version_id_marker.as_deref(),
                    max_keys,
                    None,
                )
                .await
            {
                Ok(response) => response,
                Err(e) => {
                    error!("{}", ScenarioError::s3(e, &[]));
                    return total_deleted;
                }
            };
            let mut keys = version_keys(&response.output, |_| true);
            if !keys.is_empty() {
                if let Some(suffix) = suffix {
                    keys.retain(|(key, _)| key.ends_with(suffix));
                }
                total_deleted += keys.len();
                self.tasks.push((bucket.to_string(), keys));
            }
            if response.output.is_truncated() == Some(true) {
                next_marker = response.output.next_key_marker().map(str::to_string);
                next_version_id_marker =
                    response.output.next_version_id_marker().map(str::to_string);
                println!(
                    "Next : {}, {}",
                    next_marker.as_deref().unwrap_or_default(),
                    next_version_id_marker.as_deref().unwrap_or_default()
                );
            } else {
                break;
            }
        }
        total_deleted
    }

    /// 원본 `CurrentObjectClear`
    async fn current_object_clear(&mut self, bucket: &str) -> usize {
        let mut next_marker = String::new();
        let mut total_deleted = 0;
        loop {
            let response = match self
                .client
                .list_objects(bucket, None, Some(&next_marker), 1000, None)
                .await
            {
                Ok(response) => response,
                Err(e) => {
                    error!("{}", ScenarioError::s3(e, &["NoSuchBucket"]));
                    return total_deleted;
                }
            };
            let contents = response.output.contents();
            if contents.is_empty() {
                // `response.S3Objects`가 `null`
                error!("{}", null_reference());
                return total_deleted;
            }
            let keys: Keys = contents
                .iter()
                .map(|o| (o.key().unwrap_or_default().to_string(), None))
                .collect();
            total_deleted += keys.len();
            self.tasks.push((bucket.to_string(), keys));
            if response.output.is_truncated() == Some(true) {
                // .NET SDK는 `NextMarker`가 없으면 마지막 키를 쓴다.
                next_marker = response
                    .output
                    .next_marker()
                    .or_else(|| contents.last().and_then(|o| o.key()))
                    .unwrap_or_default()
                    .to_string();
                println!("Next : {next_marker}");
            } else {
                break;
            }
        }
        total_deleted
    }

    /// 원본 `NoncurrentObjectClear`(`markers == false`)와 `MarkerObjectClear`(`markers == true`)
    async fn versions_clear(&mut self, bucket: &str, markers: bool) -> usize {
        let mut next_key_marker = Some(String::new());
        let mut next_version_id_marker = Some(String::new());
        let mut total_deleted = 0;
        loop {
            let response = match self
                .client
                .list_versions(
                    bucket,
                    None,
                    next_key_marker.as_deref(),
                    next_version_id_marker.as_deref(),
                    1000,
                    None,
                )
                .await
            {
                Ok(response) => response,
                Err(e) => {
                    error!("{}", ScenarioError::s3(e, &[]));
                    return total_deleted;
                }
            };
            if version_keys(&response.output, |_| true).is_empty() {
                break;
            }
            let keys = version_keys(&response.output, |is_marker| is_marker == markers);
            total_deleted += keys.len();
            self.tasks.push((bucket.to_string(), keys));
            if response.output.is_truncated() == Some(true) {
                next_key_marker = response.output.next_key_marker().map(str::to_string);
                next_version_id_marker =
                    response.output.next_version_id_marker().map(str::to_string);
            } else {
                break;
            }
        }
        total_deleted
    }

    /// 원본 `WaitTasks`
    async fn wait_tasks(&mut self, _bucket: &str) {
        info!("WaitTasks({})", self.tasks.len());
        let mut set = JoinSet::new();
        for (bucket, keys) in self.tasks.drain(..) {
            let client = self.client.clone();
            set.spawn(async move { delete_objects(&client, &bucket, keys, None).await });
        }
        while set.join_next().await.is_some() {}
        info!("Finish");
    }
}

/// 원본 `Versions`(버전과 삭제 마커를 문서 순서로 담은 목록)를 `keep(is_marker)`로 거른 키·버전 ID.
fn version_keys(
    output: &testrust_s3::s3_client::ListVersions,
    keep: impl Fn(bool) -> bool,
) -> Keys {
    output
        .entries()
        .unwrap_or_default()
        .iter()
        .filter(|entry| keep(entry.is_delete_marker()))
        .map(|entry| {
            (
                entry.key().unwrap_or_default().to_string(),
                entry.version_id().map(str::to_string),
            )
        })
        .collect()
}

/// 원본 `Clear(bucketName, maxKeys, isDelete)`: 지운 개수를 돌려준다.
async fn clear(client: &S3Client, bucket: &str, max_keys: i32, is_delete: bool) -> usize {
    let mut total_deleted = 0;
    if let Err(e) = clear_inner(client, bucket, max_keys, is_delete, &mut total_deleted).await {
        error!("{e}");
    }
    total_deleted
}

async fn clear_inner(
    client: &S3Client,
    bucket: &str,
    max_keys: i32,
    is_delete: bool,
    total_deleted: &mut usize,
) -> Result<(), ScenarioError> {
    check_and_grant_permissions(client, bucket).await;
    let bypass = get_object_lock(client, bucket).await;
    if bypass {
        info!("Object Lock 활성화됨: {bucket}");
    }
    let mut next_marker: Option<String> = None;
    let mut next_version_id_marker: Option<String> = None;
    let mut tasks = JoinSet::new();
    loop {
        let response = client
            .list_versions(
                bucket,
                None,
                next_marker.as_deref(),
                next_version_id_marker.as_deref(),
                max_keys,
                None,
            )
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        let keys = version_keys(&response.output, |_| true);
        if keys.is_empty() {
            break;
        }
        *total_deleted += keys.len();
        {
            let client = client.clone();
            let bucket = bucket.to_string();
            tasks.spawn(async move { delete_objects(&client, &bucket, keys, Some(bypass)).await });
        }
        if response.output.is_truncated() == Some(true) {
            next_marker = response.output.next_key_marker().map(str::to_string);
            next_version_id_marker = response.output.next_version_id_marker().map(str::to_string);
        } else {
            break;
        }
    }
    while tasks.join_next().await.is_some() {}

    if is_delete {
        delete_bucket(client, bucket).await;
    }
    info!("AllObjectClear({bucket}) success");
    Ok(())
}

/// 원본 `DeleteBucket`
async fn delete_bucket(client: &S3Client, bucket: &str) {
    match client.delete_bucket(bucket).await {
        Ok(_) => info!("DeleteBucket({bucket}) success"),
        Err(e) => error!("{}", ScenarioError::s3(e, &[])),
    }
}

/// 원본 `DeleteObjects`
async fn delete_objects(client: &S3Client, bucket: &str, keys: Keys, bypass: Option<bool>) {
    // SDK가 본문 없는 2xx(`NoContent` 등)를 오류로 돌려주면 .NET처럼 상태만 다른 응답으로 본다.
    let result = match client.delete_objects(bucket, &keys, bypass, None).await {
        Ok(response) => Ok(response.status),
        Err(S3Error::Service { status, .. }) if (200..300).contains(&status) => Ok(status),
        Err(e) => Err(e),
    };
    match result {
        Ok(status) => {
            if status != 200 {
                error!("StatusCode : {}", status_name(status));
            }
            info!("DeleteObjects({bucket}, {}) success", keys.len());
        }
        Err(e) => error!("{}", ScenarioError::s3(e, &[])),
    }
}

/// 원본 `CheckAndGrantPermissions`
async fn check_and_grant_permissions(client: &S3Client, bucket: &str) {
    info!("권한 확인 중: {bucket}");
    let result: Result<(), ScenarioError> = async {
        let acl = client
            .get_bucket_acl(bucket)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        if has_delete_permission(&acl.output) {
            info!("삭제 권한이 확인되었습니다: {bucket}");
        } else {
            info!("삭제 권한이 없습니다. 권한을 부여합니다: {bucket}");
            grant_delete_permission(client, bucket).await?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        warn!("권한 확인 중 오류 발생: {bucket}, {}", e.message);
    }
}

fn owner_has_full_control(grants: &[Grant], owner_id: Option<&str>) -> bool {
    grants.iter().any(|grant| {
        grant.grantee().and_then(|g| g.id()) == owner_id
            && grant.permission() == Some(&Permission::FullControl)
    })
}

/// 원본 `CheckDeletePermission`
fn has_delete_permission(acl: &aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput) -> bool {
    let Some(owner) = acl.owner() else {
        warn!("버킷 ACL 정보가 불완전합니다.");
        return false;
    };
    owner_has_full_control(acl.grants(), owner.id())
}

/// 원본 `GrantDeletePermission`
async fn grant_delete_permission(client: &S3Client, bucket: &str) -> Result<(), ScenarioError> {
    let result: Result<(), ScenarioError> = async {
        let response = client
            .get_bucket_acl(bucket)
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        let acl = response.output;
        let Some(owner) = acl.owner() else {
            error!("버킷 소유자 정보를 가져올 수 없습니다: {bucket}");
            return Ok(());
        };
        let mut grants = acl.grants().to_vec();
        if owner_has_full_control(&grants, owner.id()) {
            info!("이미 FULL_CONTROL 권한이 있습니다: {bucket}");
            return Ok(());
        }
        let build = |e: aws_sdk_s3::error::BuildError| {
            ScenarioError::new("Amazon.Runtime.AmazonClientException", e.to_string())
        };
        grants.push(
            Grant::builder()
                .grantee(
                    Grantee::builder()
                        .r#type(Type::CanonicalUser)
                        .set_id(owner.id().map(str::to_string))
                        .set_display_name(owner.display_name().map(str::to_string))
                        .build()
                        .map_err(build)?,
                )
                .permission(Permission::FullControl)
                .build(),
        );
        let policy = AccessControlPolicy::builder()
            .set_grants(Some(grants))
            .owner(owner.clone())
            .build();
        client
            .put_bucket_acl(bucket, None, Some(policy))
            .await
            .map_err(|e| ScenarioError::s3(e, &[]))?;
        info!("Owner에게 삭제 권한이 성공적으로 부여되었습니다: {bucket}");
        Ok(())
    }
    .await;
    if let Err(e) = &result {
        error!("권한 부여 중 오류 발생: {bucket}, {}", e.message);
    }
    result
}

/// 원본 `GetObjectLock`
async fn get_object_lock(client: &S3Client, bucket: &str) -> bool {
    match client.get_object_lock_configuration(bucket).await {
        Ok(response) => {
            response
                .output
                .object_lock_configuration()
                .and_then(|c| c.object_lock_enabled())
                == Some(&ObjectLockEnabled::Enabled)
        }
        // 설정이 없을 경우
        Err(S3Error::Service { .. }) => false,
        Err(e) => {
            let e: ScenarioError = e.into();
            error!("Object Lock 확인 중 오류 발생: {bucket}\n{e}");
            false
        }
    }
}
