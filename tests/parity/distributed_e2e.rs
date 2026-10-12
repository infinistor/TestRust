//! 분산 실행 혼합 구성 E2E: Worker 2개와 Controller를 실제 프로세스로 띄워 가짜 S3(`support/mock_s3.rs`)에 대해
//! Prepare → Get(ETag 검사) → Put → Mix → Delete를 차례로 실행한다.
//!
//! 구성: Rust 단독(항상), .NET Controller + Rust Worker, Rust Controller + .NET Worker, .NET 단독(`TESTCORE_BIN`이
//! 있을 때). 각 구성에서 종료 코드, 최종 결과 JSON(상태·Worker 수·건수), CSV 머리글을 확인하고, 구성끼리 결과 JSON의
//! 속성 구조와 Prepare 건수·가짜 S3에 남은 객체가 같은지 비교한다.

#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
#[path = "support/http_capture.rs"]
#[allow(dead_code)]
mod http_capture;
#[path = "support/mock_s3.rs"]
#[allow(dead_code)]
mod mock_s3;
#[path = "support/xml_canon.rs"]
#[allow(dead_code)]
mod xml_canon;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use mock_s3::MockS3;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

const TESTS: [(&str, &str); 5] = [
    ("Prepare", "--test-prepare"),
    ("Get", "--test-get"),
    ("Put", "--test-put"),
    ("Mix", "--test-mix"),
    ("Delete", "--test-delete"),
];

const CSV_HEADER: &str = "RunId,SampleId,TestType,Scope,WorkerId,CollectedAtUtc,WorkerSampleAtUtc,ElapsedSeconds,IntervalSeconds,State,Available,IsFinal,Error,ReadSuccess,ReadFailed,ReadOpsPerSecond,WriteSuccess,WriteFailed,WriteOpsPerSecond,HeadSuccess,HeadFailed,HeadOpsPerSecond,DeleteSuccess,DeleteFailed,DeleteOpsPerSecond,ListSuccess,ListFailed,ListOpsPerSecond,EstimatedReadBytesPerSecond,EstimatedWriteBytesPerSecond,ExpectedWorkers,ReportedWorkers";

fn rust_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_TestRust"))
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `GET /driver/status`가 200을 돌려줄 때까지 기다린다.
async fn wait_ready(port: u16) {
    for _ in 0..200 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            let request = format!(
                "GET /driver/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(request.as_bytes()).await.is_ok() {
                let mut response = Vec::new();
                let _ = stream.read_to_end(&mut response).await;
                if response.starts_with(b"HTTP/1.1 200") {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Worker {port}가 준비되지 않았다");
}

struct Run {
    /// 테스트별 최종 결과 JSON.
    reports: Vec<(String, Value)>,
    /// Prepare 뒤 가짜 S3 객체(키, 크기).
    prepared: Vec<(String, usize)>,
}

async fn run_configuration(name: &str, controller: &Path, workers: [&Path; 2]) -> Run {
    let s3 = MockS3::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ports = [free_port(), free_port()];
    let mut children: Vec<Child> = Vec::new();
    for (i, exe) in workers.iter().enumerate() {
        let n = i + 1;
        let ini = root.join(format!("worker{n}.ini"));
        std::fs::write(
            &ini,
            format!(
                "[worker]\nname = driver{n}\nurl = http://127.0.0.1:{}/driver\nWorkPath = work{n}\nResultPath = worker{n}-results\nLeaseTimeoutSeconds = 15\n",
                ports[i]
            ),
        )
        .unwrap();
        children.push(
            Command::new(exe)
                .args(["--worker", "-c"])
                .arg(&ini)
                .current_dir(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
    }
    for port in ports {
        wait_ready(port).await;
    }
    let controller_ini = |file_count: bool| {
        format!(
            "[controller]\nResultPath = results\nLeaseTimeoutSeconds = 15\nPollIntervalSeconds = 1\nRequestTimeoutSeconds = 5\nStartDelaySeconds = 2\nPrepareTimeoutSeconds = 60\ndrivers = 2\n\n[driver1]\nname = driver1\nurl = http://127.0.0.1:{}/driver\n\n[driver2]\nname = driver2\nurl = http://127.0.0.1:{}/driver\n\n[Default]\nBucketName = test-bucket\nThreadPrefix = TH\nObjectPrefix = FILE\nFileSize = 128\n\n[UpDown]\nThreadCount = 1\n{}Times = 1\nBucketType = 4\nReadRatio = 1\nWriteRatio = 1\nETagCheck = true\n\n[Main User]\nURL = {}\nAccessKey = test-access\nSecretKey = test-secret\n",
            ports[0],
            ports[1],
            if file_count { "FileCount = 3\n" } else { "" },
            s3.url
        )
    };
    std::fs::write(root.join("controller-files.ini"), controller_ini(true)).unwrap();
    std::fs::write(root.join("controller.ini"), controller_ini(false)).unwrap();

    let mut reports = Vec::new();
    let mut prepared = Vec::new();
    for (test, flag) in TESTS {
        let ini = if matches!(test, "Prepare" | "Get") {
            "controller-files.ini"
        } else {
            "controller.ini"
        };
        let save = format!("{test}.json");
        let output = tokio::time::timeout(
            Duration::from_secs(90),
            Command::new(controller)
                .args(["--controller", "-c", ini, flag, "-s", &save])
                .current_dir(root)
                .stdin(Stdio::null())
                .output(),
        )
        .await
        .unwrap_or_else(|_| panic!("{name} {test}: 시간 초과"))
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name} {test}: 종료 코드\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(&save)).unwrap()).unwrap();
        let csv = std::fs::read(root.join(format!("{test}.csv"))).unwrap();
        assert!(csv.starts_with(b"\xEF\xBB\xBF"), "{name} {test}: CSV BOM");
        let header = String::from_utf8_lossy(&csv[3..])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        assert_eq!(header, CSV_HEADER, "{name} {test}: CSV 머리글");
        check_report(name, test, &report);
        if test == "Prepare" {
            prepared = s3.objects().into_iter().collect();
        }
        reports.push((test.to_string(), report));
    }
    drop(children);
    Run { reports, prepared }
}

fn count(report: &Value, field: &str) -> i64 {
    report["Total"]["Result"][field].as_i64().unwrap_or(-1)
}

fn check_report(name: &str, test: &str, report: &Value) {
    let context = format!("{name} {test}: {report:#}");
    assert_eq!(report["State"], "Completed", "{context}");
    assert_eq!(report["Complete"], true, "{context}");
    assert_eq!(report["ExpectedWorkers"], 2, "{context}");
    assert_eq!(report["ReportedWorkers"], 2, "{context}");
    assert_eq!(
        report["Workers"].as_array().map(Vec::len),
        Some(2),
        "{context}"
    );
    for field in [
        "ReadFailed",
        "WriteFailed",
        "HeadFailed",
        "DeleteFailed",
        "ListFailed",
    ] {
        assert_eq!(count(report, field), 0, "{field} {context}");
    }
    match test {
        // Worker 2개 × 스레드 1개 × 파일 3개
        "Prepare" => assert_eq!(count(report, "Write"), 6, "{context}"),
        "Get" => assert!(count(report, "Read") > 0, "{context}"),
        "Put" => assert!(count(report, "Write") > 0, "{context}"),
        "Mix" => assert!(
            count(report, "Read") + count(report, "Write") > 0,
            "{context}"
        ),
        _ => {}
    }
}

/// JSON의 속성 경로 구조(값 제외).
fn shape(value: &Value, path: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let p = format!("{path}.{k}");
                out.insert(p.clone());
                shape(v, &p, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                shape(item, &format!("{path}[]"), out);
            }
        }
        _ => {}
    }
}

fn shapes(run: &Run) -> Vec<(String, BTreeSet<String>)> {
    run.reports
        .iter()
        .map(|(test, report)| {
            let mut set = BTreeSet::new();
            shape(report, "", &mut set);
            (test.clone(), set)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn distributed_mixed_configurations() {
    let rust = rust_exe();
    let reference = run_configuration("Rust 단독", &rust, [&rust, &rust]).await;
    let Some(dotnet) = cli_harness::testcore_exe() else {
        eprintln!("TESTCORE_BIN이 없어 .NET 혼합 구성은 건너뛴다");
        return;
    };
    for (name, controller, workers) in [
        (".NET 단독", &dotnet, [&dotnet, &dotnet]),
        (".NET Controller + Rust Worker", &dotnet, [&rust, &rust]),
        ("Rust Controller + .NET Worker", &rust, [&dotnet, &dotnet]),
    ] {
        let run = run_configuration(name, controller, [workers[0], workers[1]]).await;
        assert_eq!(shapes(&run), shapes(&reference), "{name}: 결과 JSON 구조");
        assert_eq!(
            run.prepared, reference.prepared,
            "{name}: Prepare 뒤 S3 객체"
        );
    }
}

/// 성능 비교 묶음(`tools/perf`) 검증용: `MOCK_S3_ADDR`(기본 `0.0.0.0:19000`)에서 가짜 S3를 `MOCK_S3_SECONDS`초(기본 600) 연다.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "수동 실행: 가짜 S3 서버"]
async fn mock_s3_server() {
    let addr = std::env::var("MOCK_S3_ADDR").unwrap_or_else(|_| "0.0.0.0:19000".to_string());
    let seconds: u64 = std::env::var("MOCK_S3_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let s3 = MockS3::start_on(&addr).await;
    eprintln!("가짜 S3: {}", s3.url);
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}
