//! TESTCore `Data/Ksan/*` 이식: KSAN 확장 API의 요청·응답 본문.
//!
//! 응답은 원본 `XmlSerializer` 규칙으로 읽는다. 루트 요소와 자식 요소는 S3 네임스페이스에 있어야 하고,
//! 모르는 요소는 무시하며, 같은 요소가 여러 번 나오면 마지막 값을 쓴다. JSON 출력(`ToString()`)은
//! 원본 속성 순서와 이름을 따른다.

use serde::Serialize;
use testrust_common::DotnetDateTime;

use crate::xml_doc::{self, Element, XmlError};

/// `http://s3.amazonaws.com/doc/2006-03-01/`
pub const S3_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

fn expect_root(root: &Element, ns: &str, name: &str) -> Result<(), XmlError> {
    if root.is(ns, name) {
        Ok(())
    } else {
        Err(XmlError(root.position))
    }
}

/// `XmlConvert.ToBoolean`
fn parse_bool(element: &Element) -> Result<bool, XmlError> {
    match element.text.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(XmlError(element.end_position)),
    }
}

/// `XmlConvert.ToInt32`
fn parse_i32(element: &Element) -> Result<i32, XmlError> {
    element
        .text
        .trim()
        .parse()
        .map_err(|_| XmlError(element.end_position))
}

/// 원본 `TagIndexingConfiguration`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagIndexingConfiguration {
    #[serde(rename = "Status")]
    pub status: String,
}

impl TagIndexingConfiguration {
    pub const ENABLED: &str = "Enabled";

    /// 응답 XML을 읽는다. `Status`가 없으면 기본 생성자 값 `Enabled`가 남는다.
    pub fn from_xml(text: &str) -> Result<Self, XmlError> {
        let root = xml_doc::parse(text)?;
        expect_root(&root, S3_NAMESPACE, "TagIndexingConfiguration")?;
        Ok(Self {
            status: root
                .child_text(S3_NAMESPACE, "Status")
                .unwrap_or_else(|| Self::ENABLED.to_string()),
        })
    }

    /// 원본 `Utility.ClassToXML`(`StringWriter` + `XmlSerializer`) 출력. `XmlWriterSettings` 기본값이라
    /// 줄바꿈은 운영체제와 관계없이 `\r\n`이다.
    pub fn to_xml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-16\"?>\r\n\
             <TagIndexingConfiguration xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
             xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns=\"{S3_NAMESPACE}\">\r\n  \
             <Status>{}</Status>\r\n</TagIndexingConfiguration>",
            escape_text(&self.status)
        )
    }
}

impl Default for TagIndexingConfiguration {
    fn default() -> Self {
        Self {
            status: Self::ENABLED.to_string(),
        }
    }
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 원본 `ListBucketTagSearchResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListBucketTagSearchResult {
    #[serde(rename = "IsTruncated")]
    pub is_truncated: bool,
    #[serde(rename = "Contents")]
    pub contents: Vec<Content>,
}

/// 원본 `Content`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Content {
    #[serde(rename = "ETag")]
    pub etag: Option<String>,
    #[serde(rename = "Key")]
    pub key: Option<String>,
    #[serde(rename = "LastModified")]
    pub last_modified: DotnetDateTime,
    #[serde(rename = "Owner")]
    pub owner: Option<Owner>,
    #[serde(rename = "Size")]
    pub size: i32,
    #[serde(rename = "StorageClass")]
    pub storage_class: Option<String>,
}

/// 원본 `Owner`. XML 요소 이름은 `Id`다(S3 응답의 `ID`는 읽히지 않는다).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Owner {
    #[serde(rename = "DisplayName")]
    pub display_name: Option<String>,
    #[serde(rename = "Id")]
    pub id: Option<String>,
}

impl ListBucketTagSearchResult {
    pub fn from_xml(text: &str) -> Result<Self, XmlError> {
        let root = xml_doc::parse(text)?;
        expect_root(&root, S3_NAMESPACE, "ListBucketTagSearchResult")?;
        let ns = S3_NAMESPACE;
        let mut result = Self::default();
        for child in &root.children {
            if child.is(ns, "IsTruncated") {
                result.is_truncated = parse_bool(child)?;
            } else if child.is(ns, "Contents") {
                result.contents.push(Content::from_element(child)?);
            }
        }
        Ok(result)
    }
}

impl Content {
    fn from_element(element: &Element) -> Result<Self, XmlError> {
        let ns = S3_NAMESPACE;
        let mut content = Self::default();
        for child in &element.children {
            match child.name.as_str() {
                _ if child.ns != ns => {}
                "ETag" => content.etag = Some(child.text.clone()),
                "Key" => content.key = Some(child.text.clone()),
                "LastModified" => {
                    content.last_modified = DotnetDateTime::parse_xml(&child.text)
                        .map_err(|_| XmlError(child.end_position))?;
                }
                "Owner" => {
                    content.owner = Some(Owner {
                        display_name: child.child_text(ns, "DisplayName"),
                        id: child.child_text(ns, "Id"),
                    });
                }
                "Size" => content.size = parse_i32(child)?,
                "StorageClass" => content.storage_class = Some(child.text.clone()),
                _ => {}
            }
        }
        Ok(content)
    }
}

/// 원본 `KsanErrorResponse`(루트 `Error`, 네임스페이스 없음).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct KsanErrorResponse {
    #[serde(rename = "Code")]
    pub code: Option<String>,
    #[serde(rename = "Message")]
    pub message: Option<String>,
    #[serde(rename = "RequestId")]
    pub request_id: Option<String>,
}

impl KsanErrorResponse {
    pub fn from_xml(text: &str) -> Result<Self, XmlError> {
        let root = xml_doc::parse(text)?;
        expect_root(&root, "", "Error")?;
        Ok(Self {
            code: root.child_text("", "Code"),
            message: root.child_text("", "Message"),
            request_id: root.child_text("", "RequestId"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_index_defaults_to_enabled() {
        let xml = format!("<TagIndexingConfiguration xmlns=\"{S3_NAMESPACE}\"/>");
        assert_eq!(
            TagIndexingConfiguration::from_xml(&xml).unwrap().status,
            "Enabled"
        );
        assert_eq!(
            TagIndexingConfiguration::from_xml("<TagIndexingConfiguration/>").unwrap_err(),
            XmlError((1, 2))
        );
    }
}
