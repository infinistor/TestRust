//! 입력 JSON 파일을 읽는 DTO. 원본의 `JsonSerializer.Deserialize<T>(text)`(대소문자를 구분하는 기본 옵션).
//!
//! - `List<KeyVersion>`(`DeleteObjects`): `Key`, `VersionId`, `ETag`, `LastModifiedTime`, `Size`.
//! - `ObjectLockLegalHold`(`PutObjectLegalHold`): `Status`는 `ObjectLockLegalHoldStatus`(생성자 `value`로
//!   바인딩되는 `ConstantClass`)라 `{ "Value": "ON" }` 모양이다.
//! - `Tagging`(`PutObjectTagging`): `TagSet`은 `List<Tag>`(`Key`, `Value`).

use aws_sdk_s3::primitives::DateTime;
use aws_sdk_s3::types::{
    ObjectIdentifier, ObjectLockLegalHold, ObjectLockLegalHoldStatus, Tag, Tagging,
};
use testrust_common::DotnetDateTime;
use testrust_common::json::{
    Deserializer, FromJson, JsonError, ReadOptions, Token, deserialize, list_type_name,
};
use testrust_s3::S3Error;

use crate::dispatch::CommandError;

/// `JsonSerializer.Deserialize<T>(text)`. JSON `null`이면 `None`. 실패하면 `JsonException`.
pub(super) fn parse<T: FromJson>(text: &str) -> Result<Option<T>, CommandError> {
    deserialize::<T>(text, ReadOptions::default())
        .map_err(|JsonError(message)| CommandError::new("System.Text.Json.JsonException", message))
}

/// `Amazon.S3.Model.KeyVersion`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct KeyVersion {
    pub key: Option<String>,
    pub version_id: Option<String>,
    pub etag: Option<String>,
    pub last_modified_time: Option<DotnetDateTime>,
    pub size: Option<i64>,
}

impl FromJson for KeyVersion {
    fn type_name() -> String {
        "Amazon.S3.Model.KeyVersion".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Key" => o.key = d.read_nullable()?,
                "VersionId" => o.version_id = d.read_nullable()?,
                "ETag" => o.etag = d.read_nullable()?,
                "LastModifiedTime" => o.last_modified_time = d.read_nullable()?,
                "Size" => o.size = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl KeyVersion {
    /// 요청에 넣는 `ObjectIdentifier`. 원본은 `Key`가 없어도 요소를 빼고 보내지만 SDK 형식은 키가 필수다.
    pub(super) fn to_identifier(&self) -> Result<ObjectIdentifier, S3Error> {
        let modified = match &self.last_modified_time {
            Some(time) => Some(DateTime::from_secs(
                time.to_unix_seconds().map_err(S3Error::Argument)?,
            )),
            None => None,
        };
        ObjectIdentifier::builder()
            .set_key(self.key.clone())
            .set_version_id(self.version_id.clone())
            .set_e_tag(self.etag.clone())
            .set_last_modified_time(modified)
            .set_size(self.size)
            .build()
            .map_err(|e| S3Error::Request(e.to_string()))
    }
}

/// `ObjectLockLegalHoldStatus`: 생성자 매개변수 `value`에 `Value` 속성이 바인딩된다.
/// 바깥 `Option`은 `Status`의 유무, 안쪽은 `Value`의 유무(없으면 null 상수가 된다).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LegalHoldStatus {
    pub value: Option<String>,
}

impl FromJson for LegalHoldStatus {
    fn type_name() -> String {
        "Amazon.S3.ObjectLockLegalHoldStatus".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            if name == "Value" {
                o.value = d.read_nullable()?;
                return Ok(true);
            }
            Ok(false)
        })
    }
}

/// `Amazon.S3.Model.ObjectLockLegalHold`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LegalHold {
    pub status: Option<LegalHoldStatus>,
}

impl FromJson for LegalHold {
    fn type_name() -> String {
        "Amazon.S3.Model.ObjectLockLegalHold".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            if name == "Status" {
                o.status = d.read_nullable()?;
                return Ok(true);
            }
            Ok(false)
        })
    }
}

impl LegalHold {
    /// 요청에 넣는 값. `Status`가 있지만 `Value`가 없으면 원본은 요청을 만드는 중에
    /// `ArgumentNullException`(`Parameter 'key'`)을 던진다.
    pub(super) fn to_sdk(&self) -> Result<ObjectLockLegalHold, CommandError> {
        let status = match &self.status {
            None => None,
            Some(LegalHoldStatus { value: None }) => {
                return Err(CommandError::new(
                    "System.ArgumentNullException",
                    "Value cannot be null. (Parameter 'key')",
                ));
            }
            Some(LegalHoldStatus { value: Some(value) }) => {
                Some(ObjectLockLegalHoldStatus::from(value.as_str()))
            }
        };
        Ok(ObjectLockLegalHold::builder().set_status(status).build())
    }
}

/// `Amazon.S3.Model.Tag`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TagItem {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl FromJson for TagItem {
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

/// `Amazon.S3.Model.Tagging`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TaggingInput {
    pub tag_set: Option<Vec<Option<TagItem>>>,
}

impl FromJson for TaggingInput {
    fn type_name() -> String {
        "Amazon.S3.Model.Tagging".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            if name == "TagSet" {
                let tok = d.read_token()?;
                o.tag_set = d.read_list::<TagItem>(tok)?;
                return Ok(true);
            }
            Ok(false)
        })
    }
}

impl TaggingInput {
    /// 요청에 넣는 값. `null` 요소는 건너뛴다. SDK 형식은 `Tag`의 키·값이 필수라 없는 값은 표식(`UNSET`)으로
    /// 만들고, S3 클라이언트가 서명 전에 그 요소를 지운다(원본처럼 요소를 빼고 보낸다).
    pub(super) fn to_sdk(&self) -> Result<Tagging, S3Error> {
        let tags = self
            .tag_set
            .iter()
            .flatten()
            .flatten()
            .map(|tag| {
                Tag::builder()
                    .key(
                        tag.key
                            .clone()
                            .unwrap_or_else(|| testrust_s3::UNSET.to_string()),
                    )
                    .value(
                        tag.value
                            .clone()
                            .unwrap_or_else(|| testrust_s3::UNSET.to_string()),
                    )
                    .build()
                    .map_err(|e| S3Error::Request(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Tagging::builder()
            .set_tag_set(Some(tags))
            .build()
            .map_err(|e| S3Error::Request(e.to_string()))
    }
}

/// `List<KeyVersion>`. 요소가 JSON `null`이면 `None`이다.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct KeyVersionList(pub Vec<Option<KeyVersion>>);

impl FromJson for KeyVersionList {
    fn type_name() -> String {
        list_type_name::<KeyVersion>()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        Ok(d.read_list::<KeyVersion>(tok)?.map(Self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_version_list_reads_known_properties() {
        let list =
            parse::<KeyVersionList>(r#"[{"Key":"a","VersionId":"v","Size":5,"x":[1]},null]"#)
                .unwrap()
                .unwrap();
        assert_eq!(list.0.len(), 2);
        let first = list.0[0].as_ref().unwrap();
        assert_eq!(first.key.as_deref(), Some("a"));
        assert_eq!(first.version_id.as_deref(), Some("v"));
        assert_eq!(first.size, Some(5));
        assert!(list.0[1].is_none());
        assert!(parse::<KeyVersionList>("null").unwrap().is_none());
    }

    #[test]
    fn json_errors_keep_dotnet_messages() {
        let error = parse::<KeyVersionList>(r#"{ "Key": "a" }"#).unwrap_err();
        assert_eq!(
            error.to_string(),
            "System.Text.Json.JsonException: The JSON value could not be converted to System.Collections.Generic.List`1[Amazon.S3.Model.KeyVersion]. Path: $ | LineNumber: 0 | BytePositionInLine: 1."
        );
        let error = parse::<LegalHold>(r#"{ "Status": "ON" }"#).unwrap_err();
        assert_eq!(
            error.to_string(),
            "System.Text.Json.JsonException: The JSON value could not be converted to Amazon.S3.ObjectLockLegalHoldStatus. Path: $.Status | LineNumber: 0 | BytePositionInLine: 16."
        );
    }

    #[test]
    fn legal_hold_value_is_case_sensitive() {
        let hold = parse::<LegalHold>(r#"{ "Status": { "value": "ON" } }"#)
            .unwrap()
            .unwrap();
        assert_eq!(
            hold.to_sdk().unwrap_err().message,
            "Value cannot be null. (Parameter 'key')"
        );
        let hold = parse::<LegalHold>(r#"{ "Status": { "Value": "OFF" } }"#)
            .unwrap()
            .unwrap();
        assert_eq!(hold.to_sdk().unwrap().status().unwrap().as_str(), "OFF");
    }

    #[test]
    fn tagging_skips_null_tags() {
        let tagging =
            parse::<TaggingInput>(r#"{ "TagSet": [ null, { "Key": "k", "Value": "v" } ] }"#)
                .unwrap()
                .unwrap()
                .to_sdk()
                .unwrap();
        assert_eq!(tagging.tag_set().len(), 1);
    }
}
