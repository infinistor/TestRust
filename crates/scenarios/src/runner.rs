//! 원본 시나리오의 스레드 조립 패턴(UpDownTest·LocalTest·MultiSystemTest 공용).
//!
//! 원본은 클라이언트마다 `Thread`를 만들어(`_taskList`) 1ms 간격으로 시작하고, 주 스레드가 `TaskCheck()`
//! (살아 있는 스레드가 있고 모든 클라이언트가 `Quit`은 아님)로 감시하며 2초마다 진행 상황을 출력한다.
//! 여기서는 같은 일을 tokio 작업으로 한다.
//!
//! - 시작 전 작업은 만들어만 두고([`TestTasks::add`]) [`TestTasks::start`]에서 띄운다(원본 `new Thread` → `Start`).
//! - 스레드 안에서 처리하지 않은 예외(`Err`)는 원본처럼 프로세스를 끝낸다([`testrust_common::dotnet_exit::crash`]).
//!   최종 출력과 JSON 저장은 하지 않는다.
//! - 동기 클라이언트(LocalClient)는 [`TestTasks::add_blocking`]으로 블로킹 스레드에서 돌린다.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use testrust_clients::local::LocalClient;
use testrust_clients::multi_system::MultiSystemClient;
use testrust_clients::up_down::UpDownClient;
use testrust_common::dotnet_exit::crash;
use testrust_model::TestClient;
use tokio::task::JoinHandle;

use crate::ScenarioError;

/// 원본 `Quit` 속성을 가진 클라이언트(`ITestClient`, `MultiSystemClient`).
pub trait Quit: Send + Sync {
    fn is_quit(&self) -> bool;
    fn set_quit_true(&self);
}

macro_rules! impl_quit {
    ($($client:ty),*) => {$(
        impl Quit for $client {
            fn is_quit(&self) -> bool {
                self.quit()
            }

            fn set_quit_true(&self) {
                self.set_quit(true);
            }
        }
    )*};
}

impl_quit!(UpDownClient, LocalClient, MultiSystemClient);

type Work = Pin<Box<dyn Future<Output = ()> + Send>>;

enum Pending {
    Async(Work),
    Blocking(Box<dyn FnOnce() + Send>),
}

/// 원본 `_testList`(클라이언트)와 `_taskList`(스레드).
pub struct TestTasks<C: ?Sized> {
    clients: Vec<Arc<C>>,
    pending: Vec<Pending>,
    handles: Vec<JoinHandle<()>>,
}

impl<C: Quit + ?Sized + 'static> Default for TestTasks<C> {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
            pending: Vec::new(),
            handles: Vec::new(),
        }
    }
}

/// 스레드 안 예외: 원본처럼 프로세스를 끝낸다.
fn unhandled<E: Into<ScenarioError>>(error: E) -> ! {
    let error = error.into();
    crash(&error.dotnet_type, &error.message)
}

impl<C: Quit + ?Sized + 'static> TestTasks<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// 원본 `_testList.Add(client)`와 `_taskList.Add(new Thread(work))`. 작업은 [`Self::start`]에서 시작한다.
    pub fn add<F, E>(&mut self, client: Arc<C>, work: F)
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Into<ScenarioError>,
    {
        self.clients.push(client);
        self.pending.push(Pending::Async(Box::pin(async move {
            if let Err(e) = work.await {
                unhandled(e);
            }
        })));
    }

    /// [`Self::add`]와 같으나 작업의 `Err`를 `on_error`로 넘긴다(분산 실행: 프로세스를 끝내지 않고 `RunControl.Stop`).
    pub fn add_with<F, H>(&mut self, client: Arc<C>, work: F, on_error: H)
    where
        F: Future<Output = Result<(), ScenarioError>> + Send + 'static,
        H: FnOnce(ScenarioError) + Send + 'static,
    {
        self.clients.push(client);
        self.pending.push(Pending::Async(Box::pin(async move {
            if let Err(e) = work.await {
                on_error(e);
            }
        })));
    }

    /// [`Self::add`]의 동기 작업판(블로킹 스레드에서 실행).
    pub fn add_blocking<F, E>(&mut self, client: Arc<C>, work: F)
    where
        F: FnOnce() -> Result<(), E> + Send + 'static,
        E: Into<ScenarioError>,
    {
        self.clients.push(client);
        self.pending.push(Pending::Blocking(Box::new(move || {
            if let Err(e) = work() {
                unhandled(e);
            }
        })));
    }

    /// 원본 `_testList`.
    pub fn clients(&self) -> &[Arc<C>] {
        &self.clients
    }

    /// 만든 작업이 없는지(원본 `_taskList.Count == 0`).
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.handles.is_empty()
    }

    /// 원본 `foreach (var item in _taskList) { item.Start(); Thread.Sleep(1); }`. 작업이 없으면 `false`
    /// (원본은 여기서 `Task Start Failed` 등을 로그로 남긴다. 문구가 시나리오마다 달라 호출한 쪽이 남긴다).
    pub async fn start(&mut self) -> bool {
        self.start_with(true).await
    }

    /// [`Self::start`]. `gap`이 아니면 1ms 간격 없이 띄운다(원본 분산 실행 `TaskStart`).
    pub async fn start_with(&mut self, gap: bool) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        for pending in std::mem::take(&mut self.pending) {
            let handle = match pending {
                Pending::Async(work) => tokio::spawn(work),
                Pending::Blocking(work) => tokio::task::spawn_blocking(work),
            };
            self.handles.push(handle);
            if gap {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        true
    }

    /// 원본 `TaskCheck()`: 살아 있는 작업이 있고 모든 클라이언트가 `Quit`은 아니면 `true`.
    pub fn check(&self) -> bool {
        if self.handles.is_empty() || self.handles.iter().all(JoinHandle::is_finished) {
            return false;
        }
        !self.clients.iter().all(|c| c.is_quit())
    }

    /// 원본 `TestStop()`: 모든 클라이언트를 `Quit`으로.
    pub fn stop(&self) {
        for client in &self.clients {
            client.set_quit_true();
        }
    }

    /// 원본 `JoinTasks()`: 모든 작업이 끝날 때까지 기다린다.
    pub async fn join(&mut self) {
        for handle in std::mem::take(&mut self.handles) {
            let _ = handle.await;
        }
    }
}

/// 감시 루프의 `Thread.Sleep(1)`.
pub async fn idle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// 원본 `FinalResult`: 마무리(정지·대기)와 최종 출력·저장을 한 번만 한다.
#[derive(Debug, Default)]
pub struct FinalResult {
    completed: bool,
}

impl FinalResult {
    /// 처음 부를 때만 `true`(원본 `Interlocked.CompareExchange`).
    pub fn begin(&mut self) -> bool {
        !std::mem::replace(&mut self.completed, true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Default)]
    struct Fake(AtomicBool);

    impl Quit for Fake {
        fn is_quit(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }

        fn set_quit_true(&self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn check_stop_and_join() {
        let mut tasks = TestTasks::<Fake>::new();
        assert!(!tasks.start().await);
        let client = Arc::new(Fake::default());
        let worker = client.clone();
        tasks.add(client, async move {
            while !worker.is_quit() {
                idle().await;
            }
            Ok::<(), ScenarioError>(())
        });
        assert!(!tasks.check(), "시작 전에는 감시할 작업이 없다");
        assert!(tasks.start().await);
        assert!(tasks.check());
        tasks.stop();
        assert!(!tasks.check());
        tasks.join().await;
        assert!(tasks.is_empty());
    }

    #[test]
    fn final_result_once() {
        let mut result = FinalResult::default();
        assert!(result.begin());
        assert!(!result.begin());
    }
}
