//! `Data/S3/MyAnalyticsConfiguration.cs`, `MyDestination.cs`: 분석 설정 입력과 SDK 형식 변환.
//!
//! 원본 동작 그대로 둔 것
//! - `Filter`나 `Destination`이 없으면(`null`) `NullReferenceException`.
//! - 필터 조건이 없으면(`Prefix`도 `Tag`도 없는 `{}`) SDK가 요청 XML을 만들다 `NullReferenceException`.
//!   `Id` 검사(`AmazonS3Exception`)가 먼저다.

use aws_sdk_s3::types::{
    AnalyticsAndOperator, AnalyticsConfiguration, AnalyticsExportDestination, AnalyticsFilter,
    AnalyticsS3BucketDestination, AnalyticsS3ExportFileFormat, StorageClassAnalysis,
    StorageClassAnalysisDataExport, StorageClassAnalysisSchemaVersion,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::filter::{MyFilter, Predicate};
use super::{CommandError, built, null_reference, required_id};

/// `MyDestination`
#[derive(Debug, Default, Clone)]
pub(super) struct MyDestination {
    pub account_id: Option<String>,
    pub bucket_name: Option<String>,
    pub format: Option<String>,
    pub prefix: Option<String>,
}

impl FromJson for MyDestination {
    fn type_name() -> String {
        "TestCore.Data.S3.MyDestination".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "AccountId" => o.account_id = d.read_nullable()?,
                "BucketName" => o.bucket_name = d.read_nullable()?,
                "Format" => o.format = d.read_nullable()?,
                "Prefix" => o.prefix = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyDestination {
    /// `GetAnalyticsExportDestination()`
    fn export_destination(&self) -> AnalyticsExportDestination {
        let s3_bucket_destination = built(
            AnalyticsS3BucketDestination::builder()
                .format(AnalyticsS3ExportFileFormat::from(
                    self.format.as_deref().unwrap_or(UNSET),
                ))
                .set_bucket_account_id(self.account_id.clone())
                .bucket(self.bucket_name.as_deref().unwrap_or(UNSET))
                .set_prefix(self.prefix.clone())
                .build(),
        );
        AnalyticsExportDestination::builder()
            .s3_bucket_destination(s3_bucket_destination)
            .build()
    }
}

/// `MyAnalyticsConfiguration`
#[derive(Debug, Default, Clone)]
pub(super) struct MyAnalyticsConfiguration {
    pub id: Option<String>,
    pub filter: Option<MyFilter>,
    pub destination: Option<MyDestination>,
}

impl FromJson for MyAnalyticsConfiguration {
    fn type_name() -> String {
        "TestCore.Data.S3.MyAnalyticsConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Id" => o.id = d.read_nullable()?,
                "Filter" => o.filter = d.read_nullable()?,
                "Destination" => o.destination = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyAnalyticsConfiguration {
    /// `GetAnalyticsConfiguration()`에 클라이언트 호출 안에서 나는 오류(`Id` 검사, 빈 필터)까지 이어 붙인 것.
    pub(super) fn analytics_configuration(&self) -> Result<AnalyticsConfiguration, CommandError> {
        let predicate = self
            .filter
            .as_ref()
            .ok_or_else(null_reference)?
            .predicate()?;
        let destination = self
            .destination
            .as_ref()
            .ok_or_else(null_reference)?
            .export_destination();
        let id = required_id(self.id.as_deref(), "AnalyticsId")?;
        let filter = match predicate.ok_or_else(null_reference)? {
            Predicate::Prefix(prefix) => AnalyticsFilter::Prefix(prefix),
            Predicate::Tag(tag) => AnalyticsFilter::Tag(tag),
            Predicate::And { prefix, tags } => AnalyticsFilter::And(
                AnalyticsAndOperator::builder()
                    .set_prefix(prefix)
                    .set_tags(Some(tags))
                    .build(),
            ),
        };
        let data_export = built(
            StorageClassAnalysisDataExport::builder()
                .output_schema_version(StorageClassAnalysisSchemaVersion::V1)
                .destination(destination)
                .build(),
        );
        Ok(built(
            AnalyticsConfiguration::builder()
                .id(id)
                .filter(filter)
                .storage_class_analysis(
                    StorageClassAnalysis::builder()
                        .data_export(data_export)
                        .build(),
                )
                .build(),
        ))
    }
}
