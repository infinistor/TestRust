//! KsanClient가 TESTCore `KsanClient`와 같은 요청을 보내고 응답을 같게 처리하는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `ksan` 명령으로 만든다. 오라클과 같은 방식으로 로컬 포트에
//! 한 번만 응답하는 서버를 띄워 요청을 기록한다.
//!
//! 실행마다 달라지는 값(포트, `X-Amz-Date`, 서명)은 비교 전에 자리표시자로 바꾼다.
//! 서명 계산 자체는 `parity_sign`에서 확인한다.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use testrust_common::DotnetDateTime;
use testrust_common::dotnet_json::{NEW_LINE, to_dotnet_json};
use testrust_s3::ksan::{KsanClient, KsanError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CASES: &[&str] = &[
    "get-tag-index-ok",
    "get-tag-index-no-status",
    "get-tag-index-no-namespace",
    "get-tag-index-extra",
    "get-tag-index-empty-body",
    "get-tag-index-404-error",
    "get-tag-index-403-empty",
    "get-tag-index-500-text",
    "get-tag-index-400-no-message",
    "get-tag-index-599-empty",
    "delete-tag-index",
    "put-tag-index-ok",
    "put-tag-index-204",
    "put-tag-index-empty-bucket",
    "list-tag-search-ok",
    "list-tag-search-empty",
    "list-tag-search-special-tag",
    "list-tag-search-no-tag",
    "list-tag-search-zero-max",
    "storage-move-ok",
    "storage-move-no-version",
    "storage-move-404",
];

#[derive(Debug, PartialEq, Eq)]
struct Captured {
    line: String,
    headers: BTreeSet<(String, String)>,
    body: String,
}

/// 요청 하나를 받아 기록하고 지정한 응답을 돌려준다.
async fn capture(
    listener: TcpListener,
    status: u16,
    body: String,
) -> Option<(String, Vec<(String, String)>, String)> {
    let (mut socket, _) = listener.accept().await.ok()?;
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        if socket.read(&mut byte).await.ok()? == 0 {
            break;
        }
        buffer.push(byte[0]);
    }
    let head = String::from_utf8(buffer).ok()?;
    let mut lines = head.split("\r\n").filter(|l| !l.is_empty());
    let line = lines.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|h| h.split_once(':'))
        .map(|(n, v)| (n.to_string(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut request_body = vec![0u8; length];
    socket.read_exact(&mut request_body).await.ok()?;
    let response = format!(
        "HTTP/1.1 {status} Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await.ok()?;
    socket.flush().await.ok()?;
    Some((line, headers, String::from_utf8(request_body).ok()?))
}

/// 헤더 이름은 소문자로, 실행마다 달라지는 값은 자리표시자로 바꾼다.
fn normalize(line: &str, headers: &[(String, String)], body: &str, port: u16) -> Captured {
    let headers = headers
        .iter()
        .map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            let value = match name.as_str() {
                "host" => value.replace(&port.to_string(), "<PORT>"),
                "x-amz-date" => "<DATE>".to_string(),
                "authorization" => {
                    let (head, _) = value.split_once("Signature=").expect("서명");
                    let (credential, rest) = head.split_once('/').expect("자격 증명");
                    let (_, scope) = rest.split_once('/').expect("날짜");
                    format!("{credential}/<DATE>/{scope}Signature=<SIG>")
                }
                _ => value.clone(),
            };
            (name, value)
        })
        .collect();
    Captured {
        line: line.to_string(),
        headers,
        body: body.to_string(),
    }
}

async fn run(spec: &Value, client: &KsanClient) -> Result<Option<String>, KsanError> {
    let text = |key: &str| spec.get(key).and_then(Value::as_str);
    let bucket = text("bucket").unwrap_or("");
    Ok(match text("op").unwrap() {
        "get-tag-index" => Some(to_dotnet_json(&client.get_bucket_tag_index(bucket).await?)),
        "delete-tag-index" => {
            client.delete_bucket_tag_index(bucket).await?;
            None
        }
        "put-tag-index" => {
            client.put_bucket_tag_index(bucket).await?;
            None
        }
        "list-tag-search" => {
            let max_keys = spec.get("maxKeys").and_then(Value::as_i64).unwrap_or(1000) as i32;
            let result = client
                .list_bucket_tag_search(bucket, text("tag").unwrap_or(""), max_keys)
                .await?;
            Some(to_dotnet_json(&result))
        }
        "storage-move" => {
            client
                .storage_move(
                    bucket,
                    text("key").unwrap_or(""),
                    text("storageClass").unwrap_or(""),
                    text("versionId"),
                )
                .await?;
            None
        }
        op => panic!("알 수 없는 op: {op}"),
    })
}

/// 기준 출력은 KST(+09:00) PC에서 만들었다. 오프셋이 있는 시각은 현지 시각으로 바뀌므로,
/// 이 PC의 현지 시각 표현으로 바꿔 비교한다.
fn localize(expected: &str) -> String {
    let local = DotnetDateTime::parse_xml("2024-12-31T23:59:59+09:00").unwrap();
    expected
        .replace("\r\n", NEW_LINE)
        .replace("2024-12-31T23:59:59+09:00", &local.to_json_text())
}

#[tokio::test]
async fn ksan_client_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    let read = |path: String| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
    };
    let mut failures = Vec::new();
    for name in CASES {
        let spec = read(format!("ksan/{name}.json"));
        let expected = read(format!("baseline/ksan/{name}.json"));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = spec.get("status").and_then(Value::as_u64).unwrap_or(200) as u16;
        let body = spec
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let server = tokio::spawn(capture(listener, status, body));

        let client = KsanClient::new(
            "127.0.0.1",
            port.into(),
            "AKIAEXAMPLE",
            "secretExample",
            false,
        );
        let outcome = run(&spec, &client).await;
        let captured = match tokio::time::timeout(Duration::from_millis(200), server).await {
            Ok(result) => result.unwrap(),
            Err(_) => None,
        };

        // 요청
        let expected_request = &expected["request"];
        let expected_port = expected["port"].as_u64().unwrap() as u16;
        let expected_captured = expected_request.get("line").map(|line| {
            let headers: Vec<(String, String)> = expected_request["headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| (h[0].as_str().unwrap().into(), h[1].as_str().unwrap().into()))
                .collect();
            normalize(
                line.as_str().unwrap(),
                &headers,
                expected_request["body"].as_str().unwrap(),
                expected_port,
            )
        });
        let actual_captured =
            captured.map(|(line, headers, body)| normalize(&line, &headers, &body, port));
        if actual_captured != expected_captured {
            failures.push(format!(
                "{name} 요청\n  expected {expected_captured:?}\n  actual   {actual_captured:?}"
            ));
        }

        // 결과
        let expected_result = expected["result"].as_str().map(localize);
        let expected_error = expected["error"].as_object().map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
            )
        });
        let (actual_result, actual_error) = match outcome {
            Ok(result) => (result, None),
            Err(e) => (None, Some((e.dotnet_type().to_string(), e.to_string()))),
        };
        if (&actual_result, &actual_error) != (&expected_result, &expected_error) {
            failures.push(format!(
                "{name} 결과\n  expected {expected_result:?} {expected_error:?}\n  actual   {actual_result:?} {actual_error:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
