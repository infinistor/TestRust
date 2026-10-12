//! 원본 `Distributed/Controller.cs`: Worker 상태 확인, 제출, 하트비트, 폴링, 예약 시작, 정리.
//!
//! 통신은 Worker `/driver` HTTP API(JSON camelCase)다. 오류 문구는 원본의 .NET 예외 형식 이름(`HttpRequestException`,
//! `TaskCanceledException`, `JsonException`)을 그대로 쓴다(`Worker 조회 실패: {형식}`, `Controller 오류: {형식}`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::Method;
use hyper_rustls::HttpsConnectorBuilder;
use serde::Serialize;
use serde::de::DeserializeOwned;
use testrust_common::{DotnetDateTimeOffset, from_web_json, to_web_json};
use testrust_config::Config;
use testrust_s3::DotnetUri;
use testrust_s3::http_transport::HttpTransport;
use testrust_scenarios::ScenarioError;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::contracts::{RunOptions, RunSnapshot, StartRequest, TestRequest, WorkerStatus};
use crate::result_writer::{ResultSettings, ResultWriter, WorkerSample};
use crate::settings::{DistributedSettings, DriverSettings};

/// Worker 요청 한 건의 실패 원인(.NET 예외 형식으로 구분한다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// `HttpRequestException`: 연결 실패, 성공이 아닌 상태 코드, 식별 오류.
    Http(String),
    /// `TaskCanceledException`: 요청 제한 시간 또는 취소.
    Canceled,
    /// `JsonException`.
    Json(String),
}

impl NetError {
    /// `Exception.GetType().Name`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Http(_) => "HttpRequestException",
            Self::Canceled => "TaskCanceledException",
            Self::Json(_) => "JsonException",
        }
    }
}

/// `Task.WhenAll` 안에서 던져지는 예외.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ControllerError {
    /// `InvalidOperationException`(메시지를 그대로 보여 준다).
    Invalid(String),
    /// `TimeoutException`(메시지를 그대로 보여 준다).
    Timeout(&'static str),
    /// `OperationCanceledException`.
    Canceled,
    /// 그 밖의 예외(`GetType().Name`).
    Other(String),
}

impl From<NetError> for ControllerError {
    fn from(error: NetError) -> Self {
        match error {
            NetError::Canceled => Self::Canceled,
            other => Self::Other(other.type_name().to_string()),
        }
    }
}

impl From<ScenarioError> for ControllerError {
    fn from(error: ScenarioError) -> Self {
        let name = error.dotnet_type.rsplit('.').next().unwrap_or_default();
        Self::Other(name.to_string())
    }
}

/// 원본 `HttpClient { Timeout = RequestTimeoutSeconds }`.
struct Http {
    transport: HttpTransport,
    timeout: Duration,
}

impl Http {
    fn new(timeout: Duration) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder =
            match HttpsConnectorBuilder::new().with_provider_and_native_roots(provider.clone()) {
                Ok(builder) => builder,
                Err(_) => HttpsConnectorBuilder::new().with_tls_config(
                    rustls::ClientConfig::builder_with_provider(provider)
                        .with_safe_default_protocol_versions()
                        .expect("기본 프로토콜 버전")
                        .with_root_certificates(rustls::RootCertStore::empty())
                        .with_no_client_auth(),
                ),
            };
        let connector = builder.https_or_http().enable_http1().build();
        Self {
            transport: HttpTransport::new(connector, true),
            timeout,
        }
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<String>,
        token: &CancellationToken,
    ) -> Result<testrust_s3::http_transport::HttpResponse, NetError> {
        let uri =
            DotnetUri::parse(url).map_err(|_| NetError::Http(format!("잘못된 주소: {url}")))?;
        let host = if uri.is_default_port() {
            uri.host().to_string()
        } else {
            format!("{}:{}", uri.host(), uri.port())
        };
        let mut headers = vec![("Host", host.as_str())];
        let content = match &body {
            Some(text) => {
                headers.push(("Content-Type", "application/json; charset=utf-8"));
                Bytes::from(text.clone())
            }
            None => Bytes::new(),
        };
        let work = self.transport.send(method, &uri, headers, content);
        tokio::select! {
            biased;
            () = token.cancelled() => Err(NetError::Canceled),
            result = tokio::time::timeout(self.timeout, work) => match result {
                Err(_) => Err(NetError::Canceled),
                Ok(Err(e)) => Err(NetError::Http(format!("{e:?}"))),
                Ok(Ok(response)) => Ok(response),
            },
        }
    }

    /// `GetFromJsonAsync<T>`: 성공이 아닌 상태는 `HttpRequestException`, `null` 본문은 `None`.
    async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        token: &CancellationToken,
    ) -> Result<Option<T>, NetError> {
        let response = self.send(Method::GET, url, None, token).await?;
        if !(200..300).contains(&response.status) {
            return Err(NetError::Http(format!("상태 코드 {}", response.status)));
        }
        let value: serde_json::Value =
            serde_json::from_str(&response.body).map_err(|e| NetError::Json(e.to_string()))?;
        if value.is_null() {
            return Ok(None);
        }
        from_web_json(&response.body)
            .map(Some)
            .map_err(|e| NetError::Json(e.to_string()))
    }

    /// `PostAsJsonAsync`: 응답 상태 코드를 돌려준다.
    async fn post<T: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
        token: &CancellationToken,
    ) -> Result<u16, NetError> {
        Ok(self
            .send(Method::POST, url, Some(to_web_json(body)), token)
            .await?
            .status)
    }

    /// `PostAsJsonAsync` + `EnsureSuccessStatusCode`.
    async fn post_ensure<T: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
        token: &CancellationToken,
    ) -> Result<(), NetError> {
        let status = self.post(url, body, token).await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(NetError::Http(format!("상태 코드 {status}")))
        }
    }
}

/// `new { }`.
#[derive(Serialize)]
struct Empty {}

/// 실행 동안 하트비트·폴링이 함께 쓰는 상태.
struct Context {
    settings: DistributedSettings,
    run_id: String,
    http: Http,
    /// `submitted`: 응답이 유실되어도 중단 대상에 포함한다.
    submitted: Mutex<Vec<String>>,
    /// 마지막으로 통계를 받은 시각(연결 제한 시간 판정).
    last_seen: Mutex<HashMap<String, Instant>>,
}

impl Context {
    fn is_submitted(&self, name: &str) -> bool {
        self.submitted.lock().unwrap().iter().any(|n| n == name)
    }

    fn submitted_drivers(&self) -> Vec<DriverSettings> {
        self.settings
            .drivers
            .iter()
            .filter(|d| self.is_submitted(&d.name))
            .cloned()
            .collect()
    }

    fn touch(&self, name: &str) {
        self.last_seen
            .lock()
            .unwrap()
            .insert(name.to_string(), Instant::now());
    }

    fn silent_for(&self, name: &str) -> Duration {
        self.last_seen
            .lock()
            .unwrap()
            .get(name)
            .map_or(Duration::ZERO, Instant::elapsed)
    }

    /// 조회 실패를 누락 표본으로 반환하여 다른 Worker의 통계와 기존 누적값을 보존한다.
    async fn poll(self: &Arc<Self>, token: &CancellationToken) -> Vec<WorkerSample> {
        let tasks = self.settings.drivers.iter().cloned().map(|d| {
            let ctx = self.clone();
            let token = token.clone();
            async move {
                match ctx.fetch_snapshot(&d, &token).await {
                    Ok(snapshot) => WorkerSample::new(d.name, Some(snapshot)),
                    Err(e) => {
                        WorkerSample::failed(d.name, format!("Worker 조회 실패: {}", e.type_name()))
                    }
                }
            }
        });
        join_all(tasks).await
    }

    async fn fetch_snapshot(
        &self,
        d: &DriverSettings,
        token: &CancellationToken,
    ) -> Result<RunSnapshot, NetError> {
        let url = format!("{}/runs/{}", d.url, self.run_id);
        let snapshot: Option<RunSnapshot> = self.http.get_json(&url, token).await?;
        match snapshot {
            Some(s)
                if s.run_id.as_deref() == Some(self.run_id.as_str())
                    && s.worker_id.as_deref() == Some(d.name.as_str())
                    && s.result.is_some() =>
            {
                self.touch(&d.name);
                Ok(s)
            }
            _ => Err(NetError::Http("Worker 응답 식별 오류".to_string())),
        }
    }
}

/// `Task.WhenAll`: 모두 끝날 때까지 기다리고 입력 순서대로 돌려준다.
async fn join_all<T, F>(futures: impl IntoIterator<Item = F>) -> Vec<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let mut set = JoinSet::new();
    for (index, future) in futures.into_iter().enumerate() {
        set.spawn(async move { (index, future.await) });
    }
    let mut done = Vec::new();
    while let Some(result) = set.join_next().await {
        done.push(result.expect("작업이 중단되었습니다"));
    }
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, value)| value).collect()
}

/// 통계 조회나 파일 저장이 지연되어도 Worker의 실행 허가(lease)는 별도로 갱신한다.
fn spawn_heartbeat(ctx: Arc<Context>, stop: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while !stop.is_cancelled() {
            let tasks = ctx.submitted_drivers().into_iter().map(|d| {
                let ctx = ctx.clone();
                let stop = stop.clone();
                async move {
                    // Poll 단계에서 연결 실패와 lease 만료를 처리하므로 heartbeat 오류는 무시한다.
                    let url = format!("{}/runs/{}/heartbeat", d.url, ctx.run_id);
                    let _ = ctx.http.post(&url, &Empty {}, &stop).await;
                }
            });
            join_all(tasks).await;
            let delay = Duration::from_secs(ctx.settings.poll_interval_seconds as u64);
            tokio::select! {
                () = stop.cancelled() => break,
                () = tokio::time::sleep(delay) => {}
            }
        }
    })
}

/// 원본 `Controller.RunAsync`: 종료 코드(0 성공, 1 실패·중단)를 돌려준다. 시작 전 설정 오류와 결과 파일 오류는
/// `Err`(최상위가 `분산 실행 오류`로 보여 준다).
pub async fn run_async(
    settings: &DistributedSettings,
    config: &Config,
    options: &RunOptions,
    save: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<i32, ScenarioError> {
    run_with_cleanup_budget(
        settings,
        config,
        options,
        save,
        cancellation,
        CLEANUP_BUDGET,
    )
    .await
}

/// 실패·중단 뒤 Worker 중단 요청과 마지막 통계 수집에 쓰는 제한 시간(원본 30초).
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(30);

/// [`run_async`]에서 정리 제한 시간을 정할 수 있는 형태(시험용).
#[doc(hidden)]
pub async fn run_with_cleanup_budget(
    settings: &DistributedSettings,
    config: &Config,
    options: &RunOptions,
    save: Option<&str>,
    cancellation: &CancellationToken,
    cleanup_budget: Duration,
) -> Result<i32, ScenarioError> {
    const COMPLETED: &str = "Completed";
    let run_id = uuid_n();
    let mut requests = Vec::new();
    for d in &settings.drivers {
        requests.push(TestRequest::from_config(
            config,
            options,
            &run_id,
            &d.name,
            settings.lease_timeout_seconds,
        )?);
    }
    for request in &requests {
        request.validate(false)?;
    }
    let test_type = requests[0].test_type.clone().unwrap_or_default();
    let mut writer = ResultWriter::new(settings, save, &run_id, &test_type)?;
    let ctx = Arc::new(Context {
        settings: settings.clone(),
        run_id: run_id.clone(),
        http: Http::new(Duration::from_secs(settings.request_timeout_seconds as u64)),
        submitted: Mutex::new(Vec::new()),
        last_seen: Mutex::new(
            settings
                .drivers
                .iter()
                .map(|d| (d.name.clone(), Instant::now()))
                .collect(),
        ),
    });
    let heartbeat_stop = CancellationToken::new();
    let heartbeat = spawn_heartbeat(ctx.clone(), heartbeat_stop.clone());
    let mut state = "Failed";
    let mut error: Option<String> = None;
    let mut csv_healthy = true;

    let flow = drive(
        &ctx,
        &mut requests,
        &mut writer,
        &mut csv_healthy,
        cancellation,
    )
    .await;
    match flow {
        Ok(()) => state = COMPLETED,
        Err(ControllerError::Canceled) if cancellation.is_cancelled() => {
            state = "Cancelled";
            error = Some("사용자 중단".to_string());
        }
        Err(e) => {
            let message = match e {
                ControllerError::Invalid(message) => message,
                ControllerError::Timeout(message) => message.to_string(),
                ControllerError::Canceled => "Controller 오류: TaskCanceledException".to_string(),
                ControllerError::Other(name) => format!("Controller 오류: {name}"),
            };
            eprintln!("{message}");
            error = Some(message);
        }
    }

    if state != COMPLETED && !ctx.submitted.lock().unwrap().is_empty() {
        cleanup(&ctx, &mut writer, &mut csv_healthy, cleanup_budget).await;
    }
    heartbeat_stop.cancel();
    let _ = heartbeat.await;
    let result_settings = ResultSettings::new(requests[0].workload.clone(), settings);
    writer.save_final(state, error.as_deref(), Some(&result_settings))?;
    println!(
        "JSON: {}\nCSV: {}",
        writer.json_path().display(),
        writer.csv_path().display()
    );
    Ok(if state == COMPLETED { 0 } else { 1 })
}

/// 사용자 취소 토큰과 분리해야 Ctrl+C 이후에도 중단 요청과 마지막 통계를 수집할 수 있다.
async fn cleanup(
    ctx: &Arc<Context>,
    writer: &mut ResultWriter,
    csv_healthy: &mut bool,
    budget: Duration,
) {
    let token = CancellationToken::new();
    let timer = {
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(budget).await;
            token.cancel();
        })
    };
    let stops = ctx.submitted_drivers().into_iter().map(|d| {
        let ctx = ctx.clone();
        let token = token.clone();
        async move {
            // 제한 시간 내 최종 상태 수집을 계속하기 위해 개별 stop 요청 오류는 무시한다.
            let url = format!("{}/runs/{}/stop", d.url, ctx.run_id);
            let _ = ctx.http.post(&url, &Empty {}, &token).await;
        }
    });
    join_all(stops).await;
    while !token.is_cancelled() {
        let samples = ctx.poll(&token).await;
        // 정리 단계의 결과 파일 오류가 기존 실행 결과를 덮어쓰지 않게 한다.
        if sample(writer, csv_healthy, &samples).is_err() {
            break;
        }
        if samples
            .iter()
            .all(|s| s.snapshot.as_ref().is_some_and(RunSnapshot::is_final))
        {
            break;
        }
        tokio::select! {
            () = token.cancelled() => break,
            () = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
    timer.abort();
}

fn sample(
    writer: &mut ResultWriter,
    csv_healthy: &mut bool,
    samples: &[WorkerSample],
) -> Result<(), ControllerError> {
    writer.observe(samples);
    if !*csv_healthy {
        return Ok(());
    }
    writer
        .sample(samples, DotnetDateTimeOffset::now())
        .map_err(|e| {
            *csv_healthy = false;
            e.into()
        })
}

fn check_cancelled(token: &CancellationToken) -> Result<(), ControllerError> {
    if token.is_cancelled() {
        Err(ControllerError::Canceled)
    } else {
        Ok(())
    }
}

/// `try { ... }` 본문: 상태 확인, 제출, 폴링, 예약 시작.
async fn drive(
    ctx: &Arc<Context>,
    requests: &mut [TestRequest],
    writer: &mut ResultWriter,
    csv_healthy: &mut bool,
    cancellation: &CancellationToken,
) -> Result<(), ControllerError> {
    let settings = &ctx.settings;
    // 어느 Worker에도 제출하기 전에 이름, 가용 상태, 사용자 설정을 모두 확인한다.
    let checks = settings
        .drivers
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, d)| {
            let ctx = ctx.clone();
            let request = requests[index].clone();
            let token = cancellation.clone();
            async move {
                let status: Option<WorkerStatus> = ctx
                    .http
                    .get_json(&format!("{}/status", d.url), &token)
                    .await?;
                let status = match status {
                    Some(s) if s.name.as_deref() == Some(d.name.as_str()) && s.available => s,
                    _ => {
                        return Err(ControllerError::Invalid(format!(
                            "{}: 이름 불일치 또는 실행 중",
                            d.name
                        )));
                    }
                };
                if status.lease_timeout_seconds < ctx.settings.lease_timeout_seconds {
                    return Err(ControllerError::Invalid(format!(
                        "{}: Worker의 lease 상한을 확인하세요.",
                        d.name
                    )));
                }
                if !status.uses_local_user {
                    request.validate(true).map_err(|e| {
                        ControllerError::Invalid(format!(
                            "{}: Controller의 [Main User] 설정이 필요합니다. {}",
                            d.name, e.message
                        ))
                    })?;
                }
                println!(
                    "{}: S3 사용자 설정 = {}",
                    d.name,
                    if status.uses_local_user {
                        "Worker config.ini"
                    } else {
                        "Controller"
                    }
                );
                Ok::<bool, ControllerError>(status.uses_local_user)
            }
        });
    for (index, local) in join_all(checks).await.into_iter().enumerate() {
        if local? {
            requests[index].user = None;
        }
    }
    let total_threads: i64 = requests
        .iter()
        .map(|r| i64::from(r.workload.as_ref().map_or(0, |w| w.thread_count)))
        .sum();
    println!(
        "RunId={}, Workers={}, TotalThreads={total_threads}",
        ctx.run_id,
        requests.len()
    );
    let submits = settings
        .drivers
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, d)| {
            let ctx = ctx.clone();
            let request = requests[index].clone();
            let token = cancellation.clone();
            async move {
                // 응답이 유실되어도 중단 대상에 포함한다.
                ctx.submitted.lock().unwrap().push(d.name.clone());
                ctx.touch(&d.name);
                ctx.http
                    .post_ensure(&format!("{}/runs", d.url), &request, &token)
                    .await?;
                Ok::<(), ControllerError>(())
            }
        });
    for result in join_all(submits).await {
        result?;
    }
    let preparing = Instant::now();
    let mut scheduled = false;
    loop {
        check_cancelled(cancellation)?;
        let samples = ctx.poll(cancellation).await;
        sample(writer, csv_healthy, &samples)?;
        check_cancelled(cancellation)?;
        for s in &samples {
            if let Some(snapshot) = &s.snapshot {
                if matches!(snapshot.state.as_deref(), Some("Failed" | "Cancelled")) {
                    return Err(ControllerError::Invalid(format!(
                        "{}: {}",
                        s.worker_id,
                        snapshot
                            .error
                            .as_deref()
                            .or(snapshot.state.as_deref())
                            .unwrap_or_default()
                    )));
                }
            } else if ctx.silent_for(&s.worker_id).as_secs_f64()
                >= f64::from(settings.lease_timeout_seconds)
            {
                return Err(ControllerError::Invalid(format!(
                    "{}: 연결 제한 시간 초과",
                    s.worker_id
                )));
            }
        }
        let all_in = |state: &str| {
            samples
                .iter()
                .all(|s| s.snapshot.as_ref().and_then(|x| x.state.as_deref()) == Some(state))
        };
        if !scheduled {
            if preparing.elapsed().as_secs_f64() > f64::from(settings.prepare_timeout_seconds) {
                return Err(ControllerError::Timeout("Worker 준비 제한 시간 초과"));
            }
            // 전체 준비 완료 후 동일 UTC 시각을 예약하고, 그 시각 전에 모든 수락 응답을 확인한다.
            if all_in("Ready") {
                let delay = i64::from(settings.start_delay_seconds);
                let at = DotnetDateTimeOffset::new(
                    DotnetDateTimeOffset::now().value() + chrono::Duration::seconds(delay),
                );
                let ack_deadline = cancellation.child_token();
                let timer = {
                    let token = ack_deadline.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(delay as u64)).await;
                        token.cancel();
                    })
                };
                let starts = settings.drivers.iter().cloned().map(|d| {
                    let ctx = ctx.clone();
                    let token = ack_deadline.clone();
                    async move {
                        ctx.http
                            .post_ensure(
                                &format!("{}/runs/{}/start", d.url, ctx.run_id),
                                &StartRequest { start_at_utc: at },
                                &token,
                            )
                            .await
                    }
                });
                let results = join_all(starts).await;
                timer.abort();
                for result in results {
                    result?;
                }
                if DotnetDateTimeOffset::now() >= at {
                    return Err(ControllerError::Timeout(
                        "시작 시각까지 예약 수락을 확인하지 못했습니다.",
                    ));
                }
                scheduled = true;
                println!("예약 시작: {}", at.to_o_text());
            }
        }
        if scheduled && all_in("Completed") {
            return Ok(());
        }
        let delay = Duration::from_secs(settings.poll_interval_seconds as u64);
        tokio::select! {
            () = cancellation.cancelled() => return Err(ControllerError::Canceled),
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// `Guid.NewGuid().ToString("N")`: 버전 4 UUID의 소문자 16진수 32자.
fn uuid_n() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_guid_n() {
        let id = uuid_n();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_ne!(id, uuid_n());
    }
}
