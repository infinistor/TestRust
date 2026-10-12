//! ZeroMqClient가 TESTCore와 같은 프레임을 보내고 응답을 같게 처리하는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `zeromq` 명령으로 만든다(NetMQ REP 서버를 같은 프로세스에 띄워
//! `ZeroMqClient`가 보낸 프레임과 반환값, log4net 로그를 기록한다). 여기서는 같은 일을 순수 Rust
//! `zeromq` 크레이트의 REP 소켓으로 한다.
//!
//! 비교하는 것: 서버가 받은 프레임 문자열, 반환값(0 / -1), 로그(수준과 메시지). 포트는 가린다.
//! 실제 NetMQ와의 상호 운용은 `#[ignore]` 테스트(`interop_*`)로 따로 확인한다. 아래 주석 참고.

#[allow(dead_code)]
#[path = "support/client_parity.rs"]
mod client_parity;
#[allow(dead_code)]
#[path = "support/http_capture.rs"]
mod http_capture;

use ::zeromq::{Endpoint, RepSocket, Socket, SocketRecv, SocketSend};
use client_parity::{actual_logs, case_names, expected_logs, parity_root, read_json, with_logs};
use serde_json::Value;
use testrust_clients::zeromq as client;

/// REP 서버를 띄워 요청 하나를 받고 `reply`로 응답한다. (포트, 받은 프레임 태스크)
async fn start_server(reply: String) -> (u16, tokio::task::JoinHandle<String>) {
    let mut server = RepSocket::new();
    let endpoint = server.bind("tcp://127.0.0.1:0").await.unwrap();
    let port = match endpoint {
        Endpoint::Tcp(_, port) => port,
        other => panic!("예상하지 못한 엔드포인트: {other}"),
    };
    let task = tokio::spawn(async move {
        let message = server.recv().await.unwrap();
        let received = String::from_utf8_lossy(message.get(0).unwrap()).into_owned();
        server.send(reply.into()).await.unwrap();
        // 응답이 전송될 때까지 소켓을 유지한다.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        received
    });
    (port, task)
}

#[tokio::test]
async fn zeromq_client_matches_dotnet() {
    let root = parity_root();
    let mut failures = Vec::new();
    for name in case_names("zeromq") {
        let spec = read_json(root.join(format!("zeromq/{name}.json")));
        let expected = read_json(root.join(format!("baseline/zeromq/{name}.json")));
        let text = |key: &str, default: &'static str| {
            spec.get(key)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        let service_type = text("serviceType", "svc");
        let (port, server) = start_server(text("reply", "OK!")).await;

        let (code, logs) = with_logs(async {
            match spec["op"].as_str().unwrap() {
                "pause" => client::pause(&service_type, "127.0.0.1", port.into()).await,
                "resume" => client::resume(&service_type, "127.0.0.1", port.into()).await,
                op => panic!("알 수 없는 op: {op}"),
            }
        })
        .await;
        let received = server.await.unwrap();

        let actual = (received, code.map_err(|e| e.to_string()), actual_logs(logs));
        let expected_value = (
            expected["received"].as_str().unwrap().to_string(),
            Ok(expected["result"].as_i64().unwrap() as i32),
            expected_logs(&expected),
        );
        if actual != expected_value {
            failures.push(format!(
                "{name}\n  expected {expected_value:?}\n  actual   {actual:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 서버가 아직 없으면 연결될 때까지 기다린다(NetMQ `Connect`는 비동기로 재시도하고 `ReceiveFrameString`은 응답이 올
/// 때까지 막힌다). 호출자는 퓨처를 버려서 취소한다.
#[tokio::test]
async fn pause_waits_until_server_appears() {
    // 비어 있는 포트를 골라 둔다.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let call = tokio::spawn(async move { client::pause("svc", "127.0.0.1", port.into()).await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!call.is_finished(), "서버가 없는데 끝났다");

    let mut server = RepSocket::new();
    server
        .bind(&format!("tcp://127.0.0.1:{port}"))
        .await
        .unwrap();
    let message = tokio::time::timeout(std::time::Duration::from_secs(10), server.recv())
        .await
        .expect("클라이언트가 다시 연결해야 한다")
        .unwrap();
    assert_eq!(message.get(0).unwrap().as_ref(), b"svc.pause");
    server.send("OK!".into()).await.unwrap();
    let code = tokio::time::timeout(std::time::Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(code, 0);
}
// ---- NetMQ 상호 운용(수동 확인) ----
//
// 순수 Rust `zeromq`와 .NET NetMQ가 서로 통신하는지 실제 오라클 프로세스로 확인한다. dotnet과 빌드한 오라클이
// 필요해서 기본 실행에서는 건너뛴다. 아래처럼 직접 실행한다.
//
//   dotnet build tools/dotnet-oracle -p:TestCoreBin=E:\Code\Git\TESTCore\bin\TestCore
//   $env:TESTCORE_BIN = "E:\Code\Git\TESTCore\bin\TestCore"
//   cargo test -p testrust-clients --test parity_zeromq -- --ignored

fn oracle_command() -> tokio::process::Command {
    let dll = parity_root().join("../../tools/dotnet-oracle/bin/Debug/net10.0/DotnetOracle.dll");
    let mut command = tokio::process::Command::new("dotnet");
    command
        .arg(dll)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Rust REQ 클라이언트(`ZeroMqClient`)가 NetMQ REP 서버와 통신한다.
#[tokio::test]
#[ignore = "dotnet과 빌드한 오라클이 필요하다"]
async fn interop_rust_client_to_netmq_server() {
    use tokio::io::{AsyncBufReadExt, BufReader};

    for (op, reply, expected_code) in [("pause", "OK!", 0), ("resume", "NO", -1)] {
        let mut server = oracle_command()
            .args(["zeromq-serve", reply])
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
        let first = lines.next_line().await.unwrap().expect("PORT 줄");
        let port: i32 = first.strip_prefix("PORT ").expect(&first).parse().unwrap();

        let code = match op {
            "pause" => client::pause("svc", "127.0.0.1", port).await,
            _ => client::resume("svc", "127.0.0.1", port).await,
        }
        .unwrap();
        assert_eq!(code, expected_code);

        let mut output = String::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            output.push_str(&line);
        }
        assert!(
            output.contains(&format!("\"received\": \"svc.{op}\"")),
            "{output}"
        );
        server.wait().await.unwrap();
    }
}

/// NetMQ REQ 클라이언트(.NET `ZeroMqClient`)가 Rust REP 서버와 통신한다.
#[tokio::test]
#[ignore = "dotnet과 빌드한 오라클이 필요하다"]
async fn interop_netmq_client_to_rust_server() {
    use tokio::io::AsyncReadExt;

    for (op, reply, expected_code) in [("pause", "OK!", 0), ("resume", "NO", -1)] {
        let (port, server) = start_server(reply.to_string()).await;
        let mut child = oracle_command()
            .args(["zeromq-call", op, "svc", "127.0.0.1", &port.to_string()])
            .spawn()
            .unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .await
            .unwrap();
        child.wait().await.unwrap();

        assert_eq!(server.await.unwrap(), format!("svc.{op}"));
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["result"], expected_code, "{output}");
    }
}
