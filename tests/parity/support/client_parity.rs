//! Portal·Mover·ZeroMQ parity 테스트 공통 도우미: 사례·기준 출력 읽기, 요청·로그·결과 정규화와 비교.
//!
//! 정규화는 실행마다 달라지는 값(포트)과 플랫폼 차이(줄바꿈), 기준 출력을 만든 PC의 시간대에 따라 달라지는
//! 현지 시각 표현만 다룬다.
//!
//! - 헤더 이름은 대소문자까지 비교한다(.NET처럼 `Content-Type` 형식).
//! - 포트: `Host` 헤더, 오류 메시지·로그의 포트 번호를 `<PORT>`로 바꾼다.
//! - 줄바꿈: 기준 출력은 Windows(`\r\n`)에서 만들었다. 요청 본문·결과 문자열의 `\r\n`을 이 플랫폼의 줄바꿈으로
//!   바꾸고, 그에 맞춰 기준의 `Content-Length`를 다시 계산한다.
//! - 시각: 오프셋이 있는 시각은 읽을 때 현지 시각으로 바뀌므로, 기준 출력의 `2024-12-31T23:59:59+09:00`을 이 PC의
//!   현지 표현으로 바꿔 비교한다. 여러 오프셋을 읽는 사례(`variants-*`)는 `RegDate` 값을 `<LOCAL>`로 가린다.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use testrust_common::DotnetDateTime;
use testrust_common::dotnet_json::NEW_LINE;
use tracing_subscriber::fmt::MakeWriter;

use super::http_capture::CapturedRequest;

pub fn parity_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity")
}

pub fn read_json(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `tests/parity/<dir>/*.json`의 사례 이름(파일 이름에서 확장자를 뗀 것), 정렬된 순서.
pub fn case_names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(parity_root().join(dir))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "{dir}에 사례가 없다");
    names
}

/// 정규화한 요청 하나.
#[derive(Debug, PartialEq, Eq)]
pub struct NormRequest {
    pub line: String,
    pub headers: BTreeSet<(String, String)>,
    pub body: String,
}

/// 로컬 서버 주소(`127.0.0.1:포트`)의 포트 번호를 `<PORT>`로 바꾼다.
fn mask_ports(text: &str) -> String {
    const HOST: &str = "127.0.0.1:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(HOST) {
        let after = at + HOST.len();
        out.push_str(&rest[..after]);
        let digits = rest[after..].bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push_str("<PORT>");
        }
        rest = &rest[after + digits..];
    }
    out.push_str(rest);
    out
}

/// 결과 문자열 정규화: 포트를 가리고, `UserData`의 `null` 자격 증명은 빈 문자열로 본다
/// (`testrust_config::UserData`가 `String`이라 `null`을 표현하지 못한다).
fn normalize_result(text: &str) -> String {
    mask_ports(text)
        .replace("\"AccessKey\": null", "\"AccessKey\": \"\"")
        .replace("\"SecretKey\": null", "\"SecretKey\": \"\"")
}
/// 기준 출력의 `requests`.
pub fn expected_requests(expected: &Value) -> Vec<NormRequest> {
    expected["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| {
            let body = request["body"].as_str().unwrap().replace("\r\n", NEW_LINE);
            let headers = request["headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| {
                    let name = h[0].as_str().unwrap().to_string();
                    let value = match name.to_ascii_lowercase().as_str() {
                        "host" => mask_ports(h[1].as_str().unwrap()),
                        "content-length" => body.len().to_string(),
                        _ => h[1].as_str().unwrap().to_string(),
                    };
                    (name, value)
                })
                .collect();
            NormRequest {
                line: request["line"].as_str().unwrap().to_string(),
                headers,
                body,
            }
        })
        .collect()
}

/// 캡처 서버가 받은 요청.
pub fn actual_requests(requests: Vec<CapturedRequest>) -> Vec<NormRequest> {
    requests
        .into_iter()
        .map(|request| NormRequest {
            line: request.line,
            headers: request
                .headers
                .into_iter()
                .map(|(name, value)| {
                    let value = if name.eq_ignore_ascii_case("host") {
                        mask_ports(&value)
                    } else {
                        value
                    };
                    (name, value)
                })
                .collect(),
            body: request.body,
        })
        .collect()
}

/// 호출 결과: 반환값(불리언은 그대로, 객체는 `ToJsonString` 문자열, 없으면 `null`)과 오류(형식 이름, 메시지).
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub result: Value,
    pub error: Option<(String, String)>,
}

/// 기준 출력의 `result`·`error`.
pub fn expected_outcome(expected: &Value) -> Outcome {
    let result = match expected["result"].as_str() {
        Some(text) => Value::String(localize(&normalize_result(&text.replace("\r\n", NEW_LINE)))),
        None => expected["result"].clone(),
    };
    let error = expected["error"].as_object().map(|e| {
        (
            e["type"].as_str().unwrap().to_string(),
            mask_ports(e["message"].as_str().unwrap()),
        )
    });
    Outcome { result, error }
}

/// 실제 호출 결과. 정규화한다.
pub fn actual_outcome(result: Value, error: Option<(String, String)>) -> Outcome {
    Outcome {
        result: match result {
            Value::String(text) => Value::String(normalize_result(&text)),
            other => other,
        },
        error: error.map(|(kind, message)| (kind, mask_ports(&message))),
    }
}

/// 기준 출력은 KST(+09:00) PC에서 만들었다. 오프셋이 있는 시각은 현지 시각으로 바뀌므로 이 PC의
/// 현지 시각 표현으로 바꿔 비교한다.
pub fn localize(expected: &str) -> String {
    let local = DotnetDateTime::parse_xml("2024-12-31T23:59:59+09:00").unwrap();
    expected.replace("2024-12-31T23:59:59+09:00", &local.to_json_text())
}

/// 응답 본문의 `RegDate`에 시간대 오프셋(`+hh`, `-hh:mm`)이 있으면 결과 문자열의 `RegDate` 값을 가린다.
pub fn mask_local_regdate(response_body: &str, text: &str) -> String {
    let Some(start) = response_body.find("\"RegDate\":\"") else {
        return text.to_string();
    };
    let value = &response_body[start + 11..];
    let value = &value[..value.find('"').unwrap_or(value.len())];
    // 날짜 부분(`yyyy-MM-dd`)의 `-`는 오프셋이 아니다.
    let has_offset = value.len() > 10 && value[10..].contains(['+', '-']);
    if !has_offset {
        return text.to_string();
    }
    let key = "\"RegDate\": \"";
    match text.find(key) {
        Some(at) => {
            let from = at + key.len();
            let end = from + text[from..].find('"').unwrap();
            format!("{}<LOCAL>{}", &text[..from], &text[end..])
        }
        None => text.to_string(),
    }
}

pub fn mask_local_regdate_in_outcome(response_body: &str, outcome: Outcome) -> Outcome {
    Outcome {
        result: match outcome.result {
            Value::String(text) => Value::String(mask_local_regdate(response_body, &text)),
            other => other,
        },
        error: outcome.error,
    }
}

// ---- 로그 ----

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> LogBuffer {
        self.clone()
    }
}

/// 퓨처를 실행하면서 `tracing` 로그를 모은다. (수준 이름, 메시지) 목록을 돌려준다.
/// 현재 스레드 런타임(`#[tokio::test]` 기본)에서만 퓨처 안의 로그가 모두 잡힌다.
pub async fn with_logs<T>(future: impl Future<Output = T>) -> (T, Vec<(String, String)>) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let output = future.await;
    drop(guard);
    let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    let logs = text
        .lines()
        .map(|line| {
            let (level, message) = line.trim_start().split_once(' ').unwrap_or((line, ""));
            (level.to_string(), message.to_string())
        })
        .collect();
    (output, logs)
}

/// 기준 출력의 `logs`. 포트는 가린다.
pub fn expected_logs(expected: &Value) -> Vec<(String, String)> {
    expected["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|log| {
            (
                log["level"].as_str().unwrap().to_string(),
                mask_ports(log["message"].as_str().unwrap()),
            )
        })
        .collect()
}

pub fn actual_logs(logs: Vec<(String, String)>) -> Vec<(String, String)> {
    logs.into_iter()
        .map(|(level, message)| (level, mask_ports(&message)))
        .collect()
}
