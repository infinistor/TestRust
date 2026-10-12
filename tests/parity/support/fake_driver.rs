//! Controller 프로토콜 시험용 가짜 Worker. `/driver` HTTP API(상태 조회, 제출, 폴링, 예약 시작, 하트비트, 중단)를 흉내 내고
//! 받은 요청을 기록한다. 동작은 [`Behavior`]로 정한다.

use std::sync::{Arc, Mutex};

use testrust_common::{DotnetDateTimeOffset, to_web_json};
use testrust_distributed::contracts::{RunResult, RunSnapshot, WorkerStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub body: String,
}

/// Worker 한 대의 동작.
#[derive(Clone, Debug)]
pub struct Behavior {
    pub name: String,
    pub available: bool,
    pub lease_timeout_seconds: i32,
    pub uses_local_user: bool,
    /// `GET /status`가 이 상태 코드로 답한다(성공이 아니면 본문 없음).
    pub status_code: u16,
    /// 제출 뒤 `Ready`가 되기까지 폴링 횟수(`None`이면 계속 `Preparing`).
    pub ready_after_polls: Option<u32>,
    /// 예약 시작을 받은 뒤 `Running`으로 답하는 폴링 횟수(그 다음 `Completed`).
    pub running_polls: u32,
    /// `POST /runs/{id}/start`의 응답 상태 코드.
    pub start_status: u16,
    /// 폴링이 이 횟수를 넘으면 `Failed`와 이 오류를 돌려준다.
    pub fail_after_polls: Option<(u32, String)>,
    /// 폴링 응답의 RunId/WorkerId를 바꿔 식별 오류를 만든다.
    pub wrong_identity: bool,
}

impl Behavior {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            available: true,
            lease_timeout_seconds: 15,
            uses_local_user: false,
            status_code: 200,
            ready_after_polls: Some(1),
            running_polls: 1,
            start_status: 200,
            fail_after_polls: None,
            wrong_identity: false,
        }
    }
}

#[derive(Default)]
struct State {
    run_id: Option<String>,
    polls: u32,
    started: bool,
    start_at: Option<DotnetDateTimeOffset>,
    running_seen: u32,
    stopped: bool,
    requests: Vec<Recorded>,
}

pub struct FakeDriver {
    pub port: u16,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeDriver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeDriver {
    pub async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let behavior = Arc::new(behavior);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (state, behavior) = (shared.clone(), behavior.clone());
                tokio::spawn(async move {
                    let _ = serve(socket, state, behavior).await;
                });
            }
        });
        Self { port, state, task }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/driver", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    /// `method`와 경로 끝(`/stop` 등)이 맞는 요청.
    pub fn find(&self, method: &str, suffix: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && r.path.ends_with(suffix))
            .collect()
    }
}

async fn read_line(socket: &mut TcpStream) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while !line.ends_with(b"\r\n") {
        if socket.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        line.push(byte[0]);
    }
    String::from_utf8(line).ok()
}

async fn serve(
    mut socket: TcpStream,
    state: Arc<Mutex<State>>,
    behavior: Arc<Behavior>,
) -> Option<()> {
    loop {
        let mut head = Vec::new();
        loop {
            let line = read_line(&mut socket).await?;
            if line == "\r\n" {
                break;
            }
            head.push(line.trim_end().to_string());
        }
        let header = |name: &str| {
            head.iter().skip(1).find_map(|h| {
                let (n, v) = h.split_once(':')?;
                n.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        };
        let length: usize = header("Content-Length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        socket.read_exact(&mut body).await.ok()?;
        let mut parts = head[0].split(' ');
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let recorded = Recorded {
            method: method.clone(),
            path: path.clone(),
            content_type: header("Content-Type"),
            body: String::from_utf8_lossy(&body).into_owned(),
        };
        let (status, reply) = respond(&state, &behavior, recorded);
        let text = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(text.as_bytes()).await.ok()?;
    }
}

fn snapshot(run_id: &str, behavior: &Behavior, state: &str, error: Option<&str>) -> String {
    let now = DotnetDateTimeOffset::now();
    let done = matches!(state, "Completed" | "Cancelled" | "Failed");
    let mut result = RunResult::default();
    if state == "Running" || state == "Completed" {
        result.write = 10;
        result.total = 10;
        result.thread_count = 2;
        result.file_size = 1024;
    }
    let (run_id, worker_id) = if behavior.wrong_identity {
        (
            "ffffffffffffffffffffffffffffffff".to_string(),
            "other".to_string(),
        )
    } else {
        (run_id.to_string(), behavior.name.clone())
    };
    to_web_json(&RunSnapshot {
        run_id: Some(run_id.clone()),
        worker_id: Some(worker_id),
        test_type: Some("Put".to_string()),
        state: Some(state.to_string()),
        error: error.map(str::to_string),
        sample_at_utc: now,
        scheduled_at_utc: None,
        started_at_utc: matches!(state, "Running" | "Completed").then_some(now),
        issuing_stopped_at_utc: None,
        completed_at_utc: done.then_some(now),
        elapsed_seconds: 1.0,
        result: Some(result),
    })
}

fn respond(state: &Mutex<State>, b: &Behavior, request: Recorded) -> (u16, String) {
    let mut s = state.lock().unwrap();
    s.requests.push(request.clone());
    let path = request.path.as_str();
    if request.method == "GET" && path.ends_with("/status") {
        if !(200..300).contains(&b.status_code) {
            return (b.status_code, String::new());
        }
        let status = WorkerStatus {
            name: Some(b.name.clone()),
            available: b.available,
            run_id: None,
            lease_timeout_seconds: b.lease_timeout_seconds,
            uses_local_user: b.uses_local_user,
        };
        return (200, to_web_json(&status));
    }
    if request.method == "POST" && path.ends_with("/runs") {
        let value: serde_json::Value = serde_json::from_str(&request.body).unwrap_or_default();
        s.run_id = value["runId"].as_str().map(str::to_string);
        return (202, "{}".to_string());
    }
    let run_id = s.run_id.clone().unwrap_or_default();
    if request.method == "POST" && path.ends_with("/start") {
        let value: serde_json::Value = serde_json::from_str(&request.body).unwrap_or_default();
        s.start_at = value["startAtUtc"]
            .as_str()
            .and_then(|t| DotnetDateTimeOffset::parse(t).ok());
        if (200..300).contains(&b.start_status) {
            s.started = true;
        }
        return (b.start_status, "{}".to_string());
    }
    if request.method == "POST" && path.ends_with("/stop") {
        s.stopped = true;
        return (200, "{}".to_string());
    }
    if request.method == "POST" && path.ends_with("/heartbeat") {
        return (200, "{}".to_string());
    }
    if request.method == "GET" && path.contains("/runs/") {
        s.polls += 1;
        let polls = s.polls;
        if s.stopped {
            return (
                200,
                snapshot(&run_id, b, "Cancelled", Some("Controller 중단 요청")),
            );
        }
        if let Some((after, error)) = &b.fail_after_polls
            && polls > *after
        {
            return (200, snapshot(&run_id, b, "Failed", Some(error)));
        }
        if s.started {
            s.running_seen += 1;
            let state = if s.running_seen <= b.running_polls {
                "Running"
            } else {
                "Completed"
            };
            return (200, snapshot(&run_id, b, state, None));
        }
        let state = match b.ready_after_polls {
            Some(after) if polls >= after => "Ready",
            _ => "Preparing",
        };
        return (200, snapshot(&run_id, b, state, None));
    }
    (404, String::new())
}
