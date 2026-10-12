//! 입력·출력 JSON 모델: `Data/S3/MyLifecycle*.cs`, `MyReplication*.cs`, `MyDeleteMarkerReplication.cs`와
//! 원본이 `JsonSerializer.Deserialize<T>`로 읽는 SDK 모델(`Tagging`, `PublicAccessBlockConfiguration`).
//!
//! 읽기는 `testrust_common::json`(System.Text.Json 규칙, `JsonException` 메시지 포함)을 쓴다.
//! 속성 이름은 대소문자를 구분하고 모르는 속성은 건너뛴다. `int?`·`bool?`·`DateTime?` 속성의 변환 오류는
//! .NET처럼 `System.Nullable`1[...]` 형식 이름으로 보고한다.

use aws_sdk_s3::primitives::DateTime;
use aws_sdk_s3::types::{
    AbortIncompleteMultipartUpload, DeleteMarkerReplication, DeleteMarkerReplicationStatus,
    Destination, ExpirationStatus, LifecycleExpiration, LifecycleRule, LifecycleRuleFilter,
    NoncurrentVersionExpiration, PublicAccessBlockConfiguration, ReplicationConfiguration,
    ReplicationRule, ReplicationRuleStatus, StorageClass, Tag,
};
use serde::Serialize;
use testrust_common::DotnetDateTime;
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};

use crate::dispatch::CommandError;

const NS: &str = "TestCore.Data.S3";

/// `T?`(`int?`, `bool?`, `DateTime?`) 속성을 읽는다. `null`은 `None`이고, 변환 오류의 형식 이름은
/// `System.Nullable`1[T]`다.
fn read_nullable_value<T: FromJson>(d: &mut Deserializer<'_>) -> Result<Option<T>, JsonError> {
    let tok = d.read_token()?;
    if tok == Token::Null {
        return Ok(None);
    }
    T::from_json(d, tok).map_err(|JsonError(message)| {
        let plain = format!("converted to {}.", T::type_name());
        let nullable = format!("converted to System.Nullable`1[{}].", T::type_name());
        JsonError(message.replacen(&plain, &nullable, 1))
    })
}

/// 값을 쓰지 않고 객체이기만 하면 되는 속성(`ReplicationRuleFilter`: 원본은 읽고 버린다).
struct IgnoredObject;

impl FromJson for IgnoredObject {
    fn type_name() -> String {
        "Amazon.S3.Model.ReplicationRuleFilter".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), (), |_, _, _| Ok(false))
            .map(|o| o.map(|()| Self))
    }
}

fn dotnet_date_to_smithy(date: &DotnetDateTime) -> Result<DateTime, CommandError> {
    let seconds = date
        .to_unix_seconds()
        .map_err(|message| CommandError::new("System.ArgumentOutOfRangeException", message))?;
    let nanos = date.value.and_utc().timestamp_subsec_nanos();
    Ok(DateTime::from_secs_and_nanos(seconds, nanos))
}

/// 값이 없으면 빈 문자열(수명주기 필터 `Prefix`처럼 .NET도 빈 요소를 보내는 곳).
fn blank(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

/// 필수 값이 빠지면 표식(`UNSET`)을 넣는다. S3 클라이언트가 서명 전에 그 요소를 지워 .NET처럼 생략한다.
fn required(value: &Option<String>) -> String {
    value
        .clone()
        .unwrap_or_else(|| testrust_s3::UNSET.to_string())
}

// ---------------------------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------------------------

/// `MyLifecycleConfiguration`
#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleConfiguration {
    #[serde(rename = "Rules")]
    pub rules: Option<Vec<Option<MyLifecycleRule>>>,
}

/// `MyLifecycleRule`
#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleRule {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "Status")]
    pub status: Option<String>,
    #[serde(rename = "Expiration")]
    pub expiration: Option<MyLifecycleRuleExpiration>,
    #[serde(rename = "NoncurrentVersionExpiration")]
    pub noncurrent_version_expiration: Option<MyLifecycleRuleNoncurrentVersionExpiration>,
    #[serde(rename = "Filter")]
    pub filter: Option<MyLifecycleFilter>,
    #[serde(rename = "AbortIncompleteMultipartUpload")]
    pub abort_incomplete_multipart_upload: Option<MyLifecycleRuleAbortIncompleteMultipartUpload>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleRuleExpiration {
    #[serde(rename = "Days")]
    pub days: Option<i32>,
    #[serde(rename = "Date")]
    pub date: Option<DotnetDateTime>,
    #[serde(rename = "ExpiredObjectDeleteMarker")]
    pub expired_object_delete_marker: Option<bool>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleRuleNoncurrentVersionExpiration {
    #[serde(rename = "NoncurrentDays")]
    pub noncurrent_days: Option<i32>,
    #[serde(rename = "NewerNoncurrentVersions")]
    pub newer_noncurrent_versions: Option<i32>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleFilter {
    #[serde(rename = "Prefix")]
    pub prefix: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct MyLifecycleRuleAbortIncompleteMultipartUpload {
    #[serde(rename = "DaysAfterInitiation")]
    pub days_after_initiation: i32,
}

impl FromJson for MyLifecycleConfiguration {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleConfiguration")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Rules" => {
                    let tok = d.read_token()?;
                    o.rules = d.read_list::<MyLifecycleRule>(tok)?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyLifecycleRule {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleRule")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Id" => o.id = d.read_nullable()?,
                "Status" => o.status = d.read_nullable()?,
                "Expiration" => o.expiration = d.read_nullable()?,
                "NoncurrentVersionExpiration" => {
                    o.noncurrent_version_expiration = d.read_nullable()?
                }
                "Filter" => o.filter = d.read_nullable()?,
                "AbortIncompleteMultipartUpload" => {
                    o.abort_incomplete_multipart_upload = d.read_nullable()?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyLifecycleRuleExpiration {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleRuleExpiration")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Days" => o.days = read_nullable_value(d)?,
                "Date" => o.date = read_nullable_value(d)?,
                "ExpiredObjectDeleteMarker" => {
                    o.expired_object_delete_marker = read_nullable_value(d)?
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyLifecycleRuleNoncurrentVersionExpiration {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleRuleNoncurrentVersionExpiration")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "NoncurrentDays" => o.noncurrent_days = read_nullable_value(d)?,
                "NewerNoncurrentVersions" => o.newer_noncurrent_versions = read_nullable_value(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyLifecycleFilter {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleFilter")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Prefix" => o.prefix = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyLifecycleRuleAbortIncompleteMultipartUpload {
    fn type_name() -> String {
        format!("{NS}.MyLifecycleRuleAbortIncompleteMultipartUpload")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "DaysAfterInitiation" => o.days_after_initiation = d.read_value()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyLifecycleConfiguration {
    /// `GetLifecycleConfiguration()`. 규칙이 `null`이면(`Rules: [null]`) 건너뛴다.
    pub fn to_sdk(&self) -> Result<Vec<LifecycleRule>, CommandError> {
        self.rules
            .iter()
            .flatten()
            .flatten()
            .map(MyLifecycleRule::to_sdk)
            .collect()
    }

    /// `FromLifecycleConfiguration(response.Configuration)`.
    pub fn from_sdk(rules: &[LifecycleRule]) -> Self {
        Self {
            rules: Some(
                rules
                    .iter()
                    .map(|r| Some(MyLifecycleRule::from_sdk(r)))
                    .collect(),
            ),
        }
    }
}

impl MyLifecycleRule {
    fn to_sdk(&self) -> Result<LifecycleRule, CommandError> {
        let mut rule = LifecycleRule::builder()
            .set_id(self.id.clone())
            // .NET은 `Status`가 없으면 `<Status>Disabled</Status>`를 보낸다(.NET 기준 출력으로 확인).
            .status(ExpirationStatus::from(
                self.status.as_deref().unwrap_or("Disabled"),
            ));
        if let Some(expiration) = &self.expiration {
            rule = rule.expiration(
                LifecycleExpiration::builder()
                    .set_days(expiration.days)
                    .set_date(
                        expiration
                            .date
                            .as_ref()
                            .map(dotnet_date_to_smithy)
                            .transpose()?,
                    )
                    .set_expired_object_delete_marker(expiration.expired_object_delete_marker)
                    .build(),
            );
        }
        if let Some(noncurrent) = &self.noncurrent_version_expiration {
            rule = rule.noncurrent_version_expiration(
                NoncurrentVersionExpiration::builder()
                    .set_noncurrent_days(noncurrent.noncurrent_days)
                    .set_newer_noncurrent_versions(noncurrent.newer_noncurrent_versions)
                    .build(),
            );
        }
        if let Some(filter) = &self.filter {
            // `LifecyclePrefixPredicate`: `Prefix`가 `null`이어도 빈 `<Prefix>`를 보낸다.
            rule = rule.filter(
                LifecycleRuleFilter::builder()
                    .prefix(blank(&filter.prefix))
                    .build(),
            );
        }
        if let Some(abort) = &self.abort_incomplete_multipart_upload {
            rule = rule.abort_incomplete_multipart_upload(
                AbortIncompleteMultipartUpload::builder()
                    .days_after_initiation(abort.days_after_initiation)
                    .build(),
            );
        }
        rule.build()
            .map_err(|e| CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string()))
    }

    fn from_sdk(rule: &LifecycleRule) -> Self {
        Self {
            id: rule.id().map(str::to_string),
            status: Some(rule.status().as_str().to_string()),
            expiration: rule.expiration().map(|e| MyLifecycleRuleExpiration {
                days: e.days(),
                date: e.date().and_then(smithy_to_dotnet_utc),
                expired_object_delete_marker: e.expired_object_delete_marker(),
            }),
            noncurrent_version_expiration: rule.noncurrent_version_expiration().map(|e| {
                MyLifecycleRuleNoncurrentVersionExpiration {
                    noncurrent_days: e.noncurrent_days(),
                    newer_noncurrent_versions: e.newer_noncurrent_versions(),
                }
            }),
            // 접두어가 아닌 조건(태그, AND, 크기)은 `Prefix`가 `null`이다.
            filter: rule.filter().map(|f| MyLifecycleFilter {
                prefix: f.prefix().map(str::to_string),
            }),
            abort_incomplete_multipart_upload: rule.abort_incomplete_multipart_upload().map(|a| {
                MyLifecycleRuleAbortIncompleteMultipartUpload {
                    days_after_initiation: a.days_after_initiation().unwrap_or(0),
                }
            }),
        }
    }
}

fn smithy_to_dotnet_utc(time: &DateTime) -> Option<DotnetDateTime> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(time.secs(), time.subsec_nanos())
        .map(DotnetDateTime::utc)
}

// ---------------------------------------------------------------------------------------------
// Replication
// ---------------------------------------------------------------------------------------------

/// `MyReplicationConfiguration`
#[derive(Debug, Default)]
pub(super) struct MyReplicationConfiguration {
    pub role: Option<String>,
    pub rules: Option<Vec<Option<MyReplicationRule>>>,
}

#[derive(Debug, Default)]
pub(super) struct MyReplicationRule {
    pub id: Option<String>,
    pub priority: i32,
    pub status: Option<String>,
    pub destination: Option<MyReplicationDestination>,
    pub delete_marker_replication: Option<MyDeleteMarkerReplication>,
}

#[derive(Debug, Default)]
pub(super) struct MyReplicationDestination {
    pub bucket: Option<String>,
    pub storage_class: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct MyDeleteMarkerReplication {
    pub status: Option<String>,
}

impl FromJson for MyReplicationConfiguration {
    fn type_name() -> String {
        format!("{NS}.MyReplicationConfiguration")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Role" => o.role = d.read_nullable()?,
                "Rules" => {
                    let tok = d.read_token()?;
                    o.rules = d.read_list::<MyReplicationRule>(tok)?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyReplicationRule {
    fn type_name() -> String {
        format!("{NS}.MyReplicationRule")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Id" => o.id = d.read_nullable()?,
                "Priority" => o.priority = d.read_value()?,
                // `ReplicationRuleFilter`는 읽기만 하고 SDK 규칙에는 넣지 않는다.
                "Filter" => {
                    d.read_nullable::<IgnoredObject>()?;
                }
                "Status" => o.status = d.read_nullable()?,
                "Destination" => o.destination = d.read_nullable()?,
                "DeleteMarkerReplication" => o.delete_marker_replication = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyReplicationDestination {
    fn type_name() -> String {
        format!("{NS}.MyReplicationDestination")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Bucket" => o.bucket = d.read_nullable()?,
                "StorageClass" => o.storage_class = d.read_nullable()?,
                "AccountId" => o.account_id = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for MyDeleteMarkerReplication {
    fn type_name() -> String {
        format!("{NS}.MyDeleteMarkerReplication")
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Status" => o.status = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyReplicationConfiguration {
    /// `GetReplicationConfiguration()`.
    pub fn to_sdk(&self) -> Result<ReplicationConfiguration, CommandError> {
        let rules = self
            .rules
            .iter()
            .flatten()
            .flatten()
            .map(MyReplicationRule::to_sdk)
            .collect::<Result<Vec<_>, _>>()?;
        ReplicationConfiguration::builder()
            .role(required(&self.role))
            .set_rules(Some(rules))
            .build()
            .map_err(|e| CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string()))
    }
}

impl MyReplicationRule {
    fn to_sdk(&self) -> Result<ReplicationRule, CommandError> {
        let client_error = |e: aws_sdk_s3::error::BuildError| {
            CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string())
        };
        // 원본 `new ReplicationRuleStatus(Status)`: `Status`가 없으면 ArgumentNullException.
        let Some(status) = self.status.as_deref() else {
            return Err(CommandError::new(
                "System.ArgumentNullException",
                "Value cannot be null. (Parameter 'key')",
            ));
        };
        let mut rule = ReplicationRule::builder()
            .set_id(self.id.clone())
            .priority(self.priority)
            .status(ReplicationRuleStatus::from(status));
        if let Some(destination) = &self.destination {
            rule = rule.destination(
                Destination::builder()
                    .bucket(required(&destination.bucket))
                    .set_storage_class(destination.storage_class.as_deref().map(StorageClass::from))
                    .set_account(destination.account_id.clone())
                    .build()
                    .map_err(client_error)?,
            );
        }
        if let Some(delete_marker) = &self.delete_marker_replication {
            rule = rule.delete_marker_replication(
                DeleteMarkerReplication::builder()
                    .set_status(
                        delete_marker
                            .status
                            .as_deref()
                            .map(DeleteMarkerReplicationStatus::from),
                    )
                    .build(),
            );
        }
        rule.build().map_err(client_error)
    }
}

// ---------------------------------------------------------------------------------------------
// Tagging, PublicAccessBlock (SDK 모델을 System.Text.Json으로 읽는다)
// ---------------------------------------------------------------------------------------------

/// `Amazon.S3.Model.Tagging`
#[derive(Debug, Default)]
pub(super) struct TaggingInput {
    pub tag_set: Option<Vec<Option<TagInput>>>,
}

/// `Amazon.S3.Model.Tag`
#[derive(Debug, Default)]
pub(super) struct TagInput {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl FromJson for TaggingInput {
    fn type_name() -> String {
        "Amazon.S3.Model.Tagging".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "TagSet" => {
                    let tok = d.read_token()?;
                    o.tag_set = d.read_list::<TagInput>(tok)?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl FromJson for TagInput {
    fn type_name() -> String {
        "Amazon.S3.Model.Tag".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Key" => o.key = d.read_nullable()?,
                "Value" => o.value = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl TaggingInput {
    /// `setting.TagSet`. 목록의 `null` 요소는 요청에서 빠진다. `Key`·`Value`가 없는 태그는 표식(`UNSET`)으로
    /// 만들어 요청에서 그 요소를 뺀다(원본과 같다).
    pub fn tag_set(&self) -> Result<Vec<Tag>, CommandError> {
        self.tag_set
            .iter()
            .flatten()
            .flatten()
            .map(|tag| {
                Tag::builder()
                    .key(required(&tag.key))
                    .value(required(&tag.value))
                    .build()
                    .map_err(|e| {
                        CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string())
                    })
            })
            .collect()
    }
}

/// `Amazon.S3.Model.PublicAccessBlockConfiguration`
#[derive(Debug, Default)]
pub(super) struct PublicAccessBlockInput {
    pub block_public_acls: Option<bool>,
    pub ignore_public_acls: Option<bool>,
    pub block_public_policy: Option<bool>,
    pub restrict_public_buckets: Option<bool>,
}

impl FromJson for PublicAccessBlockInput {
    fn type_name() -> String {
        "Amazon.S3.Model.PublicAccessBlockConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "BlockPublicAcls" => o.block_public_acls = read_nullable_value(d)?,
                "IgnorePublicAcls" => o.ignore_public_acls = read_nullable_value(d)?,
                "BlockPublicPolicy" => o.block_public_policy = read_nullable_value(d)?,
                "RestrictPublicBuckets" => o.restrict_public_buckets = read_nullable_value(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl PublicAccessBlockInput {
    pub fn to_sdk(&self) -> PublicAccessBlockConfiguration {
        PublicAccessBlockConfiguration::builder()
            .set_block_public_acls(self.block_public_acls)
            .set_ignore_public_acls(self.ignore_public_acls)
            .set_block_public_policy(self.block_public_policy)
            .set_restrict_public_buckets(self.restrict_public_buckets)
            .build()
    }
}
