//! `Data/S3/MyInventoryConfiguration.cs`, `InventoryS3BucketDestination.cs`(`MyInventoryDestination`):
//! 인벤토리 설정 입력과 SDK 형식 변환.
//!
//! 원본 동작 그대로 둔 것
//! - `Destination`이나 `InventoryOptionalFields`가 없으면(`null`) `NullReferenceException`.
//! - `IncludedObjectVersions`는 채우지 않아 요청에 싣지 않는다(S3는 필수로 여긴다).
//! - `Encryption`은 `SSE-S3`(대소문자 무시)일 때만 암호화 설정이 되고 그 밖의 값은 무시한다.
//! - 선택 필드 문자열은 `FindValue`로 바꾸므로 모르는 값도 그대로 보낸다(`null` 요소는 빈 문자열).

use aws_sdk_s3::types::{
    InventoryConfiguration, InventoryDestination, InventoryEncryption, InventoryFormat,
    InventoryFrequency, InventoryIncludedObjectVersions, InventoryOptionalField,
    InventoryS3BucketDestination, InventorySchedule, Sses3,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::jsonutil::read_list;
use super::{CommandError, built, null_reference, required_id};

/// `MyInventoryDestination`
#[derive(Debug, Default, Clone)]
pub(super) struct MyInventoryDestination {
    pub account_id: Option<String>,
    pub bucket_name: Option<String>,
    pub format: Option<String>,
    pub prefix: Option<String>,
    pub encryption: Option<String>,
}

impl FromJson for MyInventoryDestination {
    fn type_name() -> String {
        "TestCore.Data.S3.MyInventoryDestination".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "AccountId" => o.account_id = d.read_nullable()?,
                "BucketName" => o.bucket_name = d.read_nullable()?,
                "Format" => o.format = d.read_nullable()?,
                "Prefix" => o.prefix = d.read_nullable()?,
                "Encryption" => o.encryption = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyInventoryDestination {
    /// `GetInventoryDestination()`
    fn inventory_destination(&self) -> InventoryDestination {
        let encryption = self
            .encryption
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("SSE-S3"))
            .then(|| {
                InventoryEncryption::builder()
                    .sses3(Sses3::builder().build())
                    .build()
            });
        let s3_bucket_destination = built(
            InventoryS3BucketDestination::builder()
                .set_account_id(self.account_id.clone())
                .bucket(self.bucket_name.as_deref().unwrap_or(UNSET))
                .format(InventoryFormat::from(
                    self.format.as_deref().unwrap_or(UNSET),
                ))
                .set_prefix(self.prefix.clone())
                .set_encryption(encryption)
                .build(),
        );
        InventoryDestination::builder()
            .s3_bucket_destination(s3_bucket_destination)
            .build()
    }
}

/// `MyInventoryConfiguration`
#[derive(Debug, Default, Clone)]
pub(super) struct MyInventoryConfiguration {
    pub is_enabled: bool,
    pub id: Option<String>,
    pub destination: Option<MyInventoryDestination>,
    pub schedule: Option<String>,
    pub inventory_optional_fields: Option<Vec<Option<String>>>,
}

impl FromJson for MyInventoryConfiguration {
    fn type_name() -> String {
        "TestCore.Data.S3.MyInventoryConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "IsEnabled" => o.is_enabled = d.read_value()?,
                "Id" => o.id = d.read_nullable()?,
                "Destination" => o.destination = d.read_nullable()?,
                "Schedule" => o.schedule = d.read_nullable()?,
                "InventoryOptionalFields" => o.inventory_optional_fields = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyInventoryConfiguration {
    /// `GetInventoryConfiguration()`에 클라이언트 호출 안에서 나는 오류(`Id` 검사)까지 이어 붙인 것.
    pub(super) fn inventory_configuration(&self) -> Result<InventoryConfiguration, CommandError> {
        let destination = self
            .destination
            .as_ref()
            .ok_or_else(null_reference)?
            .inventory_destination();
        let fields = self
            .inventory_optional_fields
            .as_ref()
            .ok_or_else(null_reference)?
            .iter()
            .map(|field| InventoryOptionalField::from(field.as_deref().unwrap_or("")))
            .collect();
        let id = required_id(self.id.as_deref(), "InventoryId")?;
        let schedule = built(
            InventorySchedule::builder()
                .frequency(InventoryFrequency::from(
                    self.schedule.as_deref().unwrap_or(UNSET),
                ))
                .build(),
        );
        Ok(built(
            InventoryConfiguration::builder()
                .is_enabled(self.is_enabled)
                .id(id)
                .destination(destination)
                .schedule(schedule)
                .included_object_versions(InventoryIncludedObjectVersions::from(UNSET))
                .set_optional_fields(Some(fields))
                .build(),
        ))
    }
}
