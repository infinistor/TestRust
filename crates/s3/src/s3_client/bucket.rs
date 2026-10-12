//! 원본 `#region Bucket Function`, `#region S3Util`.

use aws_sdk_s3::operation::create_bucket::CreateBucketOutput;
use aws_sdk_s3::operation::get_bucket_versioning::GetBucketVersioningOutput;
use aws_sdk_s3::operation::list_buckets::ListBucketsOutput;
use aws_sdk_s3::operation::put_bucket_versioning::PutBucketVersioningOutput;
use aws_sdk_s3::types::{
    BucketCannedAcl, BucketVersioningStatus, ObjectOwnership, VersioningConfiguration,
};
use tracing::{error, info};

use super::{S3Client, S3Error, S3Response};

/// 빈 버킷 이름 대신 쓰는 자리표시 이름(실제 버킷 이름 규칙에 맞는 소문자·숫자·`-`).
const EMPTY_BUCKET: &str = "testrust-empty-bucket-placeholder";

/// 경로 방식 주소 `/자리표시...`에서 자리표시 이름을 지운다(`/?acl`).
fn strip_empty_bucket(request: &mut aws_sdk_s3::config::http::HttpRequest) {
    let uri = request.uri().to_string();
    let marker = format!("/{EMPTY_BUCKET}");
    if let Some(at) = uri.find(&marker) {
        let mut rest = &uri[at + marker.len()..];
        // `/자리표시/` 뒤의 `/`는 남기고, `/자리표시?`이면 `/`를 넣는다.
        if rest.starts_with('/') {
            rest = &rest[1..];
        }
        let _ = request.set_uri(format!("{}/{rest}", &uri[..at]));
    }
}

impl S3Client {
    /// 원본 `ListBuckets(string prefix = null, int maxBuckets = 10000, string continuationToken = null)`.
    pub async fn list_buckets(
        &self,
        prefix: Option<&str>,
        max_buckets: i32,
        continuation_token: Option<&str>,
    ) -> Result<S3Response<ListBucketsOutput>, S3Error> {
        send!(
            self.client
                .list_buckets()
                .set_prefix(prefix.map(str::to_string))
                .max_buckets(max_buckets)
                .set_continuation_token(continuation_token.map(str::to_string)),
            empty_body = "ListAllMyBucketsResult"
        )
    }

    /// 원본 `PutBucket(string bucketName, S3CannedACL acl = null, bool? objectLockEnabledForBucket = null, ObjectOwnership ownership = null)`.
    pub async fn put_bucket(
        &self,
        bucket_name: &str,
        acl: Option<BucketCannedAcl>,
        object_lock_enabled_for_bucket: Option<bool>,
        ownership: Option<ObjectOwnership>,
    ) -> Result<S3Response<CreateBucketOutput>, S3Error> {
        super::error::required(bucket_name, "BucketName", "PutBucketRequest")?;
        send!(
            self.client
                .create_bucket()
                .bucket(bucket_name)
                .set_acl(acl)
                .set_object_lock_enabled_for_bucket(object_lock_enabled_for_bucket)
                .set_object_ownership(ownership)
        )
    }

    /// 원본 `PutBucketVersioning(string bucketName, VersionStatus status = null)`.
    pub async fn put_bucket_versioning(
        &self,
        bucket_name: &str,
        status: Option<BucketVersioningStatus>,
    ) -> Result<S3Response<PutBucketVersioningOutput>, S3Error> {
        send!(
            self.client
                .put_bucket_versioning()
                .bucket(bucket_name)
                .versioning_configuration(
                    VersioningConfiguration::builder()
                        .set_status(status)
                        .build()
                )
        )
    }

    /// 원본 `GetBucketVersioning(string bucketName)`.
    pub async fn get_bucket_versioning(
        &self,
        bucket_name: &str,
    ) -> Result<S3Response<GetBucketVersioningOutput>, S3Error> {
        send!(self.client.get_bucket_versioning().bucket(bucket_name))
    }

    /// 원본 `DoesS3BucketExist`: 오류는 모두 `false`.
    ///
    /// 원본 `AmazonS3Util.DoesS3BucketExistV2Async`는 버킷 ACL 조회(`GET /{bucket}?acl`)로 확인하고,
    /// `NoSuchBucket`이면 `false`, 그 밖의 서비스 오류(예: 접근 거부)는 버킷이 있는 것으로 본다.
    ///
    /// 버킷 이름이 비어 있어도 .NET은 `GET /?acl`을 보낸다. Rust SDK는 빈 버킷으로 요청을 만들지 못하므로 경로 방식
    /// 주소에서는 자리표시 이름으로 만들고 서명 전에 경로에서 지운다(`EMPTY_BUCKET`).
    pub async fn does_s3_bucket_exist(&self, bucket_name: &str) -> bool {
        let empty = bucket_name.is_empty() && self.presign.endpoint.is_some();
        let request =
            self.client
                .get_bucket_acl()
                .bucket(if empty { EMPTY_BUCKET } else { bucket_name });
        let result = if empty {
            request
                .customize()
                .mutate_request(strip_empty_bucket)
                .send()
                .await
        } else {
            request.send().await
        };
        match result {
            Ok(_) => true,
            Err(e) => match S3Error::from(e) {
                S3Error::Service { code, .. } => code != "NoSuchBucket",
                _ => false,
            },
        }
    }

    /// 원본 `CreateBucket(string bucketName)`: 없을 때만 만들고 결과를 로그로 남긴다.
    pub async fn create_bucket(&self, bucket_name: &str) -> bool {
        if self.does_s3_bucket_exist(bucket_name).await {
            return false;
        }
        match self.put_bucket(bucket_name, None, None, None).await {
            Ok(response) if response.status == 200 => {
                info!("CreateBucket({bucket_name}) : Create!!");
                true
            }
            Ok(_) => {
                error!("CreateBucket({bucket_name}) : Create failed");
                false
            }
            // 원본의 `catch (AggregateException)`은 `GetAwaiter().GetResult()`가 내부 예외를 그대로 던지므로
            // 실행되지 않고, `catch (Exception e) { log.Error(e); }`로 간다.
            Err(e) => {
                error!("{}: {e}", e.dotnet_type());
                false
            }
        }
    }
}
