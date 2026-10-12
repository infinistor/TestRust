//! Mono.Options `OptionSet.WriteOptionDescriptions` 형식.
//!
//! 옵션 칸 29자(넘치면 줄을 바꾸고 29칸 들여쓰기), 설명 첫 줄 51자, 다음 줄부터 49자에 31칸 들여쓰기.
//! 너비는 UTF-16 코드 단위로 센다. 줄은 글자·숫자가 아닌 문자 뒤에서 나누고, 단어 중간에서 끊으면 `-`를 붙인다.

use testrust_common::dotnet_json::NEW_LINE;

use super::definitions::OPTIONS;
use super::parser::option_names;

const OPTION_WIDTH: usize = 29;
const DESCRIPTION_FIRST_WIDTH: usize = 80 - OPTION_WIDTH;
const DESCRIPTION_REM_WIDTH: usize = 80 - OPTION_WIDTH - 2;

/// 전체 옵션 설명.
pub fn write_option_descriptions() -> String {
    let mut out = String::new();
    for def in OPTIONS {
        let mut prototype = String::new();
        for (i, name) in option_names(def).enumerate() {
            prototype.push_str(match (i, name.encode_utf16().count()) {
                (0, 1) => "  -",
                (0, _) => "      --",
                (_, 1) => ", -",
                (_, _) => ", --",
            });
            prototype.push_str(name);
        }
        if def.action.takes_value() {
            prototype.push_str("=VALUE");
        }
        let written = prototype.encode_utf16().count();
        out.push_str(&prototype);
        if written < OPTION_WIDTH {
            out.push_str(&" ".repeat(OPTION_WIDTH - written));
        } else {
            out.push_str(NEW_LINE);
            out.push_str(&" ".repeat(OPTION_WIDTH));
        }
        let indent = " ".repeat(OPTION_WIDTH + 2);
        for (i, line) in wrapped_lines(def.description).iter().enumerate() {
            if i > 0 {
                out.push_str(&indent);
            }
            out.push_str(line);
            out.push_str(NEW_LINE);
        }
    }
    out
}

/// `StringCoda.WrappedLines(description, 51, 49)`.
fn wrapped_lines(text: &str) -> Vec<String> {
    let s: Vec<u16> = text.encode_utf16().collect();
    if s.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut width = DESCRIPTION_FIRST_WIDTH;
    let mut start = 0;
    while start < s.len() {
        let mut end = line_end(start, width, &s);
        let correction = if end >= 2 && s[end - 2..end] == [13, 10] {
            2
        } else {
            1
        };
        let c = s[end - correction];
        let whitespace = is_white_space(c);
        if whitespace {
            end -= correction;
        }
        let continuation = end != s.len() && !is_eol_char(c);
        if continuation {
            end -= 1;
        }
        let mut line = String::from_utf16_lossy(&s[start..end]);
        if continuation {
            line.push('-');
        }
        lines.push(line);
        start = end;
        if whitespace {
            start += correction;
        }
        width = DESCRIPTION_REM_WIDTH;
    }
    lines
}

fn line_end(start: usize, length: usize, s: &[u16]) -> usize {
    let end = (start + length).min(s.len());
    let mut sep = None;
    for i in start..end {
        if i + 2 <= s.len() && s[i..i + 2] == [13, 10] {
            return i + 2;
        }
        if s[i] == 10 {
            return i + 1;
        }
        if is_eol_char(s[i]) {
            sep = Some(i + 1);
        }
    }
    match sep {
        Some(sep) if end != s.len() => sep,
        _ => end,
    }
}

/// `!char.IsLetterOrDigit(c)`.
fn is_eol_char(c: u16) -> bool {
    !char::from_u32(u32::from(c)).is_some_and(|c| c.is_alphabetic() || c.is_numeric())
}

/// `char.IsWhiteSpace(c)`.
fn is_white_space(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_some_and(char::is_whitespace)
}
