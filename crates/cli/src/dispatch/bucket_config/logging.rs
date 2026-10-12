//! SDK `S3BucketLoggingConfig`: `PutBucketLogging` 입력(SDK 모델을 그대로 읽는다).
//!
//! 원본 동작 그대로 둔 것
//! - `TargetBucketName`이 없으면(`null`) 로깅 설정(`LoggingEnabled`)을 보내지 않는다(다른 값이 있어도).
//!   그러면 로깅을 끄는 요청이다. `TargetPrefix`가 없으면 빈 문자열.
//! - `S3Grantee.Type`은 읽기 전용이라 JSON에서 읽지 않고, `CanonicalUser`·`EmailAddress`·`URI`를 `null`이
//!   아닌 값으로 설정한 마지막 것이 정한다.
//! - `S3Grant.Permission`은 `S3Permission`(생성자가 둘)이라 객체를 읽을 수 없다(`NotSupportedException`).
//! - 문자열 상수 `PartitionDateSource`는 `{ "Value": "..." }` 객체. 목록 요소가 `null`이면 건너뛴다.
//!
//! 파일 내용이 JSON `null`이면 `LoggingEnabled` 없는 요청을 보낸다(원본과 같다).

use aws_sdk_s3::types::{
    BucketLoggingStatus, Grantee, LoggingEnabled, PartitionDateSource as SdkPartitionDateSource,
    PartitionedPrefix, SimplePrefix, TargetGrant, TargetObjectKeyFormat, Type,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::jsonutil::{ConstantValue, PartitionDateSource, S3Permission, read_list};
use super::{CommandError, built};

/// SDK `S3Grantee`
#[derive(Debug, Default)]
struct GranteeInput {
    canonical_user: Option<String>,
    display_name: Option<String>,
    email_address: Option<String>,
    uri: Option<String>,
    kind: Option<&'static str>,
}

impl FromJson for GranteeInput {
    fn type_name() -> String {
        "Amazon.S3.Model.S3Grantee".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "CanonicalUser" => {
                    o.canonical_user = d.read_nullable()?;
                    if o.canonical_user.is_some() {
                        o.kind = Some("CanonicalUser");
                    }
                }
                "DisplayName" => o.display_name = d.read_nullable()?,
                "EmailAddress" => {
                    o.email_address = d.read_nullable()?;
                    if o.email_address.is_some() {
                        o.kind = Some("AmazonCustomerByEmail");
                    }
                }
                "URI" => {
                    o.uri = d.read_nullable()?;
                    if o.uri.is_some() {
                        o.kind = Some("Group");
                    }
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl GranteeInput {
    fn to_sdk(&self) -> Grantee {
        built(
            Grantee::builder()
                .set_display_name(self.display_name.clone())
                .set_email_address(self.email_address.clone())
                .set_id(self.canonical_user.clone())
                .set_uri(self.uri.clone())
                .r#type(Type::from(self.kind.unwrap_or(UNSET)))
                .build(),
        )
    }
}

/// SDK `S3Grant`
#[derive(Debug, Default)]
struct GrantInput {
    grantee: Option<GranteeInput>,
}

impl FromJson for GrantInput {
    fn type_name() -> String {
        "Amazon.S3.Model.S3Grant".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Grantee" => o.grantee = d.read_nullable()?,
                // `null`만 읽을 수 있다. 값이 있으면 읽다가 예외가 난다.
                "Permission" => {
                    d.read_nullable::<S3Permission>()?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `PartitionedPrefix`
#[derive(Debug, Default)]
struct PartitionedPrefixInput {
    partition_date_source: Option<ConstantValue>,
}

impl FromJson for PartitionedPrefixInput {
    fn type_name() -> String {
        "Amazon.S3.Model.PartitionedPrefix".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "PartitionDateSource" => {
                    o.partition_date_source = d
                        .read_nullable::<PartitionDateSource>()?
                        .map(|source| source.0);
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `SimplePrefix`(속성 없음)
#[derive(Debug, Default)]
struct SimplePrefixInput;

impl FromJson for SimplePrefixInput {
    fn type_name() -> String {
        "Amazon.S3.Model.SimplePrefix".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self, |_, _, _| Ok(false))
    }
}

/// SDK `TargetObjectKeyFormat`
#[derive(Debug, Default)]
struct KeyFormatInput {
    partitioned_prefix: Option<PartitionedPrefixInput>,
    simple_prefix: Option<SimplePrefixInput>,
}

impl FromJson for KeyFormatInput {
    fn type_name() -> String {
        "Amazon.S3.Model.TargetObjectKeyFormat".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "PartitionedPrefix" => o.partitioned_prefix = d.read_nullable()?,
                "SimplePrefix" => o.simple_prefix = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl KeyFormatInput {
    fn to_sdk(&self) -> Result<TargetObjectKeyFormat, CommandError> {
        let partitioned_prefix = self
            .partitioned_prefix
            .as_ref()
            .map(|prefix| {
                let source = prefix
                    .partition_date_source
                    .as_ref()
                    .map(|source| source.get().map(SdkPartitionDateSource::from))
                    .transpose()?;
                Ok::<_, CommandError>(
                    PartitionedPrefix::builder()
                        .set_partition_date_source(source)
                        .build(),
                )
            })
            .transpose()?;
        Ok(TargetObjectKeyFormat::builder()
            .set_partitioned_prefix(partitioned_prefix)
            .set_simple_prefix(
                self.simple_prefix
                    .as_ref()
                    .map(|_| SimplePrefix::builder().build()),
            )
            .build())
    }
}

/// SDK `S3BucketLoggingConfig`
#[derive(Debug, Default)]
pub(super) struct LoggingConfigInput {
    grants: Option<Vec<Option<GrantInput>>>,
    target_bucket_name: Option<String>,
    target_object_key_format: Option<KeyFormatInput>,
    target_prefix: Option<String>,
}

impl FromJson for LoggingConfigInput {
    fn type_name() -> String {
        "Amazon.S3.Model.S3BucketLoggingConfig".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Grants" => o.grants = read_list(d)?,
                "TargetBucketName" => o.target_bucket_name = d.read_nullable()?,
                "TargetObjectKeyFormat" => o.target_object_key_format = d.read_nullable()?,
                "TargetPrefix" => o.target_prefix = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl LoggingConfigInput {
    /// 요청 본문 `BucketLoggingStatus`.
    pub(super) fn to_sdk(&self) -> Result<BucketLoggingStatus, CommandError> {
        let logging_enabled = self
            .target_bucket_name
            .as_ref()
            .map(|bucket| {
                let grants = self.grants.as_ref().map(|grants| {
                    grants
                        .iter()
                        .flatten()
                        .map(|grant| {
                            TargetGrant::builder()
                                .set_grantee(grant.grantee.as_ref().map(GranteeInput::to_sdk))
                                .build()
                        })
                        .collect()
                });
                let key_format = self
                    .target_object_key_format
                    .as_ref()
                    .map(KeyFormatInput::to_sdk)
                    .transpose()?;
                Ok::<_, CommandError>(built(
                    LoggingEnabled::builder()
                        .target_bucket(bucket)
                        .target_prefix(self.target_prefix.as_deref().unwrap_or(""))
                        .set_target_grants(grants)
                        .set_target_object_key_format(key_format)
                        .build(),
                ))
            })
            .transpose()?;
        Ok(BucketLoggingStatus::builder()
            .set_logging_enabled(logging_enabled)
            .build())
    }
}
