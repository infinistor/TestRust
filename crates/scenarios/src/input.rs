//! 여러 명령 모듈이 함께 쓰는 입력 처리: `string.IsNullOrWhiteSpace`, `File.ReadAllText`, 자주 나는 .NET 예외.

use std::io;
use std::path::{Path, PathBuf};

use testrust_s3::S3Error;
use testrust_s3::s3_client::error::full_path;

use crate::ScenarioError;

/// `string.IsNullOrWhiteSpace(value)`.
pub fn blank(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(|v| v.trim().is_empty())
}

/// `Path.GetFullPath(path)`.
pub fn full_path_of(path: &str) -> PathBuf {
    full_path(Path::new(path))
}

/// 파일 입출력 오류를 .NET 예외로 바꾼다. 메시지는 전체 경로를 쓰고, `FileNotFoundException`은
/// `ToString()`처럼 `File name:` 줄이 붙는다.
pub fn io_error(path: &Path, error: &io::Error) -> ScenarioError {
    let full = full_path(path);
    let mapped = S3Error::io(&full, error);
    let mut message = mapped.to_string();
    if mapped.dotnet_type() == "System.IO.FileNotFoundException" {
        message.push_str(&format!("\nFile name: '{}'", full.display()));
    }
    ScenarioError::new(mapped.dotnet_type(), message)
}

/// `File.ReadAllText(path)`: BOM으로 UTF-8/UTF-16/UTF-32를 판별하고 없으면 UTF-8로 읽는다
/// (잘못된 바이트·남는 바이트는 U+FFFD).
pub fn read_all_text(path: &str) -> Result<String, ScenarioError> {
    let full = full_path_of(path);
    let bytes = std::fs::read(&full).map_err(|e| io_error(&full, &e))?;
    Ok(decode_text(&bytes))
}

/// `StreamReader` 기본 인코딩 판별.
pub fn decode_text(bytes: &[u8]) -> String {
    fn utf16(bytes: &[u8], to_u16: fn([u8; 2]) -> u16) -> String {
        let mut text = String::new();
        let units = bytes.as_chunks::<2>().0.iter().map(|pair| to_u16(*pair));
        text.extend(char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')));
        if bytes.len() % 2 == 1 {
            text.push('\u{FFFD}');
        }
        text
    }
    fn utf32(bytes: &[u8], to_u32: fn([u8; 4]) -> u32) -> String {
        bytes
            .chunks(4)
            .map(|quad| match *quad {
                [a, b, c, d] => char::from_u32(to_u32([a, b, c, d])).unwrap_or('\u{FFFD}'),
                _ => '\u{FFFD}',
            })
            .collect()
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE\x00\x00") {
        utf32(rest, u32::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\x00\x00\xFE\xFF") {
        utf32(rest, u32::from_be_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        String::from_utf8_lossy(rest).into_owned()
    } else if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        utf16(rest, u16::from_be_bytes)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `System.Text.Json.JsonException`.
pub fn json_error(error: testrust_common::json::JsonError) -> ScenarioError {
    ScenarioError::new("System.Text.Json.JsonException", error.0)
}

/// `System.NullReferenceException`.
pub fn null_reference() -> ScenarioError {
    ScenarioError::new(
        "System.NullReferenceException",
        "Object reference not set to an instance of an object.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_boms() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFab"), "ab");
        assert_eq!(decode_text(b"\xFF\xFEa\x00b\x00"), "ab");
        assert_eq!(decode_text(b"\xFE\xFF\x00a\x00b"), "ab");
        assert_eq!(decode_text(b"\xFF\xFE\x00\x00a\x00\x00\x00"), "a");
        assert_eq!(decode_text(b"\xFF\xFEa\x00b"), "a\u{FFFD}");
        assert_eq!(decode_text(b"plain"), "plain");
    }
}
