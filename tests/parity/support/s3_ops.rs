//! `tools/dotnet-oracle`(`Program.cs`, `S3Ops.cs`)의 `S3Probe`와 같은 op 이름·같은 설정 객체로 Rust `S3Client`를 호출한다.
//! 설정 객체(수명 주기 규칙, CORS 규칙, 태그, 복제 설정 등)는 오라클의 값과 같게 유지해야 한다.

use aws_sdk_s3::primitives::DateTime;
use aws_sdk_s3::primitives::DateTimeFormat;
use aws_sdk_s3::types::{
    AbortIncompleteMultipartUpload, AccessControlPolicy, AnalyticsConfiguration,
    AnalyticsExportDestination, AnalyticsFilter, AnalyticsS3BucketDestination,
    AnalyticsS3ExportFileFormat, BucketCannedAcl, BucketLifecycleConfiguration,
    BucketLoggingStatus, BucketVersioningStatus, ChecksumAlgorithm, Condition, CorsConfiguration,
    CorsRule, DefaultRetention, DeleteMarkerReplication, DeleteMarkerReplicationStatus,
    Destination, ErrorDocument, Event, ExpirationStatus, Grant, Grantee, IndexDocument,
    InventoryConfiguration, InventoryDestination, InventoryFilter, InventoryFormat,
    InventoryFrequency, InventoryIncludedObjectVersions, InventoryS3BucketDestination,
    InventorySchedule, LambdaFunctionConfiguration, LifecycleExpiration, LifecycleRule,
    LifecycleRuleFilter, LoggingEnabled, MetricsConfiguration, MetricsFilter,
    NoncurrentVersionExpiration, ObjectCannedAcl, ObjectLockConfiguration, ObjectLockEnabled,
    ObjectLockLegalHold, ObjectLockLegalHoldStatus, ObjectLockRetention, ObjectLockRetentionMode,
    ObjectLockRule, ObjectOwnership, Owner, Permission, PublicAccessBlockConfiguration,
    QueueConfiguration, Redirect, ReplicationConfiguration, ReplicationRule, ReplicationRuleFilter,
    ReplicationRuleStatus, RoutingRule, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule, StorageClass,
    StorageClassAnalysis, StorageClassAnalysisDataExport, StorageClassAnalysisSchemaVersion, Tag,
    Tagging, TopicConfiguration, Transition, TransitionStorageClass, Type, WebsiteConfiguration,
};
use serde_json::Value;
use testrust_s3::S3Client;
use testrust_s3::S3Error;
use testrust_s3::s3_client::{PartETag, PutBody, S3_MAX_KEYS};

/// 성공 결과: HTTP 상태 코드와, 상태 코드만으로 비교할 수 없는 값(내려받은 내용 등).
#[derive(Debug, Default)]
pub struct OpOk {
    pub status: Option<u16>,
    pub detail: Option<String>,
}

pub type OpResult = Result<OpOk, S3Error>;

fn status<T>(result: Result<testrust_s3::S3Response<T>, S3Error>) -> OpResult {
    result.map(|r| OpOk {
        status: Some(r.status),
        detail: None,
    })
}

/// 사례 JSON의 입력.
pub struct Spec {
    pub bucket: String,
    pub key: String,
    pub body: String,
    pub chunked: bool,
    pub ext: String,
    pub file_size: usize,
    pub part_size: i64,
    pub thread_count: usize,
}

impl Spec {
    pub fn new(spec: &Value) -> Self {
        let text = |key: &str, default: &str| {
            spec.get(key)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        Self {
            bucket: text("bucket", "my-bucket"),
            key: text("key", "dir/key.txt"),
            body: text("body", "hello world"),
            chunked: spec.get("chunked").and_then(Value::as_bool).unwrap_or(true),
            ext: text("ext", "bin"),
            file_size: spec.get("fileSize").and_then(Value::as_u64).unwrap_or(0) as usize,
            part_size: spec
                .get("partSize")
                .and_then(Value::as_i64)
                .unwrap_or(5 * 1024 * 1024),
            thread_count: spec
                .get("threadCount")
                .and_then(Value::as_u64)
                .unwrap_or(10) as usize,
        }
    }
}

/// 파일 본문 규칙: i번째 바이트 = 'a' + i % 26(오라클과 같다).
pub fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|i| b'a' + (i % 26) as u8).collect()
}

fn tags() -> Vec<Tag> {
    vec![
        Tag::builder()
            .key("project")
            .value("alpha")
            .build()
            .unwrap(),
        Tag::builder()
            .key("env")
            .value("test & dev")
            .build()
            .unwrap(),
    ]
}

fn retain_until() -> DateTime {
    DateTime::from_str("2030-01-02T03:04:05Z", DateTimeFormat::DateTime).unwrap()
}

fn owner() -> Owner {
    Owner::builder()
        .id("owner-id")
        .display_name("owner")
        .build()
}

fn acl_user_policy() -> AccessControlPolicy {
    AccessControlPolicy::builder()
        .owner(owner())
        .grants(
            Grant::builder()
                .grantee(
                    Grantee::builder()
                        .r#type(Type::CanonicalUser)
                        .id("user-id")
                        .display_name("user")
                        .build()
                        .unwrap(),
                )
                .permission(Permission::FullControl)
                .build(),
        )
        .build()
}

fn acl_group_policy() -> AccessControlPolicy {
    AccessControlPolicy::builder()
        .owner(owner())
        .grants(
            Grant::builder()
                .grantee(
                    Grantee::builder()
                        .r#type(Type::Group)
                        .uri("http://acs.amazonaws.com/groups/global/AllUsers")
                        .build()
                        .unwrap(),
                )
                .permission(Permission::Read)
                .build(),
        )
        .build()
}

/// 오라클 `S3Ops`와 같은 op를 실행한다.
pub async fn run_op(op: &str, c: &S3Client, s: &Spec) -> OpResult {
    let b = s.bucket.as_str();
    let k = s.key.as_str();
    match op {
        // ---- 기존 op ----
        "list-buckets" => status(c.list_buckets(None, 10000, None).await),
        "put-bucket" => status(c.put_bucket(b, None, None, None).await),
        "head-bucket-exists" => Ok(OpOk {
            status: Some(u16::from(c.does_s3_bucket_exist(b).await)),
            detail: None,
        }),
        "put-object" => status(
            c.put_object(b, k, PutBody::Text(s.body.clone()), s.chunked, None)
                .await,
        ),
        "put-object-checksum" => status(
            c.put_object_with_checksum(
                b,
                k,
                PutBody::Text(s.body.clone()),
                s.chunked,
                ChecksumAlgorithm::Crc32,
            )
            .await,
        ),
        "get-object" => status(c.get_object(b, k, None, None).await),
        "get-object-range" => status(c.get_object(b, k, None, Some((0, 4))).await),
        "head-object" => status(c.head_object(b, k, None, None).await),
        "list-objects-v2" => status(
            c.list_objects_v2(b, Some("dir/"), None, S3_MAX_KEYS, Some("/"), None)
                .await,
        ),
        "list-objects" => status(
            c.list_objects(b, Some("dir/"), None, S3_MAX_KEYS, None)
                .await,
        ),
        "delete-object" => status(c.delete_object(b, k, None, None).await),
        "delete-objects" => status(
            c.delete_objects(
                b,
                &[("a".into(), None), ("b".into(), Some("v1".into()))],
                None,
                Some(true),
            )
            .await,
        ),
        "upload-part" => status(
            c.upload_part(
                b,
                k,
                "upload-1",
                1,
                PutBody::Bytes(s.body.clone().into_bytes()),
                0,
                -1,
                s.chunked,
            )
            .await,
        ),
        "put-bucket-versioning" => status(
            c.put_bucket_versioning(b, Some(BucketVersioningStatus::Enabled))
                .await,
        ),

        // ---- 버킷 ----
        "put-bucket-acl-canned" => status(
            c.put_bucket_acl(b, Some(BucketCannedAcl::PublicRead), None)
                .await,
        ),
        "put-bucket-acl-policy" => status(c.put_bucket_acl(b, None, Some(acl_user_policy())).await),
        "get-bucket-acl" => status(c.get_bucket_acl(b).await),
        "put-object-acl-canned" => status(
            c.put_object_acl(b, k, Some(ObjectCannedAcl::Private), None)
                .await,
        ),
        "put-object-acl-policy" => {
            status(c.put_object_acl(b, k, None, Some(acl_group_policy())).await)
        }
        "get-object-acl" => status(c.get_object_acl(b, k, Some("v1")).await),
        "list-directory-buckets" => status(c.list_directory_buckets(10, Some("token-1")).await),
        "delete-bucket" => status(c.delete_bucket(b).await),
        "get-bucket-ownership-controls" => status(c.get_bucket_ownership_controls(b).await),
        "put-bucket-ownership-controls" => status(
            c.put_bucket_ownership_controls(b, ObjectOwnership::BucketOwnerEnforced)
                .await,
        ),
        "delete-bucket-ownership-controls" => status(c.delete_bucket_ownership_controls(b).await),
        "get-bucket-location" => status(c.get_bucket_location(b).await),
        "put-bucket-logging" => status(
            c.put_bucket_logging(
                b,
                BucketLoggingStatus::builder()
                    .logging_enabled(
                        LoggingEnabled::builder()
                            .target_bucket("log-bucket")
                            .target_prefix("logs/")
                            .build()
                            .unwrap(),
                    )
                    .build(),
            )
            .await,
        ),
        "get-bucket-logging" => status(c.get_bucket_logging(b).await),
        "put-bucket-notification" => status(
            c.put_bucket_notification(
                b,
                Some(vec![
                    TopicConfiguration::builder()
                        .id("t1")
                        .topic_arn("arn:aws:sns:us-east-1:123456789012:topic")
                        .events(Event::S3ObjectCreated)
                        .build()
                        .unwrap(),
                ]),
                Some(vec![
                    QueueConfiguration::builder()
                        .id("q1")
                        .queue_arn("arn:aws:sqs:us-east-1:123456789012:queue")
                        .events(Event::S3ObjectRemoved)
                        .build()
                        .unwrap(),
                ]),
                Some(vec![
                    LambdaFunctionConfiguration::builder()
                        .id("l1")
                        .lambda_function_arn("arn:aws:lambda:us-east-1:123456789012:function:f")
                        .events(Event::S3ObjectCreatedPut)
                        .build()
                        .unwrap(),
                ]),
            )
            .await,
        ),
        "get-bucket-notification" => status(c.get_bucket_notification(b).await),
        "put-bucket-versioning-suspended" => status(
            c.put_bucket_versioning(b, Some(BucketVersioningStatus::Suspended))
                .await,
        ),
        "get-bucket-versioning" => status(c.get_bucket_versioning(b).await),
        "put-cors" => status(
            c.put_cors(
                b,
                CorsConfiguration::builder()
                    .cors_rules(
                        CorsRule::builder()
                            .id("rule1")
                            .allowed_methods("GET")
                            .allowed_methods("PUT")
                            .allowed_origins("https://example.com")
                            .allowed_headers("*")
                            .expose_headers("ETag")
                            .max_age_seconds(3000)
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "get-cors" => status(c.get_cors(b).await),
        "delete-cors" => status(c.delete_cors(b).await),
        "get-bucket-tagging" => status(c.get_bucket_tagging(b).await),
        "put-bucket-tagging" => status(c.put_bucket_tagging(b, tags()).await),
        "delete-bucket-tagging" => status(c.delete_bucket_tagging(b).await),
        "put-lifecycle" => status(
            c.put_lifecycle_configuration(
                b,
                BucketLifecycleConfiguration::builder()
                    .rules(
                        LifecycleRule::builder()
                            .id("expire-logs")
                            .status(ExpirationStatus::Enabled)
                            .filter(LifecycleRuleFilter::builder().prefix("logs/").build())
                            .expiration(LifecycleExpiration::builder().days(30).build())
                            .transitions(
                                Transition::builder()
                                    .days(10)
                                    .storage_class(TransitionStorageClass::StandardIa)
                                    .build(),
                            )
                            .noncurrent_version_expiration(
                                NoncurrentVersionExpiration::builder()
                                    .noncurrent_days(7)
                                    .build(),
                            )
                            .abort_incomplete_multipart_upload(
                                AbortIncompleteMultipartUpload::builder()
                                    .days_after_initiation(3)
                                    .build(),
                            )
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "get-lifecycle" => status(c.get_lifecycle_configuration(b).await),
        "delete-lifecycle" => status(c.delete_lifecycle(b).await),
        "put-bucket-policy" => status(
            c.put_bucket_policy(b, "{\"Version\":\"2012-10-17\",\"Statement\":[]}")
                .await,
        ),
        "get-bucket-policy" => status(c.get_bucket_policy(b).await),
        "delete-bucket-policy" => status(c.delete_bucket_policy(b).await),
        "get-bucket-policy-status" => status(c.get_bucket_policy_status(b).await),
        "put-object-lock-configuration" => status(
            c.put_object_lock_configuration(
                b,
                ObjectLockConfiguration::builder()
                    .object_lock_enabled(ObjectLockEnabled::Enabled)
                    .rule(
                        ObjectLockRule::builder()
                            .default_retention(
                                DefaultRetention::builder()
                                    .mode(ObjectLockRetentionMode::Governance)
                                    .days(1)
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .await,
        ),
        "get-object-lock-configuration" => status(c.get_object_lock_configuration(b).await),
        "put-public-access-block" => status(
            c.put_public_access_block(
                b,
                PublicAccessBlockConfiguration::builder()
                    .block_public_acls(true)
                    .ignore_public_acls(true)
                    .block_public_policy(false)
                    .restrict_public_buckets(false)
                    .build(),
            )
            .await,
        ),
        "get-public-access-block" => status(c.get_public_access_block(b).await),
        "delete-public-access-block" => status(c.delete_public_access_block(b).await),
        "get-bucket-encryption" => status(c.get_bucket_encryption(b).await),
        "put-bucket-encryption" => status(
            c.put_bucket_encryption(
                b,
                ServerSideEncryptionConfiguration::builder()
                    .rules(
                        ServerSideEncryptionRule::builder()
                            .apply_server_side_encryption_by_default(
                                ServerSideEncryptionByDefault::builder()
                                    .sse_algorithm(ServerSideEncryption::Aes256)
                                    .build()
                                    .unwrap(),
                            )
                            .bucket_key_enabled(true)
                            .build(),
                    )
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "delete-bucket-encryption" => status(c.delete_bucket_encryption(b).await),
        "get-bucket-website" => status(c.get_bucket_website(b).await),
        "put-bucket-website" => status(
            c.put_bucket_website(
                b,
                WebsiteConfiguration::builder()
                    .index_document(
                        IndexDocument::builder()
                            .suffix("index.html")
                            .build()
                            .unwrap(),
                    )
                    .error_document(ErrorDocument::builder().key("error.html").build().unwrap())
                    .routing_rules(
                        RoutingRule::builder()
                            .condition(Condition::builder().key_prefix_equals("docs/").build())
                            .redirect(
                                Redirect::builder()
                                    .replace_key_prefix_with("documents/")
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .await,
        ),
        "delete-bucket-website" => status(c.delete_bucket_website(b).await),
        "get-bucket-inventory" => status(c.get_bucket_inventory(b, "inv1").await),
        "list-bucket-inventory" => status(c.list_bucket_inventory(b).await),
        "put-bucket-inventory" => status(
            c.put_bucket_inventory(
                b,
                InventoryConfiguration::builder()
                    .id("inv1")
                    .is_enabled(true)
                    .destination(
                        InventoryDestination::builder()
                            .s3_bucket_destination(
                                InventoryS3BucketDestination::builder()
                                    .bucket("arn:aws:s3:::dest-bucket")
                                    .prefix("inv/")
                                    .format(InventoryFormat::Csv)
                                    .build()
                                    .unwrap(),
                            )
                            .build(),
                    )
                    .schedule(
                        InventorySchedule::builder()
                            .frequency(InventoryFrequency::Daily)
                            .build()
                            .unwrap(),
                    )
                    .included_object_versions(InventoryIncludedObjectVersions::All)
                    .filter(InventoryFilter::builder().prefix("data/").build().unwrap())
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "delete-bucket-inventory" => status(c.delete_bucket_inventory(b, "inv1").await),
        "get-bucket-metrics" => status(c.get_bucket_metrics(b, "m1").await),
        "list-bucket-metrics" => status(c.list_bucket_metrics(b).await),
        "put-bucket-metrics" => status(
            c.put_bucket_metrics(
                b,
                MetricsConfiguration::builder()
                    .id("m1")
                    .filter(MetricsFilter::Prefix("data/".into()))
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "delete-bucket-metrics" => status(c.delete_bucket_metrics(b, "m1").await),
        "get-bucket-analytics" => status(c.get_bucket_analytics(b, "a1").await),
        "list-bucket-analytics" => status(c.list_bucket_analytics(b).await),
        "put-bucket-analytics" => status(
            c.put_bucket_analytics(
                b,
                AnalyticsConfiguration::builder()
                    .id("a1")
                    .filter(AnalyticsFilter::Prefix("data/".into()))
                    .storage_class_analysis(
                        StorageClassAnalysis::builder()
                            .data_export(
                                StorageClassAnalysisDataExport::builder()
                                    .output_schema_version(StorageClassAnalysisSchemaVersion::V1)
                                    .destination(
                                        AnalyticsExportDestination::builder()
                                            .s3_bucket_destination(
                                                AnalyticsS3BucketDestination::builder()
                                                    .bucket("arn:aws:s3:::dest-bucket")
                                                    .prefix("analytics/")
                                                    .format(AnalyticsS3ExportFileFormat::Csv)
                                                    .build()
                                                    .unwrap(),
                                            )
                                            .build(),
                                    )
                                    .build()
                                    .unwrap(),
                            )
                            .build(),
                    )
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "delete-bucket-analytics" => status(c.delete_bucket_analytics(b, "a1").await),

        // ---- 객체 ----
        "put-object-tagging-header" => status(
            c.put_object(
                b,
                k,
                PutBody::Text("hello world".into()),
                true,
                Some(tags()),
            )
            .await,
        ),
        "put-object-file" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("s3probe-test.{}", s.ext));
            std::fs::write(
                &path,
                pattern(if s.file_size > 0 { s.file_size } else { 32 }),
            )
            .unwrap();
            status(
                c.put_object(b, k, PutBody::File(path), s.chunked, None)
                    .await,
            )
        }
        "put-object-stream" => status(
            c.put_object(
                b,
                k,
                PutBody::Bytes(s.body.clone().into_bytes()),
                s.chunked,
                None,
            )
            .await,
        ),
        "copy-object" => status(
            c.copy_object("src-bucket", "src/key 1.txt", b, k, Some("v1"))
                .await,
        ),
        "copy-object-special" => status(
            c.copy_object(
                "src-bucket",
                "src/한글 a+b(1)*'!~_-.txt",
                b,
                "dst/한글 key.txt",
                Some("v 1/+"),
            )
            .await,
        ),
        "list-versions" => status(
            c.list_versions(b, Some("dir/"), Some("km"), Some("vm"), 50, Some("/"))
                .await,
        ),
        "get-object-tagging" => status(c.get_object_tagging(b, k, Some("v1")).await),
        "put-object-tagging" => status(
            c.put_object_tagging(
                b,
                k,
                Tagging::builder()
                    .set_tag_set(Some(tags()))
                    .build()
                    .unwrap(),
            )
            .await,
        ),
        "delete-object-tagging" => status(c.delete_object_tagging(b, k).await),
        "get-object-retention" => status(c.get_object_retention(b, k, Some("v1")).await),
        "put-object-retention" => status(
            c.put_object_retention(
                b,
                k,
                ObjectLockRetention::builder()
                    .mode(ObjectLockRetentionMode::Governance)
                    .retain_until_date(retain_until())
                    .build(),
                None,
                Some("v1"),
                Some(true),
            )
            .await,
        ),
        "put-object-retention-nobypass" => status(
            c.put_object_retention(
                b,
                k,
                ObjectLockRetention::builder()
                    .mode(ObjectLockRetentionMode::Compliance)
                    .retain_until_date(retain_until())
                    .build(),
                None,
                None,
                Some(false),
            )
            .await,
        ),
        "put-object-legal-hold" => status(
            c.put_object_legal_hold(
                b,
                k,
                ObjectLockLegalHold::builder()
                    .status(ObjectLockLegalHoldStatus::On)
                    .build(),
                Some("v1"),
            )
            .await,
        ),
        "get-object-legal-hold" => status(c.get_object_legal_hold(b, k, Some("v1")).await),
        "get-bucket-replication" => status(c.get_bucket_replication(b).await),
        "put-bucket-replication" => status(
            c.put_bucket_replication(
                b,
                ReplicationConfiguration::builder()
                    .role("arn:aws:iam::123456789012:role/replication")
                    .rules(
                        ReplicationRule::builder()
                            .id("rule1")
                            .status(ReplicationRuleStatus::Enabled)
                            .priority(1)
                            .filter(ReplicationRuleFilter::builder().prefix("docs/").build())
                            .delete_marker_replication(
                                DeleteMarkerReplication::builder()
                                    .status(DeleteMarkerReplicationStatus::Disabled)
                                    .build(),
                            )
                            .destination(
                                Destination::builder()
                                    .bucket("arn:aws:s3:::dest-bucket")
                                    .storage_class(StorageClass::StandardIa)
                                    .build()
                                    .unwrap(),
                            )
                            .build()
                            .unwrap(),
                    )
                    .build()
                    .unwrap(),
                Some("token-ignored"),
            )
            .await,
        ),
        "delete-bucket-replication" => status(c.delete_bucket_replication(b).await),
        "restore-object" => status(c.restore_object(b, k, Some("v1"), 7).await),
        "restore-object-nodays" => status(c.restore_object(b, k, None, -1).await),

        // ---- 멀티파트 ----
        "initiate-multipart-upload" => status(c.initiate_multipart_upload(b, k).await),
        "upload-part-file" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s3probe-test.bin");
            std::fs::write(&path, pattern(32)).unwrap();
            status(
                c.upload_part(b, k, "upload-1", 2, PutBody::File(path), 4, 8, s.chunked)
                    .await,
            )
        }
        "copy-part" => status(
            c.copy_part(
                "src-bucket",
                "src/key.txt",
                b,
                k,
                "upload-1",
                3,
                0,
                1023,
                Some("v1"),
            )
            .await,
        ),
        "complete-multipart-upload" => status(
            c.complete_multipart_upload(
                b,
                k,
                "upload-1",
                &[
                    PartETag::new(1, Some("\"etag1\"")),
                    PartETag::new(2, Some("\"etag2\"")),
                ],
            )
            .await,
        ),
        "abort-multipart-upload" => status(c.abort_multipart_upload(b, k, "upload-1").await),
        "list-multipart-uploads" => status(
            c.list_multipart_uploads(b, Some("dir/"), Some("um"), Some("km"), 50, Some("/"))
                .await,
        ),
        "list-parts" => status(c.list_parts(b, k, "upload-1", 2, 50).await),
        "list-parts-nomarker" => status(c.list_parts(b, k, "upload-1", 0, S3_MAX_KEYS).await),

        // ---- TransferUtility ----
        "upload" | "upload-content-type" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("s3probe-test.{}", s.ext));
            std::fs::write(&path, pattern(s.file_size)).unwrap();
            let content_type = (op == "upload-content-type").then_some("text/csv");
            c.upload(
                b,
                k,
                Some(&path),
                s.part_size,
                s.thread_count,
                None,
                None,
                content_type,
            )
            .await
            .map(|()| OpOk {
                status: None,
                detail: Some("done".into()),
            })
        }
        "upload-bytes" => c
            .upload(
                b,
                k,
                None,
                s.part_size,
                s.thread_count,
                None,
                Some(pattern(s.file_size)),
                None,
            )
            .await
            .map(|()| OpOk {
                status: None,
                detail: Some("done".into()),
            }),
        "upload-bytes-and-file" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("s3probe-test.{}", s.ext));
            std::fs::write(&path, pattern(5)).unwrap();
            c.upload(
                b,
                k,
                Some(&path),
                s.part_size,
                s.thread_count,
                None,
                Some(pattern(s.file_size)),
                None,
            )
            .await
            .map(|()| OpOk {
                status: None,
                detail: Some("done".into()),
            })
        }
        "upload-stream" => c
            .upload(
                b,
                k,
                None,
                s.part_size,
                s.thread_count,
                Some(pattern(s.file_size)),
                None,
                (s.ext != "none").then_some("text/csv"),
            )
            .await
            .map(|()| OpOk {
                status: None,
                detail: Some("done".into()),
            }),
        "download" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s3probe.out");
            c.download(b, k, &path, Some("v1")).await?;
            Ok(OpOk {
                status: None,
                detail: Some(std::fs::read_to_string(&path).unwrap()),
            })
        }
        "download-existing" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s3probe.out");
            std::fs::write(&path, "OLDOLDOLDOLDOLDOLD").unwrap();
            c.download(b, k, &path, None).await?;
            Ok(OpOk {
                status: None,
                detail: Some(std::fs::read_to_string(&path).unwrap()),
            })
        }
        "download-newdir" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sub").join("x.out");
            c.download(b, k, &path, None).await?;
            Ok(OpOk {
                status: None,
                detail: Some(std::fs::read_to_string(&path).unwrap()),
            })
        }
        "download-error-existing" => {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s3probe.out");
            std::fs::write(&path, "OLDOLDOLDOLDOLDOLD").unwrap();
            let result = c.download(b, k, &path, None).await;
            Ok(OpOk {
                status: None,
                detail: Some(match result {
                    Ok(()) => "no error".into(),
                    Err(e) => format!(
                        "{}|{}|{}",
                        e.dotnet_type(),
                        e,
                        std::fs::read_to_string(&path).unwrap()
                    ),
                }),
            })
        }
        op => panic!("알 수 없는 op: {op}"),
    }
}
