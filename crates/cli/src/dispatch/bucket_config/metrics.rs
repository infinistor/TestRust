//! `Data/S3/MyMetricsConfiguration.cs`: 메트릭 설정 입력과 SDK 형식 변환.
//!
//! 원본 동작 그대로 둔 것: `Filter`가 없으면(속성 자체가 없을 때) 빈 `MyFilter`지만, JSON `null`이면
//! `NullReferenceException`. 조건이 없는 필터는 요청에 `Filter`를 싣지 않는다.

use aws_sdk_s3::types::{MetricsAndOperator, MetricsConfiguration, MetricsFilter};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};

use super::filter::{MyFilter, Predicate};
use super::{CommandError, built, null_reference, required_id};

/// `MyMetricsConfiguration`
#[derive(Debug, Clone)]
pub(super) struct MyMetricsConfiguration {
    pub id: Option<String>,
    pub filter: Option<MyFilter>,
}

impl Default for MyMetricsConfiguration {
    fn default() -> Self {
        Self {
            id: None,
            filter: Some(MyFilter::default()),
        }
    }
}

impl FromJson for MyMetricsConfiguration {
    fn type_name() -> String {
        "TestCore.Data.S3.MyMetricsConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Id" => o.id = d.read_nullable()?,
                "Filter" => o.filter = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyMetricsConfiguration {
    /// `GetMetricsConfiguration()`에 클라이언트 호출 안에서 나는 오류(`Id` 검사)까지 이어 붙인 것.
    pub(super) fn metrics_configuration(&self) -> Result<MetricsConfiguration, CommandError> {
        let predicate = self
            .filter
            .as_ref()
            .ok_or_else(null_reference)?
            .predicate()?;
        let id = required_id(self.id.as_deref(), "MetricsId")?;
        let filter = predicate.map(|predicate| match predicate {
            Predicate::Prefix(prefix) => MetricsFilter::Prefix(prefix),
            Predicate::Tag(tag) => MetricsFilter::Tag(tag),
            Predicate::And { prefix, tags } => MetricsFilter::And(
                MetricsAndOperator::builder()
                    .set_prefix(prefix)
                    .set_tags(Some(tags))
                    .build(),
            ),
        });
        Ok(built(
            MetricsConfiguration::builder()
                .id(id)
                .set_filter(filter)
                .build(),
        ))
    }
}
