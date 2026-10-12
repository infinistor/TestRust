//! SDK `ServerSideEncryptionConfiguration`: `PutBucketEncryption` 입력(SDK 모델을 그대로 읽는다).
//!
//! 원본 동작 그대로 둔 것
//! - `ServerSideEncryptionAlgorithm`은 `{ "Value": "..." }` 객체이고, `Value`가 없으면 `ArgumentNullException`.
//!   문자열로 쓰면 변환 오류다.
//! - 규칙 요소가 `null`이면 건너뛴다. 알고리즘이 없으면 요청에서 뺀다.
//!
//! 어긋나는 점: 파일 내용이 JSON `null`이면 원본은 본문 없이 요청을 보내지만 여기서는 빈
//! `<ServerSideEncryptionConfiguration>`을 보낸다.

use aws_sdk_s3::types::{
    BlockedEncryptionTypes, EncryptionType, ServerSideEncryption, ServerSideEncryptionByDefault,
    ServerSideEncryptionConfiguration, ServerSideEncryptionRule,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::jsonutil::{ConstantValue, NullableBool, ServerSideEncryptionMethod, read_list};
use super::{CommandError, built};

/// SDK `ServerSideEncryptionByDefault`
#[derive(Debug, Default)]
struct ByDefaultInput {
    algorithm: Option<ConstantValue>,
    key_id: Option<String>,
}

impl FromJson for ByDefaultInput {
    fn type_name() -> String {
        "Amazon.S3.Model.ServerSideEncryptionByDefault".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "ServerSideEncryptionAlgorithm" => {
                    o.algorithm = d
                        .read_nullable::<ServerSideEncryptionMethod>()?
                        .map(|method| method.0);
                }
                "ServerSideEncryptionKeyManagementServiceKeyId" => o.key_id = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `BlockedEncryptionTypes`
#[derive(Debug, Default)]
struct BlockedInput {
    encryption_type: Option<Vec<Option<String>>>,
}

impl FromJson for BlockedInput {
    fn type_name() -> String {
        "Amazon.S3.Model.BlockedEncryptionTypes".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "EncryptionType" => o.encryption_type = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `ServerSideEncryptionRule`
#[derive(Debug, Default)]
struct RuleInput {
    blocked_encryption_types: Option<BlockedInput>,
    bucket_key_enabled: Option<bool>,
    by_default: Option<ByDefaultInput>,
}

impl FromJson for RuleInput {
    fn type_name() -> String {
        "Amazon.S3.Model.ServerSideEncryptionRule".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "BlockedEncryptionTypes" => o.blocked_encryption_types = d.read_nullable()?,
                "BucketKeyEnabled" => {
                    o.bucket_key_enabled = d.read_nullable::<NullableBool>()?.map(|v| v.0);
                }
                "ServerSideEncryptionByDefault" => o.by_default = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl RuleInput {
    fn to_sdk(&self) -> Result<ServerSideEncryptionRule, CommandError> {
        let by_default = self
            .by_default
            .as_ref()
            .map(|by_default| {
                let algorithm = match &by_default.algorithm {
                    Some(algorithm) => algorithm.get()?,
                    None => UNSET,
                };
                Ok::<_, CommandError>(built(
                    ServerSideEncryptionByDefault::builder()
                        .sse_algorithm(ServerSideEncryption::from(algorithm))
                        .set_kms_master_key_id(by_default.key_id.clone())
                        .build(),
                ))
            })
            .transpose()?;
        let blocked = self.blocked_encryption_types.as_ref().map(|blocked| {
            let types = blocked.encryption_type.as_ref().map(|types| {
                types
                    .iter()
                    .map(|item| EncryptionType::from(item.as_deref().unwrap_or("")))
                    .collect()
            });
            BlockedEncryptionTypes::builder()
                .set_encryption_type(types)
                .build()
        });
        Ok(ServerSideEncryptionRule::builder()
            .set_apply_server_side_encryption_by_default(by_default)
            .set_bucket_key_enabled(self.bucket_key_enabled)
            .set_blocked_encryption_types(blocked)
            .build())
    }
}

/// SDK `ServerSideEncryptionConfiguration`
#[derive(Debug, Default)]
pub(super) struct SseConfigurationInput {
    rules: Option<Vec<Option<RuleInput>>>,
}

impl FromJson for SseConfigurationInput {
    fn type_name() -> String {
        "Amazon.S3.Model.ServerSideEncryptionConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "ServerSideEncryptionRules" => o.rules = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl SseConfigurationInput {
    pub(super) fn to_sdk(&self) -> Result<ServerSideEncryptionConfiguration, CommandError> {
        let rules = self
            .rules
            .iter()
            .flatten()
            .flatten()
            .map(RuleInput::to_sdk)
            .collect::<Result<_, _>>()?;
        Ok(built(
            ServerSideEncryptionConfiguration::builder()
                .set_rules(Some(rules))
                .build(),
        ))
    }
}
