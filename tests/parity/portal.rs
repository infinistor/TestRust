//! PortalManager·KHttpClient가 TESTCore와 같은 요청을 보내고 응답을 같게 처리하는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `portal` 명령으로 만든다(`tests/parity/gen-client-baselines.ps1`).
//! 오라클과 같은 방식으로 로컬 포트에 응답 서버를 띄워 요청(요청 줄, 헤더, 본문)과 결과(반환값·예외 형식과
//! 메시지·오류 로그)를 비교한다.
//!
//! - `tests/parity/portal/<이름>.json`: 사례. `variants`가 있으면 응답 본문만 바꿔가며 결과·오류·로그만 비교한다
//!   (JSON 읽기 경계 사례).
//! - 정규화는 `support/client_parity.rs`를 참고한다(포트, 줄바꿈, 현지 시각만 다룬다).

#[allow(dead_code)]
#[path = "support/client_parity.rs"]
mod client_parity;
#[allow(dead_code)]
#[path = "support/http_capture.rs"]
mod http_capture;

use client_parity::{
    actual_logs, actual_outcome, actual_requests, case_names, expected_logs, expected_outcome,
    expected_requests, mask_local_regdate_in_outcome, parity_root, read_json, with_logs,
};
use http_capture::{CannedResponse, CaptureServer};
use serde::Serialize;
use serde_json::Value;
use testrust_clients::portal::{PortalError, PortalManager};
use testrust_common::to_dotnet_json;
use testrust_config::PortalConfig;

/// 객체 결과는 `ToJsonString` 문자열로, 없으면 `null`.
fn as_json<T: Serialize>(value: Option<T>) -> Value {
    value.map_or(Value::Null, |v| Value::String(to_dotnet_json(&v)))
}

/// 사례를 실행한다. 오라클의 `Portal`과 같은 기본값을 쓴다.
async fn run(spec: &Value, port: u16) -> (Value, Option<(String, String)>) {
    let text = |key: &str, default: &'static str| {
        spec.get(key)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let (volume, user, password, ip) = (
        text("volume", "vol1"),
        text("user", "user1"),
        text("password", "pw"),
        text("ip", "10.0.0.1"),
    );
    let bucket = spec.get("bucket").and_then(Value::as_str);
    let size = spec
        .get("size")
        .and_then(Value::as_u64)
        .unwrap_or(1_000_000_000);

    let url = format!("http://127.0.0.1:{port}{}", text("urlSuffix", ""));
    let config = PortalConfig::new(&url, &text("apiKey", "test-api-key"));
    let manager = match PortalManager::new(config) {
        Ok(manager) => manager,
        Err(e) => return (Value::Null, Some((e.dotnet_type().into(), e.to_string()))),
    };
    let outcome: Result<Value, PortalError> = async {
        Ok(match spec["op"].as_str().unwrap() {
            "health" => Value::Bool(manager.health_check().await?),
            "get-volume" => as_json(manager.get_volume(&volume).await?),
            "create-volume" => {
                manager.create_volume(&volume, size, &password).await?;
                Value::Null
            }
            "start-volume" => {
                manager.start_volume(&volume).await?;
                Value::Null
            }
            "stop-volume" => {
                manager.stop_volume(&volume).await?;
                Value::Null
            }
            "delete-volume" => {
                manager.delete_volume(&volume).await?;
                Value::Null
            }
            "assign-volume" => {
                manager.assign_volume(&volume, &user, size).await?;
                Value::Null
            }
            "get-user" => as_json(manager.get_user(&user).await?),
            "is-user" => Value::Bool(manager.is_user(&user).await?),
            "create-user" => {
                manager.create_user(&volume, &user, size, &password).await?;
                Value::Null
            }
            "get-user-credential" => as_json(manager.get_user_credential(&volume, &user).await?),
            "delete-user" => {
                manager.delete_user(&user).await?;
                Value::Null
            }
            "put-access-ip" => {
                manager.put_access_ip(&volume, &user, &ip, bucket).await?;
                Value::Null
            }
            "delete-access-ip" => {
                manager.delete_access_ip(&volume, &user, bucket).await?;
                Value::Null
            }
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

/// 응답 본문을 지정해 사례를 한 번 실행한다.
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
async fn portal_manager_matches_dotnet() {
    let root = parity_root();
    let mut failures = Vec::new();
    for name in case_names("portal") {
        let spec = read_json(root.join(format!("portal/{name}.json")));
        let expected = read_json(root.join(format!("baseline/portal/{name}.json")));

        if let Some(variants) = expected.get("variants") {
            // 응답 본문만 바꿔가며 결과·오류·로그를 비교한다. 이 메시지들에는 포트가 없다.
            for variant in variants.as_array().unwrap() {
                let body = variant["responseBody"].as_str().unwrap();
                let run = run_once(&spec, body).await;
                let actual =
                    mask_local_regdate_in_outcome(body, actual_outcome(run.result, run.error));
                let expected_outcome =
                    mask_local_regdate_in_outcome(body, expected_outcome(variant));
                let expected_logs = expected_logs(variant);
                let logs = actual_logs(run.logs);
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
