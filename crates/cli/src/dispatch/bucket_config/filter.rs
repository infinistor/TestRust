//! `Data/S3/MyFilter.cs`, `MyTag.cs`: 분석·메트릭 설정 입력의 필터.
//!
//! `GetAnalyticsFilter`와 `GetMetricsFilter`는 같은 분기를 쓰므로 한 번만 옮기고 SDK 형식은 각 설정이 만든다.
//! 원본 동작 그대로 둔 것
//! - `Tag`가 둘 이하(`Prefix`가 없을 때)이면 첫 태그 하나만 쓴다(`Tag.Count > 2`일 때만 `And`로 묶는다).
//! - 태그 요소가 `null`이면 `item.Key`에서 `NullReferenceException`.

use aws_sdk_s3::types::Tag;
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::UNSET;

use super::jsonutil::read_list;
use super::{CommandError, built, null_reference};

/// `MyTag`
#[derive(Debug, Default, Clone)]
pub(super) struct MyTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl FromJson for MyTag {
    fn type_name() -> String {
        "TestCore.Data.S3.MyTag".into()
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

impl MyTag {
    /// `new Tag { Key = item.Key, Value = item.Value }`. 값이 없으면 요청에서 뺀다.
    fn to_sdk(&self) -> Tag {
        built(
            Tag::builder()
                .key(self.key.as_deref().unwrap_or(UNSET))
                .value(self.value.as_deref().unwrap_or(UNSET))
                .build(),
        )
    }
}

/// 필터 조건(분석 `AnalyticsFilterPredicate`, 메트릭 `MetricsFilterPredicate`).
pub(super) enum Predicate {
    Prefix(String),
    Tag(Tag),
    And {
        prefix: Option<String>,
        tags: Vec<Tag>,
    },
}

/// `MyFilter`
#[derive(Debug, Default, Clone)]
pub(super) struct MyFilter {
    pub prefix: Option<String>,
    pub tag: Option<Vec<Option<MyTag>>>,
}

impl FromJson for MyFilter {
    fn type_name() -> String {
        "TestCore.Data.S3.MyFilter".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Prefix" => o.prefix = d.read_nullable()?,
                "Tag" => o.tag = read_list(d)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// 요소가 `null`이면 `NullReferenceException`.
fn tag_of(item: &Option<MyTag>) -> Result<Tag, CommandError> {
    item.as_ref().map(MyTag::to_sdk).ok_or_else(null_reference)
}

impl MyFilter {
    /// `Prefix`·`Tag` 조합에 맞는 조건. 조건이 없으면 `None`.
    pub(super) fn predicate(&self) -> Result<Option<Predicate>, CommandError> {
        Ok(match (&self.prefix, &self.tag) {
            // Prefix, Tag가 모두 있을 경우
            (Some(prefix), Some(tags)) => Some(Predicate::And {
                prefix: Some(prefix.clone()),
                tags: tags.iter().map(tag_of).collect::<Result<_, _>>()?,
            }),
            // Prefix만 있을 경우
            (Some(prefix), None) => Some(Predicate::Prefix(prefix.clone())),
            // Tag만 있을 경우
            (None, Some(tags)) if !tags.is_empty() => {
                if tags.len() > 2 {
                    Some(Predicate::And {
                        prefix: None,
                        tags: tags.iter().map(tag_of).collect::<Result<_, _>>()?,
                    })
                } else {
                    Some(Predicate::Tag(tag_of(&tags[0])?))
                }
            }
            _ => None,
        })
    }
}
