//! 원본 `Distributed/WorkerHost.cs`: `WorkerManager`(작업 하나씩, RunId 중복 판정, 결과 저장), `WorkerJob`, `WorkerHost`(HTTP).
//!
//! 경로(`/driver`)와 JSON 필드 이름은 .NET Worker와 같다. 요청·응답은 웹 JSON(camelCase), 결과 파일은 PascalCase 들여쓰기다.
//!
//! 원본과 달라지는 점(동작은 같다)
//!
//! - ASP.NET Core 대신 axum으로 같은 여섯 경로를 연다. 오류는 `ArgumentException` 400, `KeyNotFoundException` 404,
//!   `InvalidOperationException` 409, 그 밖은 500 `Worker 내부 오류`(본문 `{"error": 메시지}`). 본문을 읽지 못하면
//!   ASP.NET Core의 모델 바인딩처럼 본문 없는 400이다.
//! - Kestrel은 `localhost`가 아닌 호스트 이름을 모든 인터페이스에 연다. 여기서는 IP 주소와 `localhost`(127.0.0.1)는 그
//!   주소에, 그 밖의 이름은 `0.0.0.0`에 연다.
//! - Worker 작업은 tokio 태스크에서 실행하고, 실행기가 패닉하면 `Worker 실행 오류: Exception`으로 끝낸다.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;
use sha2::{Digest, Sha256};
use testrust_common::{
    DotnetDateTime, DotnetDateTimeOffset, from_web_json, to_dotnet_json, to_web_json,
};
use testrust_scenarios::ScenarioError;
use testrust_scenarios::run_control::RunControl;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::contracts::{RunResult, RunSnapshot, StartRequest, TestRequest, WorkerStatus};
use crate::diagnostics;
use crate::runner::{DistributedTestRunner, RunnerFactory, UpDownRunner, is_canceled, short_type};
use crate::settings::{DistributedSettings, argument};

fn invalid_operation(message: &str) -> ScenarioError {
    ScenarioError::new("System.InvalidOperationException", message)
}

fn key_not_found(message: &str) -> ScenarioError {
    ScenarioError::new("System.Collections.Generic.KeyNotFoundException", message)
}

/// `Guid.TryParseExact(text, "N")`.
fn is_guid_n(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 진단 출력 대상(원본은 `Console.WriteLine`).
pub type DiagnosticsSink = dyn Fn(&str) + Send + Sync;

struct ManagerState {
    jobs: HashMap<String, Arc<WorkerJob>>,
    active: Option<Arc<WorkerJob>>,
}

/// 원본 `WorkerManager`: Worker당 한 작업만 실행하고, RunId별 중복 제출 및 저장된 결과 조회를 관리한다.
pub struct WorkerManager {
    settings: DistributedSettings,
    debug: bool,
    factory: Arc<RunnerFactory>,
    output: Arc<DiagnosticsSink>,
    state: Mutex<ManagerState>,
}

impl WorkerManager {
    /// 원본 `new WorkerManager(settings, factory: null, debug)`: 실제 S3 부하 실행기를 쓴다.
    pub fn new(settings: DistributedSettings, debug: bool) -> Result<Self, ScenarioError> {
        let work_path = settings.work_path.clone();
        Self::with_factory(
            settings,
            Arc::new(move |request, control| {
                Ok(Arc::new(UpDownRunner::new(request, control, &work_path)?)
                    as Arc<dyn DistributedTestRunner>)
            }),
            debug,
        )
    }

    /// 원본 `new WorkerManager(settings, factory, debug)`.
    pub fn with_factory(
        settings: DistributedSettings,
        factory: Arc<RunnerFactory>,
        debug: bool,
    ) -> Result<Self, ScenarioError> {
        std::fs::create_dir_all(&settings.result_path)?;
        std::fs::create_dir_all(&settings.work_path)?;
        Ok(Self {
            settings,
            debug,
            factory,
            output: Arc::new(|text| println!("{text}")),
            state: Mutex::new(ManagerState {
                jobs: HashMap::new(),
                active: None,
            }),
        })
    }

    /// 진단 출력(`--debug`)을 콘솔 대신 `sink`로 보낸다.
    pub fn with_output(mut self, sink: Arc<DiagnosticsSink>) -> Self {
        self.output = sink;
        self
    }

    pub fn settings(&self) -> &DistributedSettings {
        &self.settings
    }

    /// 원본 `Status()`.
    pub fn status(&self) -> WorkerStatus {
        let state = lock(&self.state);
        WorkerStatus {
            name: Some(self.settings.name.clone()),
            available: state.active.as_ref().is_none_or(|job| job.is_completed()),
            run_id: state.active.as_ref().map(|job| job.id.clone()),
            lease_timeout_seconds: self.settings.lease_timeout_seconds,
            uses_local_user: self.settings.local_user.is_some(),
        }
    }

    /// 원본 `Submit(request)`: 요청을 복제하여 로컬 설정을 적용하고 검증한 뒤 비동기 실행을 등록한다.
    pub fn submit(&self, request: &TestRequest) -> Result<RunSnapshot, ScenarioError> {
        // 원본 요청을 유지해야 호출자의 설정을 변경하거나 접미어를 중복 적용하지 않는다.
        let incoming = serde_json::to_string(request).expect("요청 직렬화는 실패하지 않는다");
        let mut frozen: TestRequest =
            serde_json::from_str(&incoming).expect("직렬화한 요청은 다시 읽힌다");
        if let Some(local) = &self.settings.local_user {
            frozen.user = Some(local.clone());
        }
        if !self.settings.bucket_suffix.is_empty()
            && let Some(workload) = frozen.workload.as_mut()
        {
            workload.bucket_name = Some(format!(
                "{}{}",
                workload.bucket_name.as_deref().unwrap_or_default(),
                self.settings.bucket_suffix
            ));
        }
        frozen.validate(true)?;
        if request.worker_id.as_deref() != Some(self.settings.name.as_str()) {
            return Err(argument("Worker 이름 불일치"));
        }
        if request.lease_timeout_seconds > self.settings.lease_timeout_seconds {
            return Err(argument("Worker lease 상한 초과"));
        }
        // 수신한 설정의 해시로 동일 RunId 재전송과 설정을 바꾼 재사용을 구분한다.
        let hash = hex::encode_upper(Sha256::digest(incoming.as_bytes()));
        let run_id = request.run_id.clone().unwrap_or_default();
        let mut state = lock(&self.state);
        if let Some(previous) = state.jobs.get(&run_id) {
            if previous.hash != hash {
                return Err(invalid_operation(
                    "동일 RunId의 설정 변경은 허용하지 않습니다.",
                ));
            }
            return Ok(previous.snapshot());
        }
        if state.active.as_ref().is_some_and(|job| !job.is_completed()) {
            return Err(invalid_operation("Worker가 다른 테스트를 처리 중입니다."));
        }
        let result_path = self.settings.result_path.join(format!("{run_id}.json"));
        // 파일을 먼저 예약하여 재시작 후에도 동일 RunId를 재실행하지 않는다.
        if result_path.exists() {
            return Err(invalid_operation(
                "이미 사용한 RunId입니다. 새 실행 ID가 필요합니다.",
            ));
        }
        if self.debug {
            (self.output)(&diagnostics::request_text(&self.settings, &frozen)?);
        }
        let placeholder = RunSnapshot {
            run_id: request.run_id.clone(),
            worker_id: request.worker_id.clone(),
            test_type: request.test_type.clone(),
            state: Some("Failed".to_string()),
            error: Some("Worker 종료로 완료되지 않은 실행".to_string()),
            sample_at_utc: DotnetDateTimeOffset::now(),
            ..RunSnapshot::default()
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&result_path)?;
        file.write_all(to_dotnet_json(&placeholder).as_bytes())?;
        drop(file);
        let job = Arc::new(WorkerJob::new(
            frozen,
            hash,
            result_path,
            self.factory.clone(),
        ));
        state.active = Some(job.clone());
        state.jobs.insert(run_id, job.clone());
        job.start();
        Ok(job.snapshot())
    }

    fn find(&self, id: &str) -> Result<Arc<WorkerJob>, ScenarioError> {
        if !is_guid_n(id) {
            return Err(argument("RunId 형식 오류"));
        }
        lock(&self.state)
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| key_not_found("실행을 찾을 수 없습니다."))
    }

    /// 원본 `Snapshot(id)`: 메모리의 실행 상태를 우선 조회하고, 재시작 전 작업은 저장된 JSON에서 읽는다.
    pub fn snapshot(&self, id: &str) -> Result<RunSnapshot, ScenarioError> {
        match self.find(id) {
            Ok(job) => Ok(job.snapshot()),
            Err(e) if e.dotnet_type == "System.Collections.Generic.KeyNotFoundException" => {
                let path = self.settings.result_path.join(format!("{id}.json"));
                if !path.exists() {
                    return Err(e);
                }
                let text = std::fs::read_to_string(&path)?;
                serde_json::from_str(&text).map_err(|e| {
                    ScenarioError::new("System.Text.Json.JsonException", e.to_string())
                })
            }
            Err(e) => Err(e),
        }
    }

    /// 원본 `Schedule(id, at)`.
    pub fn schedule(&self, id: &str, at: DotnetDateTimeOffset) -> Result<(), ScenarioError> {
        self.find(id)?.control.schedule(at)
    }

    /// 원본 `Heartbeat(id)`.
    pub fn heartbeat(&self, id: &str) -> Result<(), ScenarioError> {
        self.find(id)?.renew();
        Ok(())
    }

    /// 원본 `Stop(id)`.
    pub fn stop(&self, id: &str) -> Result<(), ScenarioError> {
        self.find(id)?
            .control
            .stop(Some("Controller 중단 요청"), false);
        Ok(())
    }

    /// 원본 `CheckLeases()`.
    pub fn check_leases(&self) {
        let state = lock(&self.state);
        if let Some(active) = &state.active {
            active.check_lease();
        }
    }

    /// 원본 `ShutdownAsync()`: 실행 중인 작업을 멈추고 최대 30초 기다린다.
    pub async fn shutdown(&self) -> Result<(), ScenarioError> {
        let current = lock(&self.state).active.clone();
        let Some(current) = current else {
            return Ok(());
        };
        current.control.stop(Some("Worker 종료"), false);
        match tokio::time::timeout(Duration::from_secs(30), current.wait_completed()).await {
            Ok(()) => Ok(()),
            Err(_) => Err(ScenarioError::new(
                "System.TimeoutException",
                "The operation has timed out.",
            )),
        }
    }
}

struct JobState {
    runner: Option<Arc<dyn DistributedTestRunner>>,
    final_snapshot: Option<RunSnapshot>,
}

/// 원본 `WorkerJob`: 단일 작업의 실행 태스크, heartbeat 유효 기간, 최종 통계와 자원 정리를 소유한다.
struct WorkerJob {
    id: String,
    worker_id: Option<String>,
    test_type: Option<String>,
    file_size: i64,
    thread_count: i32,
    lease_timeout_seconds: i32,
    hash: String,
    result_path: PathBuf,
    factory: Arc<RunnerFactory>,
    /// 실행 태스크가 가져간다(원본 `request.User = null`: 접속 정보는 실행이 끝나면 버려진다).
    request: Mutex<Option<TestRequest>>,
    control: Arc<RunControl>,
    state: Mutex<JobState>,
    /// 시스템 시각 보정에 영향을 받지 않는 단조 시계로 heartbeat 만료를 판단한다.
    last_heartbeat: Mutex<Instant>,
    completion: watch::Sender<bool>,
}

impl WorkerJob {
    fn new(
        request: TestRequest,
        hash: String,
        result_path: PathBuf,
        factory: Arc<RunnerFactory>,
    ) -> Self {
        let workload = request.workload.clone().unwrap_or_default();
        Self {
            id: request.run_id.clone().unwrap_or_default(),
            worker_id: request.worker_id.clone(),
            test_type: request.test_type.clone(),
            file_size: workload.file_size,
            thread_count: workload.thread_count,
            lease_timeout_seconds: request.lease_timeout_seconds,
            hash,
            result_path,
            factory,
            control: Arc::new(RunControl::new(
                usize::try_from(workload.thread_count).unwrap_or_default(),
            )),
            request: Mutex::new(Some(request)),
            state: Mutex::new(JobState {
                runner: None,
                final_snapshot: None,
            }),
            last_heartbeat: Mutex::new(Instant::now()),
            completion: watch::Sender::new(false),
        }
    }

    fn is_completed(&self) -> bool {
        *self.completion.borrow()
    }

    async fn wait_completed(&self) {
        let mut receiver = self.completion.subscribe();
        let _ = receiver.wait_for(|done| *done).await;
    }

    /// 원본 `Start()`.
    fn start(self: &Arc<Self>) {
        let job = self.clone();
        tokio::spawn(async move {
            job.clone().execute().await;
            job.completion.send_replace(true);
        });
    }

    /// 원본 `Renew()`.
    fn renew(&self) {
        *lock(&self.last_heartbeat) = Instant::now();
    }

    /// 원본 `CheckLease()`.
    fn check_lease(&self) {
        if !self.is_completed()
            && lock(&self.last_heartbeat).elapsed().as_secs_f64()
                > f64::from(self.lease_timeout_seconds)
        {
            self.control.stop(Some("Controller heartbeat 만료"), true);
        }
    }

    /// 원본 `Execute()`.
    async fn execute(self: Arc<Self>) {
        let request = lock(&self.request).take();
        if let Some(request) = request {
            let job = self.clone();
            let handle = tokio::spawn(async move {
                let created = (job.factory)(&request, job.control.clone())?;
                lock(&job.state).runner = Some(created.clone());
                created.execute().await
            });
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if is_canceled(&e) && self.control.is_stopped() => {}
                Ok(Err(e)) => self
                    .control
                    .stop(Some(&format!("Worker 실행 오류: {}", short_type(&e))), true),
                Err(_) => self.control.stop(Some("Worker 실행 오류: Exception"), true),
            }
        }
        self.finish();
    }

    /// 원본 `finally`: 신규 요청 발행 중단과 완료를 표시하고, 마지막 통계를 고정한 뒤 실행기를 해제하고(자원 정리
    /// 실패까지 반영) 최종 상태를 파일에 남긴다(TESTCore `ec427f2` 이후 순서: dispose, 파일 저장).
    fn finish(&self) {
        let mut state = lock(&self.state);
        self.control.issuing_stopped();
        self.control.complete();
        // 실행기와 제어 객체를 해제한 뒤에도 조회할 수 있도록 마지막 통계를 고정한다.
        let mut final_snapshot = self.snapshot_core(state.runner.as_deref());
        // 자원 정리 결과까지 반영한 최종 상태를 파일에 남긴다.
        if let Some(runner) = &state.runner
            && runner.dispose().is_err()
        {
            final_snapshot.state = Some("Failed".to_string());
            if final_snapshot.error.is_none() {
                final_snapshot.error = Some("Worker 자원 정리 실패".to_string());
            }
        }
        if let Err(e) = self.write_result(&final_snapshot) {
            final_snapshot.state = Some("Failed".to_string());
            final_snapshot.error = Some(format!(
                "Worker 결과 저장 오류: {}",
                short_type(&ScenarioError::from(e))
            ));
        }
        state.runner = None;
        state.final_snapshot = Some(final_snapshot);
    }

    fn write_result(&self, snapshot: &RunSnapshot) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.result_path)?;
        file.write_all(to_dotnet_json(snapshot).as_bytes())?;
        file.sync_all()
    }

    /// 원본 `SnapshotCore()`.
    fn snapshot_core(&self, runner: Option<&dyn DistributedTestRunner>) -> RunSnapshot {
        // 실행기가 없으면 `new UpDownResult { FileSize, ThreadCount }`(문자열 속성은 `null`).
        let mut result = match runner {
            Some(runner) => RunResult::from(&runner.snapshot()),
            None => RunResult {
                file_size: self.file_size,
                thread_count: self.thread_count,
                ..RunResult::default()
            },
        };
        let control = self.control.apply();
        result.start_time = control
            .started_at_utc
            .map(|at| DotnetDateTime::utc(at.value()))
            .unwrap_or_default();
        result.end_time = control
            .completed_at_utc
            .map(|at| DotnetDateTime::utc(at.value()))
            .unwrap_or_default();
        RunSnapshot {
            run_id: Some(self.id.clone()),
            worker_id: self.worker_id.clone(),
            test_type: self.test_type.clone(),
            state: Some(control.state),
            error: control.error,
            sample_at_utc: control.sample_at_utc,
            scheduled_at_utc: control.scheduled_at_utc,
            started_at_utc: control.started_at_utc,
            issuing_stopped_at_utc: control.issuing_stopped_at_utc,
            completed_at_utc: control.completed_at_utc,
            elapsed_seconds: control.elapsed_seconds,
            result: Some(result),
        }
    }

    /// 원본 `Snapshot()`.
    fn snapshot(&self) -> RunSnapshot {
        let state = lock(&self.state);
        match &state.final_snapshot {
            Some(snapshot) => snapshot.clone(),
            None => self.snapshot_core(state.runner.as_deref()),
        }
    }
}

// ---- HTTP ----

#[derive(Serialize)]
struct ErrorBody<'a> {
    #[serde(rename = "Error")]
    error: &'a str,
}

/// ASP.NET Core의 HTTP JSON 응답은 `JavaScriptEncoder.UnsafeRelaxedJsonEscaping`을 쓴다. 웹 JSON 직렬화(기본 인코더)가 쓴
/// `\uXXXX` 중 큰따옴표만 `\"`로 남기고, 나머지 인쇄 가능한 문자(ASCII·한글 등 BMP 문자)는 원래 글자로 되돌린다.
fn relax_escapes(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut chars = json.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('u') => {
                chars.next();
                let digits: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    Some('"') => out.push_str("\\\""),
                    Some(ch) if !ch.is_control() => out.push(ch),
                    _ => {
                        out.push_str("\\u");
                        out.push_str(&digits);
                    }
                }
            }
            Some(next) => {
                chars.next();
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn json(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        relax_escapes(&body),
    )
        .into_response()
}

/// 원본 오류 처리 미들웨어: 예외 형식에 따라 상태 코드를 정한다.
fn error_response(error: &ScenarioError) -> Response {
    let (status, message) = match error.dotnet_type.as_str() {
        "System.ArgumentException"
        | "System.ArgumentNullException"
        | "System.ArgumentOutOfRangeException" => (StatusCode::BAD_REQUEST, error.message.as_str()),
        "System.Collections.Generic.KeyNotFoundException" => {
            (StatusCode::NOT_FOUND, error.message.as_str())
        }
        "System.InvalidOperationException" => (StatusCode::CONFLICT, error.message.as_str()),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Worker 내부 오류"),
    };
    json(status, to_web_json(&ErrorBody { error: message }))
}

fn respond<T: Serialize>(status: StatusCode, result: Result<T, ScenarioError>) -> Response {
    match result {
        Ok(value) => json(status, to_web_json(&value)),
        Err(e) => error_response(&e),
    }
}

fn empty_ok(result: Result<(), ScenarioError>) -> Response {
    match result {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => error_response(&e),
    }
}

/// 본문 JSON을 읽는다. JSON `null`은 `Ok(None)`(원본은 null 인수가 핸들러까지 간다), 객체가 아니거나 읽지 못하면 본문
/// 없는 400(ASP.NET Core 모델 바인딩 실패)이다.
fn read_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<Option<T>, StatusCode> {
    let text = std::str::from_utf8(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Null) => Ok(None),
        Ok(serde_json::Value::Object(_)) => from_web_json(text)
            .map(Some)
            .map_err(|_| StatusCode::BAD_REQUEST),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

type Manager = State<Arc<WorkerManager>>;

async fn status_handler(State(manager): Manager) -> Response {
    respond(StatusCode::OK, Ok(manager.status()))
}

async fn submit_handler(State(manager): Manager, body: Bytes) -> Response {
    match read_body::<TestRequest>(&body) {
        Ok(Some(request)) => respond(StatusCode::ACCEPTED, manager.submit(&request)),
        Ok(None) => error_response(&ScenarioError::new(
            "System.ArgumentNullException",
            "Value cannot be null. (Parameter 'request')",
        )),
        Err(status) => status.into_response(),
    }
}

async fn snapshot_handler(State(manager): Manager, Path(id): Path<String>) -> Response {
    respond(StatusCode::OK, manager.snapshot(&id))
}

async fn start_handler(State(manager): Manager, Path(id): Path<String>, body: Bytes) -> Response {
    match read_body::<StartRequest>(&body) {
        Ok(Some(request)) => empty_ok(manager.schedule(&id, request.start_at_utc)),
        // `request.StartAtUtc`의 NullReferenceException.
        Ok(None) => error_response(&ScenarioError::new(
            "System.NullReferenceException",
            "Object reference not set to an instance of an object.",
        )),
        Err(status) => status.into_response(),
    }
}

async fn heartbeat_handler(State(manager): Manager, Path(id): Path<String>) -> Response {
    empty_ok(manager.heartbeat(&id))
}

async fn stop_handler(State(manager): Manager, Path(id): Path<String>) -> Response {
    empty_ok(manager.stop(&id))
}

/// 원본 `WorkerHost.CreateApplication`: `/driver` 아래 여섯 경로. 제출은 준비만 시작하고, 실제 부하는 별도 start 요청으로
/// 지정한 시각에 시작한다.
pub fn router(manager: Arc<WorkerManager>) -> Router {
    Router::new()
        .route("/driver/status", get(status_handler))
        .route("/driver/runs", post(submit_handler))
        .route("/driver/runs/{id}", get(snapshot_handler))
        .route("/driver/runs/{id}/start", post(start_handler))
        .route("/driver/runs/{id}/heartbeat", post(heartbeat_handler))
        .route("/driver/runs/{id}/stop", post(stop_handler))
        .with_state(manager)
}

/// `[worker] url`의 권한 부분(`호스트:포트`)에 열 소켓 주소(원본 `new Uri(url).GetLeftPart(Authority)`).
pub fn listen_address(settings: &DistributedSettings) -> Result<SocketAddr, ScenarioError> {
    let uri: http::Uri = settings
        .url
        .parse()
        .map_err(|_| argument("Worker url 형식 오류"))?;
    let default_port = if uri.scheme_str() == Some("https") {
        443
    } else {
        80
    };
    let host = uri.host().unwrap_or_default().trim_matches(['[', ']']);
    let ip = match host.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) if host.eq_ignore_ascii_case("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Err(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    };
    Ok(SocketAddr::new(ip, uri.port_u16().unwrap_or(default_port)))
}

/// `[worker] url`의 주소를 연다.
pub async fn bind(settings: &DistributedSettings) -> Result<TcpListener, ScenarioError> {
    Ok(TcpListener::bind(listen_address(settings)?).await?)
}

/// 원본 `WorkerHost.RunAsync(app, manager)`: heartbeat 감시(250ms)와 HTTP 서버를 돌리고, `shutdown`이 취소되면
/// 서버를 닫은 뒤 진행 중인 작업을 `Stop("Worker 종료")`로 정리한다(최대 30초 대기).
pub async fn serve(
    listener: TcpListener,
    manager: Arc<WorkerManager>,
    shutdown: CancellationToken,
) -> Result<(), ScenarioError> {
    let stopping = shutdown.child_token();
    let monitor = {
        let (manager, stopping) = (manager.clone(), stopping.clone());
        tokio::spawn(async move {
            while !stopping.is_cancelled() {
                manager.check_leases();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    _ = stopping.cancelled() => {}
                }
            }
        })
    };
    let server = axum::serve(listener, router(manager.clone()))
        .with_graceful_shutdown({
            let stopping = stopping.clone();
            async move { stopping.cancelled().await }
        })
        .into_future();
    let mut server = std::pin::pin!(server);
    let result = tokio::select! {
        result = &mut server => result,
        // 진행 중인 요청이 끝나지 않아도 종료를 막지 않는다.
        _ = async {
            stopping.cancelled().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        } => Ok(()),
    };
    stopping.cancel();
    let _ = monitor.await;
    if manager.shutdown().await.is_err() {
        eprintln!("Worker 요청 정리 시간 초과");
    }
    result.map_err(ScenarioError::from)
}

/// 원본 `WorkerHost.RunAsync(settings, debug, ct)`: `[worker] url`의 주소에서 `/driver` API를 연다. `shutdown`이 취소되면
/// 실행 중인 작업을 멈추고(최대 30초 대기) 끝낸다.
pub async fn run_worker(
    settings: DistributedSettings,
    debug: bool,
    shutdown: CancellationToken,
) -> Result<(), ScenarioError> {
    let manager = Arc::new(WorkerManager::new(settings.clone(), debug)?);
    let listener = bind(&settings).await?;
    serve(listener, manager, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_json_keeps_text_readable() {
        let escaped = to_web_json(&ErrorBody {
            error: "Worker가 \"A\" <b>+\\ 입니다.\n",
        });
        assert!(escaped.contains("\\uAC00"));
        assert_eq!(
            relax_escapes(&escaped),
            "{\"error\":\"Worker가 \\\"A\\\" <b>+\\\\ 입니다.\\n\"}"
        );
    }

    #[test]
    fn listen_address_follows_url_authority() {
        let mut settings = DistributedSettings {
            drivers: Vec::new(),
            name: "w".into(),
            url: "http://127.0.0.1:18011/driver".into(),
            work_path: PathBuf::new(),
            result_path: PathBuf::new(),
            base_path: PathBuf::new(),
            bucket_suffix: String::new(),
            local_user: None,
            prepare_timeout_seconds: 300,
            start_delay_seconds: 5,
            request_timeout_seconds: 5,
            poll_interval_seconds: 2,
            lease_timeout_seconds: 15,
        };
        assert_eq!(
            listen_address(&settings).unwrap().to_string(),
            "127.0.0.1:18011"
        );
        settings.url = "http://localhost:80/driver".into();
        assert_eq!(
            listen_address(&settings).unwrap().to_string(),
            "127.0.0.1:80"
        );
        settings.url = "http://worker-host/driver".into();
        assert_eq!(listen_address(&settings).unwrap().to_string(), "0.0.0.0:80");
        settings.url = "http://[::1]:9000/driver".into();
        assert_eq!(listen_address(&settings).unwrap().to_string(), "[::1]:9000");
    }
}
