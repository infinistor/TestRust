//! SDK `CORSConfiguration`, `CORSRule`: `PutBucketCors` 입력. 원본은 `JsonSerializer.Deserialize<CORSConfiguration>`로
//! SDK 모델을 그대로 읽는다.
//!
//! 원본 동작 그대로 둔 것: 규칙 요소가 `null`이면 건너뛰고, 문자열 목록의 `null` 요소는 빈 문자열로 보낸다.
//! 어긋나는 점: 파일 내용이 JSON `null`이면 원본은 본문 없이 요청을 보내지만(서버는 오류) 여기서는 빈
//! `<CORSConfiguration>`을 보낸다(SDK가 본문 없는 요청을 만들 수 없다).

use aws_sdk_s3::types::{CorsConfiguration, CorsRule};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};

use super::built;
use super::jsonutil::{NullableI32, read_list};

/// SDK `CORSRule`
#[derive(Debug, Default, Clone)]
struct CorsRuleInput {
    allowed_headers: Option<Vec<Option<String>>>,
    allowed_methods: Option<Vec<Option<String>>>,
    allowed_origins: Option<Vec<Option<String>>>,
    expose_headers: Option<Vec<Option<String>>>,
    id: Option<String>,
    max_age_seconds: Option<i32>,
}

impl FromJson for CorsRuleInput {
    fn type_name() -> String {
        "Amazon.S3.Model.CORSRule".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "AllowedHeaders" => o.allowed_headers = read_list(d)?,
                "AllowedMethods" => o.allowed_methods = read_list(d)?,
                "AllowedOrigins" => o.allowed_origins = read_list(d)?,
                "ExposeHeaders" => o.expose_headers = read_list(d)?,
                "Id" => o.id = d.read_nullable()?,
                "MaxAgeSeconds" => {
                    o.max_age_seconds = d.read_nullable::<NullableI32>()?.map(|v| v.0);
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// 문자열 목록: `null` 요소는 빈 문자열.
fn strings(items: &Option<Vec<Option<String>>>) -> Option<Vec<String>> {
    items.as_ref().map(|items| {
        items
            .iter()
            .map(|item| item.clone().unwrap_or_default())
            .collect()
    })
}

impl CorsRuleInput {
    fn to_sdk(&self) -> CorsRule {
        built(
            CorsRule::builder()
                .set_id(self.id.clone())
                .set_allowed_headers(strings(&self.allowed_headers))
                .set_allowed_methods(Some(strings(&self.allowed_methods).unwrap_or_default()))
                .set_allowed_origins(Some(strings(&self.allowed_origins).unwrap_or_default()))
                .set_expose_headers(strings(&self.expose_headers))
                .set_max_age_seconds(self.max_age_seconds)
                .build(),
        )
    }
}

/// SDK `CORSConfiguration`
#[derive(Debug, Default, Clone)]
pub(super) struct CorsConfigurationInput {
    rules: Option<Vec<Option<CorsRuleInput>>>,
}

impl FromJson for CorsConfigurationInput {
    fn type_name() -> String {
        "Amazon.S3.Model.CORSConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Rules" => o.rules = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl CorsConfigurationInput {
    pub(super) fn to_sdk(&self) -> CorsConfiguration {
        let rules = self
            .rules
            .iter()
            .flatten()
            .flatten()
            .map(CorsRuleInput::to_sdk)
            .collect();
        built(
            CorsConfiguration::builder()
                .set_cors_rules(Some(rules))
                .build(),
        )
    }
}
