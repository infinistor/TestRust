//! 원본 `Distributed/RunControl.cs`: 분산 실행 작업의 준비 barrier, 예약 시작, 취소, 단조 시계 기반 실행 시간.
//!
//! 원본은 `CountdownEvent`(스레드 준비)·`ManualResetEventSlim`(예약·게이트)·`CancellationTokenSource`·`Stopwatch`를
//! 쓴다. 여기서는 tokio `watch` 채널과 [`CancellationToken`], [`Instant`]로 같은 상태 전이를 만든다.
//!
//! 상태: `Preparing` → (모든 스레드 준비) `Ready` → (`schedule`) `Scheduled` → (예약 시각) `Running` →
//! (`issuing_stopped`·`stop`) `Stopping` → (`complete`) `Completed`·`Cancelled`·`Failed`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use testrust_common::DotnetDateTimeOffset;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::ScenarioError;

/// 대기 중 취소(`OperationCanceledException`).
pub fn operation_canceled() -> ScenarioError {
    ScenarioError::new(
        "System.OperationCanceledException",
        "The operation was canceled.",
    )
}

/// [`RunControl::apply`]가 채우는 상태 값(원본 `Apply(RunSnapshot)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ControlSnapshot {
    pub state: String,
    pub error: Option<String>,
    pub sample_at_utc: DotnetDateTimeOffset,
    pub scheduled_at_utc: Option<DotnetDateTimeOffset>,
    pub started_at_utc: Option<DotnetDateTimeOffset>,
    pub issuing_stopped_at_utc: Option<DotnetDateTimeOffset>,
    pub completed_at_utc: Option<DotnetDateTimeOffset>,
    pub elapsed_seconds: f64,
}

#[derive(Debug)]
struct Inner {
    state: &'static str,
    error: Option<String>,
    cancelled: bool,
    failed_state: bool,
    start_at: Option<DotnetDateTimeOffset>,
    started: Option<DotnetDateTimeOffset>,
    stopped: Option<DotnetDateTimeOffset>,
    completed: Option<DotnetDateTimeOffset>,
    /// `Stopwatch`: 시작 시각과 멈췄을 때의 누적 시간.
    elapsed_since: Option<Instant>,
    elapsed_frozen: Option<Duration>,
    ready_remaining: usize,
}

impl Inner {
    fn elapsed(&self) -> Duration {
        match (self.elapsed_frozen, self.elapsed_since) {
            (Some(frozen), _) => frozen,
            (None, Some(since)) => since.elapsed(),
            (None, None) => Duration::ZERO,
        }
    }
}

/// 원본 `RunControl`.
#[derive(Debug)]
pub struct RunControl {
    inner: Mutex<Inner>,
    cancellation: CancellationToken,
    ready: watch::Sender<bool>,
    scheduled: watch::Sender<bool>,
    gate: watch::Sender<bool>,
}

fn now() -> DotnetDateTimeOffset {
    DotnetDateTimeOffset::new(Utc::now())
}

/// `signal`이 켜지거나 취소될 때까지 기다린다. 취소되면 `OperationCanceledException`.
async fn wait(
    signal: &watch::Sender<bool>,
    token: &CancellationToken,
) -> Result<(), ScenarioError> {
    let mut receiver = signal.subscribe();
    tokio::select! {
        result = receiver.wait_for(|set| *set) => result.map(|_| ()).map_err(|_| operation_canceled()),
        _ = token.cancelled() => Err(operation_canceled()),
    }
}

impl RunControl {
    /// 원본 `RunControl(threads)`.
    pub fn new(threads: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: "Preparing",
                error: None,
                cancelled: false,
                failed_state: false,
                start_at: None,
                started: None,
                stopped: None,
                completed: None,
                elapsed_since: None,
                elapsed_frozen: None,
                ready_remaining: threads,
            }),
            cancellation: CancellationToken::new(),
            ready: watch::Sender::new(threads == 0),
            scheduled: watch::Sender::new(false),
            gate: watch::Sender::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 원본 `Token`.
    pub fn token(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// 원본 `Token.ThrowIfCancellationRequested()`.
    pub fn throw_if_cancelled(&self) -> Result<(), ScenarioError> {
        if self.cancellation.is_cancelled() {
            Err(operation_canceled())
        } else {
            Ok(())
        }
    }

    /// 원본 `IsStopped`.
    pub fn is_stopped(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// 원본 `ElapsedSeconds`.
    pub fn elapsed_seconds(&self) -> f64 {
        self.lock().elapsed().as_secs_f64()
    }

    /// 원본 `IsFinal`.
    pub fn is_final(&self) -> bool {
        self.lock().completed.is_some()
    }

    /// 원본 `ThreadReadyAndWait()`: 부하 스레드가 준비 완료를 알리고 공통 시작 게이트가 열릴 때까지 기다린다.
    pub async fn thread_ready_and_wait(&self) -> Result<(), ScenarioError> {
        {
            let mut inner = self.lock();
            inner.ready_remaining = inner.ready_remaining.saturating_sub(1);
            if inner.ready_remaining == 0 {
                self.ready.send_replace(true);
            }
        }
        wait(&self.gate, &self.cancellation).await?;
        self.throw_if_cancelled()
    }

    /// 원본 `ReadyAndWait(beforeRelease)`: 모든 스레드 준비와 시작 예약을 기다린 뒤 통계 초기화(`before_release`)를 마치고
    /// 함께 실행시킨다.
    pub async fn ready_and_wait(&self, before_release: impl FnOnce()) -> Result<(), ScenarioError> {
        wait(&self.ready, &self.cancellation).await?;
        {
            let mut inner = self.lock();
            self.throw_if_cancelled()?;
            inner.state = "Ready";
        }
        wait(&self.scheduled, &self.cancellation).await?;
        let target = self.lock().start_at.expect("예약 뒤에만 깨어난다");
        // 예약을 로컬 단조 시계의 대기 시간으로 바꾼다.
        let delay = (target.value() - Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        let waiting_at = Instant::now();
        while delay > waiting_at.elapsed() {
            let remaining = (delay - waiting_at.elapsed()).max(Duration::from_millis(1));
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {}
                _ = self.cancellation.cancelled() => return Err(operation_canceled()),
            }
        }
        let mut inner = self.lock();
        self.throw_if_cancelled()?;
        inner.started = Some(now());
        inner.elapsed_since = Some(Instant::now());
        before_release();
        inner.state = "Running";
        self.gate.send_replace(true);
        Ok(())
    }

    /// 원본 `Schedule(target)`: 같은 예약의 재전송은 허용하되, 실행 도중 다른 시각으로 바꿀 수는 없다.
    pub fn schedule(&self, target: DotnetDateTimeOffset) -> Result<(), ScenarioError> {
        let mut inner = self.lock();
        if inner.start_at == Some(target) {
            return Ok(());
        }
        if inner.state != "Ready" || target <= now() {
            return Err(ScenarioError::new(
                "System.InvalidOperationException",
                "Ready 상태와 미래 시작 시각이 필요합니다.",
            ));
        }
        inner.start_at = Some(target);
        inner.state = "Scheduled";
        self.scheduled.send_replace(true);
        Ok(())
    }

    /// 원본 `DurationReached(seconds)`: 실제 시작 지연을 빼서 예약 시각 기준으로 부하 발행 시간을 제한한다.
    pub fn duration_reached(&self, seconds: i32) -> bool {
        let inner = self.lock();
        let (Some(start_at), Some(started)) = (inner.start_at, inner.started) else {
            return false;
        };
        let late = (started.value() - start_at.value())
            .num_nanoseconds()
            .unwrap_or(0) as f64
            / 1e9;
        inner.elapsed().as_secs_f64() >= f64::from(seconds) - late
    }

    /// 원본 `Stop(reason, failed)`.
    pub fn stop(&self, reason: Option<&str>, failed: bool) {
        let mut inner = self.lock();
        if inner.completed.is_some() {
            return;
        }
        inner.cancelled = true;
        if inner.error.is_none() {
            inner.error = if failed {
                Some(reason.unwrap_or("Worker 실행 오류").to_string())
            } else {
                reason.map(str::to_string)
            };
        }
        inner.failed_state |= failed;
        if inner.stopped.is_none() && inner.started.is_some() {
            inner.stopped = Some(now());
        }
        inner.state = "Stopping";
        self.cancellation.cancel();
    }

    /// 원본 `IssuingStopped()`: 신규 요청 발행만 끝났음을 표시한다(진행 중 요청 정리가 끝나야 `complete`).
    pub fn issuing_stopped(&self) {
        let mut inner = self.lock();
        if inner.completed.is_some() {
            return;
        }
        if inner.stopped.is_none() && inner.started.is_some() {
            inner.stopped = Some(now());
        }
        inner.state = "Stopping";
    }

    /// 원본 `Complete()`.
    pub fn complete(&self) {
        let mut inner = self.lock();
        if inner.completed.is_none() {
            inner.completed = Some(now());
        }
        let elapsed = inner.elapsed();
        inner.elapsed_frozen = Some(elapsed);
        inner.state = if inner.failed_state {
            "Failed"
        } else if inner.cancelled {
            "Cancelled"
        } else {
            "Completed"
        };
    }

    /// 원본 `Apply(snapshot)`.
    pub fn apply(&self) -> ControlSnapshot {
        let inner = self.lock();
        ControlSnapshot {
            state: inner.state.to_string(),
            error: inner.error.clone(),
            sample_at_utc: now(),
            scheduled_at_utc: inner.start_at,
            started_at_utc: inner.started,
            issuing_stopped_at_utc: inner.stopped,
            completed_at_utc: inner.completed,
            elapsed_seconds: inner.elapsed().as_secs_f64(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// 예약 전에는 부하가 시작되지 않고, 같은 예약은 다시 받아도 된다.
    #[tokio::test]
    async fn no_load_before_schedule_and_idempotent_schedule() {
        let control = Arc::new(RunControl::new(1));
        let started = Arc::new(AtomicBool::new(false));
        let worker = {
            let (control, started) = (control.clone(), started.clone());
            tokio::spawn(async move {
                control.thread_ready_and_wait().await.unwrap();
                started.store(true, Ordering::SeqCst);
            })
        };
        let main = {
            let control = control.clone();
            tokio::spawn(async move { control.ready_and_wait(|| {}).await })
        };
        while control.apply().state != "Ready" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!started.load(Ordering::SeqCst), "예약 전 부하 없음");
        let target = DotnetDateTimeOffset::new(Utc::now() + chrono::Duration::milliseconds(100));
        control.schedule(target).unwrap();
        control.schedule(target).unwrap();
        assert!(
            control
                .schedule(DotnetDateTimeOffset::new(
                    Utc::now() + chrono::Duration::seconds(5)
                ))
                .is_err()
        );
        main.await.unwrap().unwrap();
        worker.await.unwrap();
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(control.apply().state, "Running");
        control.issuing_stopped();
        control.complete();
        assert_eq!(control.apply().state, "Completed");
    }

    /// 게이트 전에 취소하면 시작하지 않고 Cancelled로 끝난다.
    #[tokio::test]
    async fn cancel_before_gate() {
        let control = Arc::new(RunControl::new(1));
        let worker = {
            let control = control.clone();
            tokio::spawn(async move { control.thread_ready_and_wait().await })
        };
        control.stop(Some("Controller 중단 요청"), false);
        assert!(worker.await.unwrap().is_err());
        assert!(control.ready_and_wait(|| {}).await.is_err());
        control.complete();
        let snapshot = control.apply();
        assert_eq!(snapshot.state, "Cancelled");
        assert_eq!(snapshot.error.as_deref(), Some("Controller 중단 요청"));
        assert!(snapshot.started_at_utc.is_none());
    }
}
