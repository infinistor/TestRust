//! 분산 Worker(`WorkerManager`·`WorkerHost`·진단·실행기)를 원본 `tests/DistributedChecks/Program.cs`의 Worker 쪽 검사와
//! 같은 순서로 확인한다. 프로토콜 검사는 S3 없이 준비 게이트만 통과하는 가짜 실행기를 쓰고, HTTP는 실제 axum 서버에
//! hyper 클라이언트로 접속한다.
//!
//! `TESTCORE_BIN`(TESTCore 빌드 디렉터리 또는 `TESTCore.exe`)이 있으면 같은 요청을 .NET Worker와 Rust Worker에 보내
//! 상태 코드·본문을 비교한다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use sha2::{Digest, Sha256};
use testrust_common::{DotnetDateTimeOffset, to_web_json};
use testrust_config::UserData;
use testrust_distributed::contracts::{
    RunSnapshot, StartRequest, TestRequest, WorkerStatus, WorkloadSettings,
};
use testrust_distributed::diagnostics;
use testrust_distributed::runner::{
    BoxFuture, DistributedTestRunner, RunnerFactory, UpDownRunner, dataset_identity, dataset_name,
};
use testrust_distributed::settings::DistributedSettings;
use testrust_distributed::worker::{self, WorkerManager};
use testrust_model::UpDownResult;
use testrust_s3::s3_client::error::full_path;
use testrust_scenarios::ScenarioError;
use testrust_scenarios::run_control::RunControl;
use tokio_util::sync::CancellationToken;

const WORKER_INI: &str = "[worker]\nname = driver1\nurl = http://127.0.0.1:0/driver\nWorkPath = work\nResultPath = worker-results\nLeaseTimeoutSeconds = 3\n";
const LOCAL_USER: &str = "\n[Main User]\nURL=http://127.0.0.1:9001\nAccessKey=local-access\nSecretKey=local-secret\nRegionName=\n";

/// 가짜 실행기: 실제 준비 게이트를 통과하고, 선택적으로 중단 요청까지 대기한다(원본 `FakeRunner`).
struct FakeRunner {
    control: Arc<RunControl>,
    wait_for_stop: bool,
    fail_dispose: bool,
    fail_execute: bool,
}

impl FakeRunner {
    fn new(control: Arc<RunControl>) -> Self {
        Self {
            control,
            wait_for_stop: false,
            fail_dispose: false,
            fail_execute: false,
        }
    }
}

impl DistributedTestRunner for FakeRunner {
    fn execute(&self) -> BoxFuture<'_, Result<(), ScenarioError>> {
        Box::pin(async move {
            let control = self.control.clone();
            let thread = tokio::spawn(async move {
                let _ = control.thread_ready_and_wait().await;
            });
            let result = async {
                self.control.ready_and_wait(|| {}).await?;
                if self.fail_execute {
                    return Err(ScenarioError::new("System.IO.IOException", "injected"));
                }
                if self.wait_for_stop {
                    self.control.token().cancelled().await;
                    self.control.throw_if_cancelled()?;
                }
                Ok(())
            }
            .await;
            let _ = thread.await;
            result
        })
    }

    fn snapshot(&self) -> UpDownResult {
        UpDownResult {
            write: 1,
            file_size: 1024,
            ..UpDownResult::default()
        }
    }

    fn dispose(&self) -> Result<(), ScenarioError> {
        if self.fail_dispose {
            Err(ScenarioError::new("System.IO.IOException", "dispose"))
        } else {
            Ok(())
        }
    }
}

fn fake_factory() -> Arc<RunnerFactory> {
    Arc::new(|_, control| Ok(Arc::new(FakeRunner::new(control)) as Arc<dyn DistributedTestRunner>))
}

/// 가짜 실행기를 만들 때마다 받은 요청을 기록한다.
fn recording_factory() -> (
    Arc<RunnerFactory>,
    Arc<Mutex<Option<TestRequest>>>,
    Arc<AtomicUsize>,
) {
    let seen = Arc::new(Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let (s, c) = (seen.clone(), calls.clone());
    let factory: Arc<RunnerFactory> = Arc::new(move |request, control| {
        *s.lock().unwrap() = Some(request.clone());
        c.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeRunner::new(control)) as Arc<dyn DistributedTestRunner>)
    });
    (factory, seen, calls)
}

fn new_guid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "{:032x}",
        (nanos << 16) ^ u128::from(COUNTER.fetch_add(1, Ordering::SeqCst))
    )
}

fn request() -> TestRequest {
    TestRequest {
        run_id: Some(new_guid()),
        worker_id: Some("driver1".into()),
        test_type: Some("Put".into()),
        lease_timeout_seconds: 2,
        user: Some(UserData::new(
            "http://127.0.0.1:9",
            "",
            "test-access",
            "test-secret",
        )),
        workload: Some(WorkloadSettings {
            bucket_name: Some("test-bucket".into()),
            file_size: 1024,
            thread_count: 1,
            times: 1,
            file_count: 3,
            read_ratio: 1,
            write_ratio: 1,
            ..WorkloadSettings::default()
        }),
    }
}

fn id(request: &TestRequest) -> &str {
    request.run_id.as_deref().unwrap()
}

fn write_config(root: &Path, file: &str, content: &str) -> PathBuf {
    let path = root.join(file);
    std::fs::write(&path, content).unwrap();
    path
}

fn load(path: &Path) -> Result<DistributedSettings, ScenarioError> {
    DistributedSettings::load(path.to_str().unwrap(), true)
}

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("조건이 10초 안에 만족되지 않았다");
}

fn state_of(manager: &WorkerManager, request: &TestRequest) -> String {
    manager
        .snapshot(id(request))
        .unwrap()
        .state
        .unwrap_or_default()
}

async fn until_state(manager: &WorkerManager, request: &TestRequest, state: &str) {
    until(|| state_of(manager, request) == state).await;
}

fn manager(settings: &DistributedSettings) -> WorkerManager {
    WorkerManager::with_factory(settings.clone(), fake_factory(), false).unwrap()
}

fn after(milliseconds: i64) -> DotnetDateTimeOffset {
    DotnetDateTimeOffset::new(Utc::now() + chrono::Duration::milliseconds(milliseconds))
}

fn saved(settings: &DistributedSettings, request: &TestRequest) -> String {
    std::fs::read_to_string(settings.result_path.join(format!("{}.json", id(request)))).unwrap()
}

fn saved_state(settings: &DistributedSettings, request: &TestRequest) -> String {
    let value: serde_json::Value = serde_json::from_str(&saved(settings, request)).unwrap();
    value["State"].as_str().unwrap().to_string()
}

fn error_type(result: Result<RunSnapshot, ScenarioError>) -> String {
    result.expect_err("오류여야 한다").dotnet_type
}

const ARGUMENT: &str = "System.ArgumentException";
const INVALID: &str = "System.InvalidOperationException";

/// `BucketSuffix`: 비어 있거나 공백이면 적용하지 않고, 있으면 요청마다 한 번만 붙인다(Thread 형식은 접미어 다음 Worker 이름).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bucket_suffix_variants() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().to_str().unwrap().to_string();
    for suffix_line in [
        "",
        "BucketSuffix =",
        "BucketSuffix =   ",
        "BucketSuffix =  -user1  ",
    ] {
        let file = write_config(
            root.path(),
            "suffix.ini",
            &format!("{WORKER_INI}\n{suffix_line}"),
        );
        let settings = load(&file).unwrap();
        let expected = if suffix_line.contains("-user1") {
            "test-bucket-user1"
        } else {
            "test-bucket"
        };
        let (factory, effective, calls) = recording_factory();
        let manager = WorkerManager::with_factory(settings.clone(), factory, false).unwrap();
        for test_type in ["Prepare", "Put", "Get", "Delete", "Mix"] {
            let mut req = request();
            req.test_type = Some(test_type.into());
            manager.submit(&req).unwrap();
            until_state(&manager, &req, "Ready").await;
            let before = calls.load(Ordering::SeqCst);
            manager.submit(&req).unwrap();
            let seen = effective.lock().unwrap().clone().unwrap();
            let main = seen
                .workload
                .as_ref()
                .unwrap()
                .to_main("driver1", &work)
                .unwrap();
            assert_eq!(main.bucket_name, expected, "{test_type} {suffix_line}");
            assert_eq!(
                req.workload.as_ref().unwrap().bucket_name.as_deref(),
                Some("test-bucket"),
                "들어온 요청은 바뀌지 않는다"
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                before,
                "중복 제출은 다시 실행하지 않는다"
            );
            manager.shutdown().await.unwrap();
        }
        if !settings.bucket_suffix.is_empty() {
            let mut thread_request = request();
            thread_request.workload.as_mut().unwrap().bucket_type = 2;
            manager.submit(&thread_request).unwrap();
            until_state(&manager, &thread_request, "Ready").await;
            let seen = effective.lock().unwrap().clone().unwrap();
            let main = seen
                .workload
                .as_ref()
                .unwrap()
                .to_main("driver1", &work)
                .unwrap();
            assert_eq!(main.bucket_name, "test-bucket-user1-driver1");
            manager.shutdown().await.unwrap();
            let mut too_long = request();
            too_long.workload.as_mut().unwrap().bucket_name = Some("a".repeat(60));
            assert_eq!(
                error_type(manager.submit(&too_long)),
                ARGUMENT,
                "접미어를 붙인 뒤 버킷 길이 검증"
            );
        }
    }
}

/// 서버 종료: Idle·Ready·Running·Completed 어느 상태에서든 깨끗이 끝나고 Cancelled(완료는 Completed)로 저장한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_shutdown_from_each_state() {
    let root = tempfile::tempdir().unwrap();
    let settings = load(&write_config(root.path(), "worker.ini", WORKER_INI)).unwrap();
    for state in ["Idle", "Ready", "Running", "Completed"] {
        let wait_for_stop = state == "Running";
        let manager = Arc::new(
            WorkerManager::with_factory(
                settings.clone(),
                Arc::new(move |_, control| {
                    let mut runner = FakeRunner::new(control);
                    runner.wait_for_stop = wait_for_stop;
                    Ok(Arc::new(runner) as Arc<dyn DistributedTestRunner>)
                }),
                false,
            )
            .unwrap(),
        );
        let listener = worker::bind(&settings).await.unwrap();
        let shutdown = CancellationToken::new();
        let running = tokio::spawn(worker::serve(listener, manager.clone(), shutdown.clone()));
        let req = request();
        if state != "Idle" {
            manager.submit(&req).unwrap();
            until_state(&manager, &req, "Ready").await;
            if state != "Ready" {
                manager.schedule(id(&req), after(50)).unwrap();
                until_state(&manager, &req, state).await;
            }
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .expect("10초 안에 끝나야 한다")
            .unwrap()
            .unwrap();
        assert!(manager.status().available, "{state}");
        if state != "Idle" {
            let expected = if state == "Completed" {
                "Completed"
            } else {
                "Cancelled"
            };
            assert_eq!(saved_state(&settings, &req), expected, "{state}");
        }
    }
}

/// 중복 제출·바쁨·설정 변경 거부, 실행·결과 저장(자격 증명 제외)·재시작 복원, lease 만료, 초기화 실패.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_dedupe_restore_lease_and_failures() {
    let root = tempfile::tempdir().unwrap();
    let worker_file = write_config(root.path(), "worker.ini", WORKER_INI);
    let settings = load(&worker_file).unwrap();
    let fallback = Arc::new(Mutex::new(None::<UserData>));
    let f = fallback.clone();
    let manager = WorkerManager::with_factory(
        settings.clone(),
        Arc::new(move |request, control| {
            *f.lock().unwrap() = request.user.clone();
            Ok(Arc::new(FakeRunner::new(control)) as Arc<dyn DistributedTestRunner>)
        }),
        false,
    )
    .unwrap();
    let req = request();
    manager.submit(&req).unwrap();
    until_state(&manager, &req, "Ready").await;
    assert!(!manager.status().uses_local_user);
    assert_eq!(
        fallback.lock().unwrap().as_ref(),
        req.user.as_ref(),
        "로컬 사용자가 없으면 Controller 접속 정보를 쓴다"
    );
    assert_eq!(manager.status().run_id.as_deref(), Some(id(&req)));
    assert!(!manager.status().available);
    assert_eq!(
        manager.submit(&req).unwrap().run_id.as_deref(),
        Some(id(&req)),
        "중복 제출은 다시 실행하지 않는다"
    );
    assert_eq!(error_type(manager.submit(&request())), INVALID, "바쁨 거부");
    let mut changed = req.clone();
    changed.workload.as_mut().unwrap().file_size += 1;
    assert_eq!(
        error_type(manager.submit(&changed)),
        INVALID,
        "설정 변경 거부"
    );
    manager.schedule(id(&req), after(100)).unwrap();
    until(|| manager.status().available).await;
    assert_eq!(state_of(&manager, &req), "Completed");
    let file = saved(&settings, &req);
    assert!(
        !file.contains("test-secret"),
        "결과 파일에 자격 증명이 없다"
    );
    let value: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(value["IsFinal"], true);
    assert_eq!(value["Result"]["Write"], 1);
    assert_eq!(value["WorkerId"], "driver1");
    assert!(file.contains("\n  \"RunId\""), "PascalCase 들여쓰기");
    let restored = WorkerManager::new(settings.clone(), false).unwrap();
    assert_eq!(
        restored.snapshot(id(&req)).unwrap().state.as_deref(),
        Some("Completed"),
        "재시작 뒤에도 결과가 남는다"
    );
    assert_eq!(
        error_type(restored.submit(&req)),
        INVALID,
        "재시작 뒤 중복 제출 거부"
    );

    // lease 만료
    let mut lease = request();
    lease.lease_timeout_seconds = 1;
    manager.submit(&lease).unwrap();
    until_state(&manager, &lease, "Ready").await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    manager.check_leases();
    until(|| manager.status().available).await;
    let failed = manager.snapshot(id(&lease)).unwrap();
    assert_eq!(failed.state.as_deref(), Some("Failed"));
    assert_eq!(failed.error.as_deref(), Some("Controller heartbeat 만료"));
    assert_eq!(saved_state(&settings, &lease), "Failed");

    // 초기화 실패
    let failed_file = write_config(
        root.path(),
        "failed.ini",
        &WORKER_INI.replace("worker-results", "failed-results"),
    );
    let failed_settings = load(&failed_file).unwrap();
    let failing = WorkerManager::with_factory(
        failed_settings.clone(),
        Arc::new(|_, _| {
            Err(ScenarioError::new(
                "System.IO.IOException",
                "injected initialization failure",
            ))
        }),
        false,
    )
    .unwrap();
    let failing_request = request();
    failing.submit(&failing_request).unwrap();
    until(|| failing.status().available).await;
    let snapshot = failing.snapshot(id(&failing_request)).unwrap();
    assert_eq!(
        snapshot.state.as_deref(),
        Some("Failed"),
        "초기화 실패는 종료 상태이고 Worker를 놓아준다"
    );
    assert_eq!(
        snapshot.error.as_deref(),
        Some("Worker 실행 오류: IOException")
    );

    // 사용자 정보가 없고 로컬 사용자도 없으면 거부
    let mut missing = request();
    missing.user = None;
    assert_eq!(error_type(manager.submit(&missing)), ARGUMENT);
}

/// 실행 오류와 자원 정리 실패는 최종 상태와 결과 파일에 반영된다(TESTCore `ec427f2`: dispose 뒤 파일 저장).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_and_dispose_failures_reach_the_result_file() {
    let root = tempfile::tempdir().unwrap();
    let settings = load(&write_config(root.path(), "worker.ini", WORKER_INI)).unwrap();
    let manager = WorkerManager::with_factory(
        settings.clone(),
        Arc::new(|_, control| {
            let mut runner = FakeRunner::new(control);
            runner.fail_dispose = true;
            Ok(Arc::new(runner) as Arc<dyn DistributedTestRunner>)
        }),
        false,
    )
    .unwrap();
    let req = request();
    manager.submit(&req).unwrap();
    until_state(&manager, &req, "Ready").await;
    manager.schedule(id(&req), after(50)).unwrap();
    until(|| manager.status().available).await;
    let snapshot = manager.snapshot(id(&req)).unwrap();
    assert_eq!(snapshot.state.as_deref(), Some("Failed"));
    assert_eq!(snapshot.error.as_deref(), Some("Worker 자원 정리 실패"));
    assert_eq!(
        saved_state(&settings, &req),
        "Failed",
        "정리 실패가 파일에도 남는다"
    );

    let manager = WorkerManager::with_factory(
        settings.clone(),
        Arc::new(|_, control| {
            let mut runner = FakeRunner::new(control);
            runner.fail_execute = true;
            Ok(Arc::new(runner) as Arc<dyn DistributedTestRunner>)
        }),
        false,
    )
    .unwrap();
    let req = request();
    manager.submit(&req).unwrap();
    until_state(&manager, &req, "Ready").await;
    manager.schedule(id(&req), after(50)).unwrap();
    until(|| manager.status().available).await;
    let snapshot = manager.snapshot(id(&req)).unwrap();
    assert_eq!(snapshot.state.as_deref(), Some("Failed"));
    assert_eq!(
        snapshot.error.as_deref(),
        Some("Worker 실행 오류: IOException")
    );
    assert!(snapshot.is_final());
    assert!(snapshot.started_at_utc.is_some() && snapshot.completed_at_utc.is_some());
}

type Sink = Arc<dyn Fn(&str) + Send + Sync>;

fn capture() -> (Arc<Mutex<String>>, Sink) {
    let output = Arc::new(Mutex::new(String::new()));
    let sink = output.clone();
    (
        output,
        Arc::new(move |text: &str| {
            let mut out = sink.lock().unwrap();
            out.push_str(text);
            out.push('\n');
        }),
    )
}

/// `--debug` 진단: 없으면 조용하고, 있으면 자격 증명과 URL 인증 정보를 가린다. UserSource·유효 버킷도 확인한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_diagnostics_mask_credentials() {
    let root = tempfile::tempdir().unwrap();
    let worker_file = write_config(root.path(), "worker.ini", WORKER_INI);
    let local_file = write_config(
        root.path(),
        "local-user.ini",
        &format!(
            "{}{LOCAL_USER}",
            WORKER_INI.replace("worker-results", "local-results")
        ),
    );
    let worker_settings = load(&worker_file).unwrap();
    let local_settings = load(&local_file).unwrap();
    for (settings, config_path) in [
        (&worker_settings, &worker_file),
        (&local_settings, &local_file),
    ] {
        for debug in [false, true] {
            let (output, sink) = capture();
            let manager = WorkerManager::with_factory(settings.clone(), fake_factory(), debug)
                .unwrap()
                .with_output(sink.clone());
            let mut req = request();
            req.user
                .as_mut()
                .unwrap()
                .set_url("http://url-user:url-pass@127.0.0.1:9/?token=url-token#url-fragment");
            if debug {
                sink(&diagnostics::settings_text(
                    settings,
                    config_path.to_str().unwrap(),
                ));
            }
            manager.submit(&req).unwrap();
            manager.submit(&req).unwrap();
            manager.shutdown().await.unwrap();
            let text = output.lock().unwrap().clone();
            if !debug {
                assert!(text.is_empty(), "debug가 아니면 조용하다");
                continue;
            }
            let marker = "[DEBUG] Worker effective test settings\n";
            let messages: Vec<&str> = text.split(marker).collect();
            assert_eq!(
                messages.len(),
                2,
                "중복 제출은 유효 설정을 다시 출력하지 않는다"
            );
            let startup: serde_json::Value =
                serde_json::from_str(&messages[0][messages[0].find('{').unwrap()..]).unwrap();
            let effective: serde_json::Value = serde_json::from_str(messages[1]).unwrap();
            assert_eq!(
                startup["ConfigPath"].as_str().unwrap(),
                full_path(config_path).to_string_lossy()
            );
            assert_eq!(
                startup["ResultPath"].as_str().unwrap(),
                settings.result_path.to_string_lossy()
            );
            let local = settings.local_user.is_some();
            assert_eq!(
                startup["UserSource"],
                if local {
                    "Worker config.ini"
                } else {
                    "Controller (pending test request)"
                }
            );
            assert_eq!(
                effective["UserSource"],
                if local {
                    "Worker config.ini"
                } else {
                    "Controller"
                }
            );
            assert_eq!(
                effective["User"]["URL"],
                if local {
                    "http://127.0.0.1:9001/"
                } else {
                    "http://127.0.0.1:9/"
                }
            );
            assert_eq!(effective["Workload"]["FileSize"], 1024);
            assert_eq!(effective["EffectiveThreadPrefix"], "TH/driver1");
            assert_eq!(effective["EffectiveBucketName"], "test-bucket");
            for secret in [
                "test-access",
                "test-secret",
                "local-access",
                "local-secret",
                "url-user",
                "url-pass",
                "url-token",
                "url-fragment",
            ] {
                assert!(!text.contains(secret), "{secret}가 출력에 있다");
            }
            assert_eq!(effective["User"]["SecretKey"], "***");
            assert_eq!(effective["User"]["AccessKey"], "***");
        }
    }
}

/// Worker 로컬 `[Main User]`는 Controller가 보낸 접속 정보를 통째로 대신한다(필드를 섞지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_user_overrides_controller_user() {
    let root = tempfile::tempdir().unwrap();
    let local_file = write_config(
        root.path(),
        "local-user.ini",
        &format!(
            "{}{LOCAL_USER}",
            WORKER_INI.replace("worker-results", "local-results")
        ),
    );
    let local_settings = load(&local_file).unwrap();
    let selected = Arc::new(Mutex::new(None::<UserData>));
    let s = selected.clone();
    let manager = WorkerManager::with_factory(
        local_settings.clone(),
        Arc::new(move |request, control| {
            *s.lock().unwrap() = request.user.clone();
            Ok(Arc::new(FakeRunner::new(control)) as Arc<dyn DistributedTestRunner>)
        }),
        false,
    )
    .unwrap();
    let status = manager.status();
    assert!(status.uses_local_user);
    assert!(
        !to_web_json(&status).contains("local-access"),
        "상태에 접속 정보가 없다"
    );
    for incoming in [
        request().user,
        None,
        Some(UserData::new("invalid-url", "remote-region", "", "")),
    ] {
        let mut req = request();
        req.user = incoming.clone();
        manager.submit(&req).unwrap();
        until_state(&manager, &req, "Ready").await;
        assert_eq!(
            selected.lock().unwrap().clone().unwrap(),
            UserData::new("http://127.0.0.1:9001", "", "local-access", "local-secret")
        );
        assert_eq!(
            req.user, incoming,
            "로컬 사용자 적용은 들어온 요청을 바꾸지 않는다"
        );
        manager.schedule(id(&req), after(50)).unwrap();
        until(|| manager.status().available).await;
        assert!(!saved(&local_settings, &req).contains("local-secret"));
    }
    for section in [
        "\n[Main User]\nURL=http://localhost:9000\n".to_string(),
        LOCAL_USER.replace("SecretKey=local-secret", "SecretKey="),
        LOCAL_USER.replace("http://127.0.0.1:9001", "file:///tmp"),
    ] {
        let file = write_config(
            root.path(),
            "invalid-local.ini",
            &format!("{WORKER_INI}{section}"),
        );
        assert_eq!(load(&file).unwrap_err().dotnet_type, ARGUMENT, "{section}");
    }
}

struct Reply {
    status: u16,
    content_type: Option<String>,
    body: String,
}

async fn call(method: &str, url: &str, body: Option<&str>) -> Reply {
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let request = hyper::Request::builder()
        .method(method)
        .uri(url)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.unwrap_or_default().to_string())))
        .unwrap();
    let response = client.request(request).await.unwrap();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        body: String::from_utf8(body.to_vec()).unwrap(),
    }
}

fn error_body(message: &str) -> String {
    // ASP.NET Core 응답은 비 ASCII 문자를 이스케이프하지 않는다.
    format!("{{\"error\":{}}}", serde_json::to_string(message).unwrap())
}

/// 실제 HTTP: 여섯 경로, 상태 코드(202·400·404·409), 오류 본문, 잘못된 JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_api() {
    let root = tempfile::tempdir().unwrap();
    let settings = load(&write_config(root.path(), "worker.ini", WORKER_INI)).unwrap();
    let manager = Arc::new(manager(&settings));
    let listener = worker::bind(&settings).await.unwrap();
    let base = format!("http://{}/driver", listener.local_addr().unwrap());
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(worker::serve(listener, manager.clone(), shutdown.clone()));

    let status = call("GET", &format!("{base}/status"), None).await;
    assert_eq!(status.status, 200);
    assert!(status.content_type.unwrap().starts_with("application/json"));
    assert_eq!(
        status.body,
        "{\"name\":\"driver1\",\"available\":true,\"runId\":null,\"leaseTimeoutSeconds\":3,\"usesLocalUser\":false}"
    );

    let req = request();
    let body = to_web_json(&req);
    let accepted = call("POST", &format!("{base}/runs"), Some(&body)).await;
    assert_eq!(accepted.status, 202);
    let snapshot: RunSnapshot = testrust_common::from_web_json(&accepted.body).unwrap();
    assert_eq!(snapshot.run_id.as_deref(), Some(id(&req)));
    assert_eq!(snapshot.worker_id.as_deref(), Some("driver1"));
    let again = call("POST", &format!("{base}/runs"), Some(&body)).await;
    assert_eq!(again.status, 202, "같은 요청의 재전송");
    let busy = call(
        "POST",
        &format!("{base}/runs"),
        Some(&to_web_json(&request())),
    )
    .await;
    assert_eq!(busy.status, 409);
    assert_eq!(
        busy.body,
        error_body("Worker가 다른 테스트를 처리 중입니다.")
    );
    let mut wrong = request();
    wrong.worker_id = Some("other".into());
    let wrong = call("POST", &format!("{base}/runs"), Some(&to_web_json(&wrong))).await;
    assert_eq!(wrong.status, 400);
    assert_eq!(wrong.body, error_body("Worker 이름 불일치"));
    let null = call("POST", &format!("{base}/runs"), Some("null")).await;
    assert_eq!(null.status, 400);
    assert_eq!(
        null.body,
        error_body("Value cannot be null. (Parameter 'request')")
    );
    for malformed in ["{", "[]", ""] {
        let reply = call("POST", &format!("{base}/runs"), Some(malformed)).await;
        assert_eq!(reply.status, 400, "{malformed:?}");
        assert!(reply.body.is_empty());
    }
    let status = call("GET", &format!("{base}/status"), None).await;
    let status: WorkerStatus = testrust_common::from_web_json(&status.body).unwrap();
    assert!(!status.available);
    assert_eq!(status.run_id.as_deref(), Some(id(&req)));

    let bad_id = call("GET", &format!("{base}/runs/not-a-guid"), None).await;
    assert_eq!(
        (bad_id.status, bad_id.body),
        (400, error_body("RunId 형식 오류"))
    );
    let unknown = call("GET", &format!("{base}/runs/{}", new_guid()), None).await;
    assert_eq!(
        (unknown.status, unknown.body),
        (404, error_body("실행을 찾을 수 없습니다."))
    );
    assert_eq!(
        call("GET", &format!("{base}/nothing"), None).await.status,
        404
    );

    until_state(&manager, &req, "Ready").await;
    let start = |at: DotnetDateTimeOffset| to_web_json(&StartRequest { start_at_utc: at });
    let past = call(
        "POST",
        &format!("{base}/runs/{}/start", id(&req)),
        Some(&start(after(-5000))),
    )
    .await;
    assert_eq!(past.status, 409, "과거 시각 예약");
    let garbage = call(
        "POST",
        &format!("{base}/runs/{}/start", id(&req)),
        Some("{"),
    )
    .await;
    assert_eq!(garbage.status, 400);
    let beat = call("POST", &format!("{base}/runs/{}/heartbeat", id(&req)), None).await;
    assert_eq!((beat.status, beat.body.as_str()), (200, ""));
    let missing = call(
        "POST",
        &format!("{base}/runs/{}/heartbeat", new_guid()),
        None,
    )
    .await;
    assert_eq!(missing.status, 404);
    let scheduled = call(
        "POST",
        &format!("{base}/runs/{}/start", id(&req)),
        Some(&start(after(200))),
    )
    .await;
    assert_eq!((scheduled.status, scheduled.body.as_str()), (200, ""));
    until(|| manager.status().available).await;
    let done = call("GET", &format!("{base}/runs/{}", id(&req)), None).await;
    let done: RunSnapshot = testrust_common::from_web_json(&done.body).unwrap();
    assert_eq!(done.state.as_deref(), Some("Completed"));
    assert_eq!(done.result.unwrap().write, 1);
    let stop = call("POST", &format!("{base}/runs/{}/stop", id(&req)), None).await;
    assert_eq!(stop.status, 200, "끝난 실행의 중단은 아무 일도 하지 않는다");

    // 중단 요청은 Cancelled로 끝난다.
    let second = request();
    call("POST", &format!("{base}/runs"), Some(&to_web_json(&second))).await;
    until_state(&manager, &second, "Ready").await;
    let stop = call("POST", &format!("{base}/runs/{}/stop", id(&second)), None).await;
    assert_eq!(stop.status, 200);
    until(|| manager.status().available).await;
    let cancelled = manager.snapshot(id(&second)).unwrap();
    assert_eq!(cancelled.state.as_deref(), Some("Cancelled"));
    assert_eq!(cancelled.error.as_deref(), Some("Controller 중단 요청"));

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// 데이터셋 디렉터리 이름은 .NET과 같은 식별 JSON의 SHA-256이고, Get+ETagCheck는 원본 파일이 없으면 거부한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dataset_directory_and_etag_source_files() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request();
    req.worker_id = Some("driver1".into());
    req.workload.as_mut().unwrap().bucket_type = 4;
    assert_eq!(
        dataset_identity(&req),
        "{\"WorkerId\":\"driver1\",\"URL\":\"http://127.0.0.1:9\",\"BucketName\":\"test-bucket\",\"ThreadPrefix\":\"TH\",\"ObjectPrefix\":\"FILE\",\"FileSize\":1024,\"BucketType\":4}"
    );
    let expected = hex::encode(Sha256::digest(dataset_identity(&req).as_bytes()));
    assert_eq!(dataset_name(&req), expected);
    assert_eq!(expected, expected.to_lowercase());

    req.test_type = Some("Put".into());
    let control = Arc::new(RunControl::new(1));
    let _runner = UpDownRunner::new(&req, control.clone(), root.path()).unwrap();
    let dataset = root.path().join("datasets").join(&expected);
    assert!(dataset.is_dir());

    req.test_type = Some("Get".into());
    req.workload.as_mut().unwrap().e_tag_check = true;
    let error = UpDownRunner::new(&req, control.clone(), root.path())
        .err()
        .expect("원본 파일이 없다");
    assert_eq!(error.dotnet_type, INVALID);
    std::fs::write(dataset.join("FILE_000"), b"x").unwrap();
    assert!(UpDownRunner::new(&req, control, root.path()).is_ok());
    // RunId가 달라도 같은 데이터셋을 쓴다.
    req.run_id = Some(new_guid());
    assert_eq!(dataset_name(&req), expected);
}

/// `TESTCORE_BIN`: 같은 요청을 .NET Worker와 Rust Worker에 보내 상태 코드와 본문을 비교한다.
fn testcore_exe() -> Option<PathBuf> {
    let bin = PathBuf::from(std::env::var_os("TESTCORE_BIN")?);
    let exe = if bin.is_dir() {
        ["TESTCore.exe", "TestCore.exe", "TestCore", "TESTCore"]
            .iter()
            .map(|name| bin.join(name))
            .find(|path| path.exists())?
    } else {
        bin
    };
    exe.exists().then_some(exe)
}

async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// 값은 빼고 JSON의 속성 이름 구조만 모은다.
fn shape(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                let path = format!("{prefix}/{key}");
                out.push(path.clone());
                shape(item, &path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                shape(item, &format!("{prefix}[]"), out);
            }
        }
        _ => {}
    }
}

fn shape_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        shape(&value, "", &mut out);
    }
    out.sort();
    out
}

struct Kill(std::process::Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 같은 요청 순서를 .NET Worker와 Rust Worker에 보내 상태 코드, 오류 본문, 응답 JSON의 속성 구조를 비교한다. S3는 닫힌
/// 포트라 실행은 곧 실패하거나 중단되므로 최종 상태 값은 비교하지 않고 둘 다 끝나는지만 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dotnet_worker_parity() {
    let Some(exe) = testcore_exe() else {
        eprintln!("TESTCORE_BIN이 없어 .NET Worker 비교를 건너뛴다");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let dotnet_port = free_port().await;
    let dotnet_dir = root.path().join("dotnet");
    std::fs::create_dir_all(&dotnet_dir).unwrap();
    let ini = |port: u16| {
        WORKER_INI
            .replace("127.0.0.1:0", &format!("127.0.0.1:{port}"))
            .replace("LeaseTimeoutSeconds = 3", "LeaseTimeoutSeconds = 30")
    };
    let dotnet_ini = write_config(&dotnet_dir, "worker.ini", &ini(dotnet_port));
    let _dotnet = Kill(
        std::process::Command::new(&exe)
            .args(["--worker", "-c"])
            .arg(&dotnet_ini)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("TESTCore Worker를 시작하지 못했다"),
    );
    let dotnet = format!("http://127.0.0.1:{dotnet_port}/driver");
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
            let uri: hyper::Uri = format!("{dotnet}/status").parse().unwrap();
            if client.get(uri).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect(".NET Worker가 30초 안에 시작하지 않았다");

    let rust_dir = root.path().join("rust");
    std::fs::create_dir_all(&rust_dir).unwrap();
    let rust_settings = load(&write_config(&rust_dir, "worker.ini", &ini(0))).unwrap();
    let rust_manager = Arc::new(WorkerManager::new(rust_settings.clone(), false).unwrap());
    let listener = worker::bind(&rust_settings).await.unwrap();
    let rust = format!("http://{}/driver", listener.local_addr().unwrap());
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(worker::serve(listener, rust_manager, shutdown.clone()));

    let mut req = request();
    req.lease_timeout_seconds = 20;
    req.user = Some(UserData::new("http://127.0.0.1:9", "", "a", "s"));
    let valid = to_web_json(&req);
    let mut other = request();
    other.lease_timeout_seconds = 20;
    let mut wrong_worker = request();
    wrong_worker.worker_id = Some("other".into());
    let mut bad_lease = request();
    bad_lease.lease_timeout_seconds = 31;
    let mut bad_workload = request();
    bad_workload.workload.as_mut().unwrap().thread_count = 0;
    let mut no_user = request();
    no_user.user = None;
    let unknown = format!("/runs/{}", new_guid());
    let run = format!("/runs/{}", id(&req));
    let past = to_web_json(&StartRequest {
        start_at_utc: after(-60_000),
    });
    let steps: Vec<(&str, String, Option<String>)> = vec![
        ("GET", "/status".into(), None),
        ("GET", "/runs/not-a-guid".into(), None),
        ("GET", unknown.clone(), None),
        ("POST", format!("{unknown}/heartbeat"), None),
        ("POST", "/runs".into(), Some("{".into())),
        ("POST", "/runs".into(), Some("null".into())),
        ("POST", "/runs".into(), Some(to_web_json(&wrong_worker))),
        ("POST", "/runs".into(), Some(to_web_json(&bad_lease))),
        ("POST", "/runs".into(), Some(to_web_json(&bad_workload))),
        ("POST", "/runs".into(), Some(to_web_json(&no_user))),
        ("POST", "/runs".into(), Some(valid.clone())),
        ("POST", "/runs".into(), Some(valid.clone())),
        ("POST", "/runs".into(), Some(to_web_json(&other))),
        ("GET", "/status".into(), None),
        ("POST", format!("{run}/start"), Some("{".into())),
        ("POST", format!("{run}/start"), Some("null".into())),
        (
            "POST",
            format!("{run}/start"),
            Some("{\"startAtUtc\":null}".into()),
        ),
        ("POST", format!("{run}/start"), Some(past.clone())),
        ("POST", format!("{run}/heartbeat"), None),
        ("POST", format!("{run}/stop"), None),
    ];
    for (method, path, body) in &steps {
        let a = call(method, &format!("{dotnet}{path}"), body.as_deref()).await;
        let b = call(method, &format!("{rust}{path}"), body.as_deref()).await;
        let label = format!("{method} {path} {}", body.as_deref().unwrap_or(""));
        assert_eq!(
            a.status, b.status,
            ".NET {} / Rust {}: {label}",
            a.status, b.status
        );
        assert_eq!(
            a.content_type.is_some(),
            b.content_type.is_some(),
            "content-type: {label}: .NET {:?} {:?} / Rust {:?} {:?}",
            a.content_type,
            a.body,
            b.content_type,
            b.body
        );
        if a.status >= 400 {
            assert_eq!(a.body, b.body, "오류 본문: {label}");
        } else {
            assert_eq!(
                shape_of(&a.body),
                shape_of(&b.body),
                "응답 구조: {label}\n{}\n{}",
                a.body,
                b.body
            );
        }
    }
    // 둘 다 끝날 때까지 기다린 뒤 최종 응답 구조를 비교한다.
    let mut finals = Vec::new();
    for base in [&dotnet, &rust] {
        let text = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let reply = call("GET", &format!("{base}{run}"), None).await;
                let snapshot: RunSnapshot = testrust_common::from_web_json(&reply.body).unwrap();
                if snapshot.is_final() {
                    break reply.body;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("실행이 끝나지 않았다");
        finals.push(text);
    }
    assert_eq!(
        shape_of(&finals[0]),
        shape_of(&finals[1]),
        "최종 응답 구조\n{}\n{}",
        finals[0],
        finals[1]
    );
    eprintln!(".NET 최종: {}\nRust 최종: {}", finals[0], finals[1]);

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(40), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
