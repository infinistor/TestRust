//! 명령 출력 공용 도우미.
//!
//! - SDK 응답 JSON 덤프(`if (print) Console.WriteLine(JsonSerializer.Serialize(response.X, jsonOptions))`)는
//!   사람이 보는 용도라 .NET과 글자 단위로 맞추지 않는다(사용자 결정). SDK 형식의 `Debug` 출력을 읽어
//!   PascalCase 속성 이름의 들여쓴 JSON으로 바꾼다. `None`과 `_`로 시작하는 내부 필드는 뺀다.
//! - 날짜: 원본의 `DateTime` 기본 `ToString()`은 문화권에 따라 달라 ko-KR 형식(`yyyy-MM-dd tt h:mm:ss`)으로 고정한다.
//!   시각은 .NET SDK처럼 UTC 그대로 쓴다.

use std::fmt::Debug;

use chrono::{DateTime, Utc};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use testrust_common::to_dotnet_json;

/// 원본 `LINE`(74자).
pub const LINE: &str = "--------------------------------------------------------------------------";

/// SDK 값을 JSON 덤프 문자열로 바꾼다. 해석하지 못하면 `Debug` 출력을 그대로 쓴다.
pub fn dump_json<T: Debug + ?Sized>(value: &T) -> String {
    let text = format!("{value:?}");
    let mut parser = DebugParser {
        chars: text.chars().collect(),
        pos: 0,
    };
    match parser.value() {
        Some(node) if parser.pos == parser.chars.len() => to_dotnet_json(&node),
        _ => format!("{value:#?}"),
    }
}

/// [`dump_json`] 결과를 한 줄로 출력한다(`Console.WriteLine`).
pub fn print_json<T: Debug + ?Sized>(value: &T) {
    println!("{}", dump_json(value));
}

/// `str.PadRight(width)`: UTF-16 길이 기준으로 공백을 채운다.
pub fn pad_right(text: &str, width: usize) -> String {
    let length = utf16_len(text);
    if length >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - length))
    }
}

/// `str.Length`(UTF-16 코드 단위 수).
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// SDK 시각(UTC). AWS SDK(.NET v4)는 응답 XML의 시각을 UTC `DateTime`으로 읽고 그대로 서식을 적용한다.
pub fn utc_time(time: &aws_sdk_s3::primitives::DateTime) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(time.secs(), time.subsec_nanos())
}

/// `DateTime.ToString("yyyy-MM-dd HH:mm:ss", InvariantInfo)`(UTC).
pub fn invariant_time(time: &aws_sdk_s3::primitives::DateTime) -> String {
    utc_time(time).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// ko-KR `DateTime.ToString()`: `yyyy-MM-dd tt h:mm:ss`(`tt`는 오전/오후, UTC).
pub fn ko_kr_time(time: &aws_sdk_s3::primitives::DateTime) -> String {
    utc_time(time).map_or_else(String::new, |t| {
        testrust_common::dotnet_format::ko_kr_datetime(&t)
    })
}

// ---------------------------------------------------------------------------------------------
// Debug 출력 → JSON
// ---------------------------------------------------------------------------------------------

/// 순서를 지키는 JSON 값.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    List(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => match (n.parse::<i64>(), n.parse::<f64>()) {
                (Ok(i), _) => serializer.serialize_i64(i),
                (_, Ok(f)) => serializer.serialize_f64(f),
                _ => serializer.serialize_str(n),
            },
            Self::Text(t) => serializer.serialize_str(t),
            Self::List(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

/// `field_name` → `FieldName`.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}

const REDACTED: &str = "*** Sensitive Data Redacted ***";

struct DebugParser {
    chars: Vec<char>,
    pos: usize,
}

impl DebugParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, text: &str) -> bool {
        let matches = text
            .chars()
            .enumerate()
            .all(|(i, c)| self.chars.get(self.pos + i) == Some(&c));
        if matches {
            self.pos += text.chars().count();
        }
        matches
    }

    fn spaces(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    fn ident(&mut self) -> String {
        let start = self.pos;
        loop {
            if self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                self.pos += 1;
            } else if self.chars.get(self.pos..self.pos + 2) == Some(&[':', ':']) {
                self.pos += 2;
            } else {
                break;
            }
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn value(&mut self) -> Option<Node> {
        self.spaces();
        if self.eat(REDACTED) {
            return Some(Node::Text(REDACTED.to_string()));
        }
        match self.peek()? {
            '"' => self.string().map(Node::Text),
            '\'' => {
                self.pos += 1;
                let c = self.peek()?;
                self.pos += 1;
                self.eat("'").then(|| Node::Text(c.to_string()))
            }
            '[' => {
                self.pos += 1;
                self.items(']').map(Node::List)
            }
            '{' => {
                // HashMap: `{"k": v, ...}`
                self.pos += 1;
                let mut fields = Vec::new();
                loop {
                    self.spaces();
                    if self.eat("}") {
                        break;
                    }
                    let key = match self.value()? {
                        Node::Text(t) | Node::Number(t) => t,
                        other => format!("{other:?}"),
                    };
                    self.spaces();
                    if !self.eat(":") {
                        return Option::None;
                    }
                    let value = self.value()?;
                    fields.push((key, value));
                    self.spaces();
                    self.eat(",");
                }
                Some(Node::Object(fields))
            }
            c if c == '-' || c.is_ascii_digit() => {
                let start = self.pos;
                self.pos += 1;
                while self.peek().is_some_and(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | ':')
                }) {
                    self.pos += 1;
                }
                // 숫자가 아니면(SDK 시각의 `Debug`는 `2023-11-14T22:13:20Z`) 문자열로 둔다.
                let token: String = self.chars[start..self.pos].iter().collect();
                Some(if token.parse::<f64>().is_ok() {
                    Node::Number(token)
                } else {
                    Node::Text(token)
                })
            }
            c if c.is_alphabetic() || c == '_' => {
                let name = self.ident();
                self.spaces();
                if self.peek() == Some('{') {
                    self.pos += 1;
                    self.fields(&name)
                } else if self.peek() == Some('(') {
                    self.pos += 1;
                    let mut items = self.items(')')?;
                    match name.as_str() {
                        "Some" if items.len() == 1 => items.pop(),
                        _ if items.len() == 1 => items.pop(),
                        _ => Some(Node::List(items)),
                    }
                } else {
                    Some(match name.as_str() {
                        "None" => Node::Null,
                        "true" => Node::Bool(true),
                        "false" => Node::Bool(false),
                        _ => Node::Text(name),
                    })
                }
            }
            _ => Option::None,
        }
    }

    fn items(&mut self, close: char) -> Option<Vec<Node>> {
        let mut items = Vec::new();
        loop {
            self.spaces();
            if self.peek() == Some(close) {
                self.pos += 1;
                return Some(items);
            }
            items.push(self.value()?);
            self.spaces();
            self.eat(",");
        }
    }

    /// `Name { field: value, .. }`. 시각(`DateTime { seconds, subsecond_nanos }`)은 ISO 8601 문자열로 바꾼다.
    fn fields(&mut self, name: &str) -> Option<Node> {
        let mut fields = Vec::new();
        loop {
            self.spaces();
            if self.eat("}") {
                break;
            }
            if self.eat("..") {
                continue;
            }
            let field = self.ident();
            self.spaces();
            if !self.eat(":") {
                return Option::None;
            }
            let value = self.value()?;
            fields.push((field, value));
            self.spaces();
            self.eat(",");
        }
        if name == "DateTime" {
            let seconds = fields.iter().find(|(k, _)| k == "seconds");
            let nanos = fields.iter().find(|(k, _)| k == "subsecond_nanos");
            if let (Some((_, Node::Number(s))), Some((_, Node::Number(n)))) = (seconds, nanos) {
                let time = DateTime::<Utc>::from_timestamp(s.parse().ok()?, n.parse().ok()?)?;
                return Some(Node::Text(
                    time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
                ));
            }
        }
        Some(Node::Object(
            fields
                .into_iter()
                .filter(|(k, v)| !k.starts_with('_') && *v != Node::Null)
                .map(|(k, v)| (pascal_case(&k), v))
                .collect(),
        ))
    }

    fn string(&mut self) -> Option<String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let c = self.peek()?;
            self.pos += 1;
            match c {
                '"' => return Some(out),
                '\\' => {
                    let escaped = self.peek()?;
                    self.pos += 1;
                    match escaped {
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        '0' => out.push('\0'),
                        'u' => {
                            // `\u{1f600}`
                            if !self.eat("{") {
                                return Option::None;
                            }
                            let start = self.pos;
                            while self.peek()? != '}' {
                                self.pos += 1;
                            }
                            let hex: String = self.chars[start..self.pos].iter().collect();
                            self.pos += 1;
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Owner {
        display_name: Option<String>,
        id: Option<String>,
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    enum Permission {
        FullControl,
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    struct Grant {
        grantee: Option<Owner>,
        permission: Option<Permission>,
        size: i64,
        tags: Vec<String>,
        _request_id: Option<String>,
    }

    #[test]
    fn debug_to_json() {
        let grants = vec![Grant {
            grantee: Some(Owner {
                display_name: None,
                id: Some("a \"b\"\n".to_string()),
            }),
            permission: Some(Permission::FullControl),
            size: -3,
            tags: vec![],
            _request_id: Some("x".to_string()),
        }];
        let json = dump_json(&grants).replace("\r\n", "\n");
        assert_eq!(
            json,
            "[\n  {\n    \"Grantee\": {\n      \"Id\": \"a \\u0022b\\u0022\\n\"\n    },\n    \"Permission\": \"FullControl\",\n    \"Size\": -3,\n    \"Tags\": []\n  }\n]"
        );
    }

    #[test]
    fn smithy_datetime() {
        let time = aws_sdk_s3::primitives::DateTime::from_secs(1_700_000_000);
        assert_eq!(dump_json(&Some(time)), "\"2023-11-14T22:13:20Z\"");
    }

    #[test]
    fn padding_uses_utf16_width() {
        assert_eq!(pad_right("버킷", 4), "버킷  ");
        assert_eq!(pascal_case("bucket_key_enabled"), "BucketKeyEnabled");
    }
}
