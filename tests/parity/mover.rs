//! MoverClient·CurlClient가 TESTCore와 같은 요청을 보내고 응답을 같게 처리하는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `mover` 명령으로 만든다(`tests/parity/gen-client-baselines.ps1`).
//! 요청(요청 줄, 헤더, 본문)과 결과(반환값·예외 형식과 메시지)를 비교하며, 정규화는
//! `support/client_parity.rs`를 참고한다(포트, 줄바꿈만 다룬다).
//!
//! - `tests/parity/mover/<이름>.json`: 사례. `variants`가 있으면 응답 본문만 바꿔가며 결과·오류만 비교한다.

#[allow(dead_code)]
#[path = "support/client_parity.rs"]
mod client_parity;
#[allow(dead_code)]
#[path = "support/http_capture.rs"]
mod http_capture;

use client_parity::{
    actual_logs, actual_outcome, actual_requests, case_names, expected_logs, expected_outcome,
    expected_requests, parity_root, read_json, with_logs,
};
use http_capture::{CannedResponse, CaptureServer};
use serde::Serialize;
use serde_json::Value;
use testrust_clients::mover::{MoverClient, MoverError, RequestMoverStart};
use testrust_common::to_dotnet_json;

fn as_json<T: Serialize>(value: Option<T>) -> Value {
    value.map_or(Value::Null, |v| Value::String(to_dotnet_json(&v)))
}

/// 사례를 실행한다. 오라클의 `Mover`와 같은 기본값을 쓴다.
async fn run(spec: &Value, port: u16) -> (Value, Option<(String, String)>) {
    let suffix = spec.get("urlSuffix").and_then(Value::as_str).unwrap_or("");
    let user = spec.get("user").and_then(Value::as_str).unwrap_or("user1");
    let job_id = spec.get("jobId").and_then(Value::as_i64).unwrap_or(0) as i32;
    let client = MoverClient::new(format!("http://127.0.0.1:{port}{suffix}"));
    let outcome: Result<Value, MoverError> = async {
        Ok(match spec["op"].as_str().unwrap() {
            "start" => {
                let request: RequestMoverStart =
                    serde_json::from_value(spec["request"].clone()).unwrap();
                Value::from(client.mover_start(&request).await?)
            }
            "status" => as_json(client.mover_status(user, job_id).await?),
            op => panic!("알 수 없는 op: {op}"),
        })
    }
    .await;
    match outcome {
        Ok(value) => (value, None),
        Err(e) => (Value::Null, Some((e.dotnet_type().into(), e.to_string()))),
    }
}

/// 사례를 한 번 실행한 결과(정규화 전).
struct Run {
    requests: Vec<http_capture::CapturedRequest>,
    result: Value,
    error: Option<(String, String)>,
    logs: Vec<(String, String)>,
}

async fn run_once(spec: &Value, response_body: &str) -> Run {
    let status = spec.get("status").and_then(Value::as_u64).unwrap_or(200) as u16;
    let server = CaptureServer::start(CannedResponse {
        status,
        headers: Vec::new(),
        body: response_body.to_string(),
    })
    .await;
    let port = server.port;
    let ((result, error), logs) = with_logs(run(spec, port)).await;
    Run {
        requests: server.requests(),
        result,
        error,
        logs,
    }
}

#[tokio::test]
async fn mover_client_matches_dotnet() {
    let root = parity_root();
    let mut failures = Vec::new();
    for name in case_names("mover") {
        let spec = read_json(root.join(format!("mover/{name}.json")));
        let expected = read_json(root.join(format!("baseline/mover/{name}.json")));

        if let Some(variants) = expected.get("variants") {
            for variant in variants.as_array().unwrap() {
                let body = variant["responseBody"].as_str().unwrap();
                let run = run_once(&spec, body).await;
                let actual = actual_outcome(run.result, run.error);
                let expected_outcome = expected_outcome(variant);
                let logs = actual_logs(run.logs);
                let expected_logs = expected_logs(variant);
                if actual != expected_outcome || logs != expected_logs {
                    failures.push(format!(
                        "{name} 본문 {body:?}\n  expected {expected_outcome:?} {expected_logs:?}\n  actual   {actual:?} {logs:?}"
                    ));
                }
            }
            continue;
        }

        let body = spec
            .get("responseBody")
            .and_then(Value::as_str)
            .unwrap_or("");
        let run = run_once(&spec, body).await;
        let requests = actual_requests(run.requests);
        let expected_requests = expected_requests(&expected);
        if requests != expected_requests {
            failures.push(format!(
                "{name} 요청\n  expected {expected_requests:?}\n  actual   {requests:?}"
            ));
        }
        let actual = actual_outcome(run.result, run.error);
        let expected_outcome = expected_outcome(&expected);
        if actual != expected_outcome {
            failures.push(format!(
                "{name} 결과\n  expected {expected_outcome:?}\n  actual   {actual:?}"
            ));
        }
        let logs = actual_logs(run.logs);
        let expected_logs = expected_logs(&expected);
        if logs != expected_logs {
            failures.push(format!(
                "{name} 로그\n  expected {expected_logs:?}\n  actual   {logs:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
