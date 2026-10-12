//! SDK `WebsiteConfiguration`: `PutBucketWebsite` 입력(SDK 모델을 그대로 읽는다).
//!
//! 원본 동작 그대로 둔 것
//! - `RedirectAllRequestsTo`의 형식은 `RoutingRuleRedirect`(리다이렉트 규칙과 같은 모델)이며 요청에는
//!   `HostName`, `Protocol`만 실린다.
//! - 라우팅 규칙 요소가 `null`이면 건너뛴다. `ErrorDocument`·`IndexDocumentSuffix`는 값이 있으면 빈 문자열도 보낸다.
//!
//! 어긋나는 점: 파일 내용이 JSON `null`이면 원본은 본문 없이 요청을 보내지만 여기서는 빈
//! `<WebsiteConfiguration>`을 보낸다.

use aws_sdk_s3::types::{
    Condition, ErrorDocument, IndexDocument, Protocol, Redirect, RedirectAllRequestsTo,
    RoutingRule, WebsiteConfiguration,
};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::built;
use super::jsonutil::read_list;

/// SDK `RoutingRuleRedirect`
#[derive(Debug, Default, Clone)]
struct RedirectInput {
    host_name: Option<String>,
    http_redirect_code: Option<String>,
    protocol: Option<String>,
    replace_key_prefix_with: Option<String>,
    replace_key_with: Option<String>,
}

impl FromJson for RedirectInput {
    fn type_name() -> String {
        "Amazon.S3.Model.RoutingRuleRedirect".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "HostName" => o.host_name = d.read_nullable()?,
                "HttpRedirectCode" => o.http_redirect_code = d.read_nullable()?,
                "Protocol" => o.protocol = d.read_nullable()?,
                "ReplaceKeyPrefixWith" => o.replace_key_prefix_with = d.read_nullable()?,
                "ReplaceKeyWith" => o.replace_key_with = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl RedirectInput {
    fn to_redirect(&self) -> Redirect {
        Redirect::builder()
            .set_host_name(self.host_name.clone())
            .set_http_redirect_code(self.http_redirect_code.clone())
            .set_protocol(self.protocol.as_deref().map(Protocol::from))
            .set_replace_key_prefix_with(self.replace_key_prefix_with.clone())
            .set_replace_key_with(self.replace_key_with.clone())
            .build()
    }

    fn to_redirect_all(&self) -> RedirectAllRequestsTo {
        built(
            RedirectAllRequestsTo::builder()
                .host_name(self.host_name.as_deref().unwrap_or(UNSET))
                .set_protocol(self.protocol.as_deref().map(Protocol::from))
                .build(),
        )
    }
}

/// SDK `RoutingRuleCondition`
#[derive(Debug, Default, Clone)]
struct ConditionInput {
    http_error_code_returned_equals: Option<String>,
    key_prefix_equals: Option<String>,
}

impl FromJson for ConditionInput {
    fn type_name() -> String {
        "Amazon.S3.Model.RoutingRuleCondition".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "HttpErrorCodeReturnedEquals" => {
                    o.http_error_code_returned_equals = d.read_nullable()?;
                }
                "KeyPrefixEquals" => o.key_prefix_equals = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// SDK `RoutingRule`
#[derive(Debug, Default, Clone)]
struct RoutingRuleInput {
    condition: Option<ConditionInput>,
    redirect: Option<RedirectInput>,
}

impl FromJson for RoutingRuleInput {
    fn type_name() -> String {
        "Amazon.S3.Model.RoutingRule".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Condition" => o.condition = d.read_nullable()?,
                "Redirect" => o.redirect = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl RoutingRuleInput {
    fn to_sdk(&self) -> RoutingRule {
        let condition = self.condition.as_ref().map(|condition| {
            Condition::builder()
                .set_http_error_code_returned_equals(
                    condition.http_error_code_returned_equals.clone(),
                )
                .set_key_prefix_equals(condition.key_prefix_equals.clone())
                .build()
        });
        RoutingRule::builder()
            .set_condition(condition)
            .set_redirect(self.redirect.as_ref().map(RedirectInput::to_redirect))
            .build()
    }
}

/// SDK `WebsiteConfiguration`
#[derive(Debug, Default)]
pub(super) struct WebsiteConfigurationInput {
    error_document: Option<String>,
    index_document_suffix: Option<String>,
    redirect_all_requests_to: Option<RedirectInput>,
    routing_rules: Option<Vec<Option<RoutingRuleInput>>>,
}

impl FromJson for WebsiteConfigurationInput {
    fn type_name() -> String {
        "Amazon.S3.Model.WebsiteConfiguration".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "ErrorDocument" => o.error_document = d.read_nullable()?,
                "IndexDocumentSuffix" => o.index_document_suffix = d.read_nullable()?,
                "RedirectAllRequestsTo" => o.redirect_all_requests_to = d.read_nullable()?,
                "RoutingRules" => o.routing_rules = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl WebsiteConfigurationInput {
    pub(super) fn to_sdk(&self) -> WebsiteConfiguration {
        WebsiteConfiguration::builder()
            .set_error_document(
                self.error_document
                    .as_deref()
                    .map(|key| built(ErrorDocument::builder().key(key).build())),
            )
            .set_index_document(
                self.index_document_suffix
                    .as_deref()
                    .map(|suffix| built(IndexDocument::builder().suffix(suffix).build())),
            )
            .set_redirect_all_requests_to(
                self.redirect_all_requests_to
                    .as_ref()
                    .map(RedirectInput::to_redirect_all),
            )
            .set_routing_rules(self.routing_rules.as_ref().map(|rules| {
                rules
                    .iter()
                    .flatten()
                    .map(RoutingRuleInput::to_sdk)
                    .collect()
            }))
            .build()
    }
}
