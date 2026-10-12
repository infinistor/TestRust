//! `Test/LocalTest.cs`: 로컬 파일시스템 대상 Prepare·Put·Get·GetV2·PutGet·Delete 부하 테스트.
//!
//! 원본과 같게 맞춘 동작
//!
//! - 스레드마다 더미 파일 하나와 `LocalClient` 하나를 만들고(`GetDummyFileName(index, FilePath)`), 클라이언트는
//!   동기 API라 블로킹 스레드에서 돌린다([`TestTasks::add_blocking`]).
//! - 진행·최종 출력의 시간은 `TimeWatcher`가 아니라 `DateTime.Now - _testStartTime`이다.
//! - 결과 JSON 이름은 `Local_{SanitizeFileName(이름).Replace(" ", "_")}_{yyyyMMdd_HHmmss}.json`.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - Ctrl+C 처리가 없다. 클라이언트의 종료 신호는 테스트가 갖는 토큰(취소되지 않는다)에만 묶는다.
//! - `Read`·`ReadV2`·`Delete`는 대상 디렉터리가 없으면 `Target directory not found`만 남기고 끝난다(결과 출력·저장 없음).
//! - `Delete`·`ReadV2`는 시간 제한 없이 스레드가 끝날 때까지 기다린다.
//! - 스레드 작업 시작은 항상 성공한다(스레드가 0개여도 최종 결과를 출력한다).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rust_decimal::Decimal;
use testrust_clients::file_util::create_random_file;
use testrust_clients::local::LocalClient;
use testrust_common::dotnet_datetime::DotnetDateTime;
use testrust_config::{MainConfig, UpDownClientConfig, UpDownConfig};
use testrust_model::time_watcher::seconds;
use testrust_model::up_down_stats::log_info;
use testrust_model::{TimeWatcher, UpDownStats};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::ScenarioError;
use crate::input::io_error;
use crate::runner::{TestTasks, idle};
use crate::util::{dummy_file_name, has_extension, path_combine, sanitize_file_name};

/// 진행 상황·최종 결과 출력 종류(원본 `PrintX`/`PrintXFinal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    Prepare,
    Write,
    Read,
    ReadV2,
    Mix,
    Delete,
}

/// 원본 `LocalTest`.
pub struct LocalTest {
    main_config: MainConfig,
    config: UpDownConfig,
    target_path: String,
    watcher: TimeWatcher,
    use_multipart: bool,
    tasks: TestTasks<LocalClient>,
    client_config: UpDownClientConfig,
    stats: UpDownStats,
    test_start_time: DotnetDateTime,
    test_start: Instant,
    /// 클라이언트 종료 신호의 상위 토큰(원본에는 Ctrl+C 처리가 없어 취소되지 않는다).
    token: CancellationToken,
}

impl LocalTest {
    pub fn new(
        main_config: &MainConfig,
        config: &UpDownConfig,
        target_path: &str,
        use_multipart: bool,
    ) -> Self {
        let client_config = UpDownClientConfig::new(
            &main_config.thread_prefix,
            &main_config.object_prefix,
            config.read_ratio,
            config.write_ratio,
            config.delete_ratio,
            main_config.file_size,
            config.bucket_type,
            config.etag_check,
            config.division_count,
            main_config.retry_count,
            main_config.is_admin,
            config.use_chunk_encoding,
        );
        Self {
            main_config: main_config.clone(),
            config: config.clone(),
            target_path: target_path.to_string(),
            watcher: TimeWatcher::new(config.times),
            use_multipart,
            tasks: TestTasks::new(),
            client_config,
            stats: UpDownStats::new(main_config.file_size),
            test_start_time: DotnetDateTime::now(),
            test_start: Instant::now(),
            token: CancellationToken::new(),
        }
    }

    /// 원본 `_testStartTime = DateTime.Now`.
    fn mark_start(&mut self) {
        self.test_start_time = DotnetDateTime::now();
        self.test_start = Instant::now();
    }

    /// `Directory.CreateDirectory(_targetPath)`.
    fn create_target_directory(&self) -> Result<(), ScenarioError> {
        let path = Path::new(&self.target_path);
        std::fs::create_dir_all(path).map_err(|e| io_error(path, &e))
    }

    /// 대상 디렉터리 확인. 없으면 로그만 남기고 `false`.
    fn target_exists(&self) -> bool {
        if Path::new(&self.target_path).is_dir() {
            return true;
        }
        error!("Target directory not found: {}", self.target_path);
        false
    }

    /// 스레드마다 더미 파일과 클라이언트를 만든다. `always_create`가 아니면 파일이 이미 있을 때 만들지 않는다.
    fn add_clients<F>(&mut self, always_create: bool, work: F)
    where
        F: Fn(&LocalClient) -> Result<(), testrust_clients::LocalError> + Clone + Send + 'static,
    {
        for index in 0..self.config.thread_count {
            let temp_file = dummy_file_name(index, Some(&self.main_config.file_path));
            if always_create || !Path::new(&temp_file).exists() {
                create_random_file(Path::new(&temp_file), self.main_config.file_size, false);
            }
            let client = Arc::new(
                LocalClient::new(
                    &self.target_path,
                    index,
                    temp_file,
                    self.client_config.clone(),
                    self.use_multipart,
                )
                .with_quit(&self.token),
            );
            let worker = client.clone();
            let work = work.clone();
            self.tasks.add_blocking(client, move || work(&worker));
        }
    }

    /// `TaskStart()`(항상 성공)과 `_watcher.Start()`.
    async fn start(&mut self) {
        if !self.tasks.is_empty() {
            self.tasks.start().await;
        }
        self.watcher.start();
    }

    /// 감시 루프: 2초마다 진행 상황을 출력한다. `timed`이면 `Times`가 지나면 끝낸다.
    async fn watch(&mut self, timed: bool, report: Report) {
        while (!timed || !self.watcher.is_end()) && self.tasks.check() {
            if self.watcher.is_next() {
                self.print(report, false);
            } else {
                idle().await;
            }
        }
    }

    /// 원본 `TestStop()`.
    async fn test_stop(&mut self) {
        self.tasks.stop();
        self.tasks.join().await;
    }

    /// 원본 `PrintX()`/`PrintXFinal()`.
    fn print(&mut self, report: Report, is_final: bool) {
        let clients: Vec<&LocalClient> = self.tasks.clients().iter().map(Arc::as_ref).collect();
        self.stats.update(&clients);
        let times = seconds(self.test_start.elapsed());
        let total =
            i64::from(self.config.thread_count).wrapping_mul(i64::from(self.config.file_count));
        let stats = &self.stats;
        let message = match (report, is_final) {
            (Report::Prepare, false) => stats.prepare_message(total, times),
            (Report::Prepare, true) => stats.prepare_final_message(total, times),
            (Report::Write, false) => stats.write_message(times),
            (Report::Write, true) => stats.write_final_message(times),
            (Report::Read, false) => stats.read_message(times),
            (Report::Read, true) => stats.read_final_message(times),
            (Report::ReadV2, false) => stats.read_total_message(total, times),
            (Report::ReadV2, true) => stats.read_v2_final_message(total, times),
            (Report::Mix, false) => stats.mix_message(times),
            (Report::Mix, true) => stats.mix_final_message(times),
            (Report::Delete, false) => stats.delete_message(times),
            (Report::Delete, true) => stats.delete_final_message(times),
        };
        log_info(&message);
    }

    /// 순차적으로 파일을 생성하는 테스트(원본 `Prepare`).
    pub async fn prepare(&mut self, check: bool, start: i32) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local Prepare Test Initialize");
        self.create_target_directory()?;
        let file_count = self.config.file_count;
        self.add_clients(true, move |client| client.prepare(file_count, check, start));
        info!("Local Prepare Test Start");
        self.start().await;
        // 모든 스레드가 파일을 생성할 때까지 반복
        self.watch(false, Report::Prepare).await;
        self.print(Report::Prepare, true);
        self.save_result_to_json("Local Prepare");
        info!("Local Prepare Test End");
        Ok(())
    }

    /// 일정 시간동안 로컬에 파일을 쓰는 테스트(원본 `Write`).
    pub async fn write(&mut self) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local Put Test Initialize");
        self.create_target_directory()?;
        self.add_clients(true, |client| client.write());
        info!("Local Put Test Start");
        self.start().await;
        self.watch(true, Report::Write).await;
        self.test_stop().await;
        self.print(Report::Write, true);
        self.save_result_to_json("Local Write");
        info!("Local Put Test End");
        Ok(())
    }

    /// 일정 시간동안 로컬 파일을 랜덤하게 읽는 테스트(원본 `Read`).
    pub async fn read(&mut self) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local Random Read Test Initialize");
        if !self.target_exists() {
            return Ok(());
        }
        self.add_clients(false, |client| client.read());
        info!("Local Random Read Test Start");
        self.start().await;
        self.watch(true, Report::Read).await;
        self.test_stop().await;
        self.print(Report::Read, true);
        self.save_result_to_json("Local Read");
        info!("Local Random Read Test End");
        Ok(())
    }

    /// 순차적으로 파일을 가져오는 테스트(원본 `ReadV2`). 사전에 파일이 생성되어 있어야 한다.
    pub async fn read_v2(&mut self, start: i32) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local Sequential Read Test Initialize");
        if !self.target_exists() {
            return Ok(());
        }
        let file_count = self.config.file_count;
        self.add_clients(false, move |client| client.read_v2(file_count, start));
        info!("Local Sequential Read Test Start");
        self.start().await;
        self.watch(false, Report::ReadV2).await;
        self.test_stop().await;
        self.print(Report::ReadV2, true);
        self.save_result_to_json("Local ReadV2");
        info!("Local Sequential Read Test End");
        Ok(())
    }

    /// 주어진 시간동안 파일을 쓰고 읽는 테스트(원본 `PutGet`).
    pub async fn put_get(&mut self) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local PutGet Test Initialize");
        self.create_target_directory()?;
        self.add_clients(true, |client| client.put_get());
        info!("Local PutGet Test Start");
        self.start().await;
        self.watch(true, Report::Mix).await;
        self.test_stop().await;
        self.print(Report::Mix, true);
        self.save_result_to_json("Local Write/Read(1:1)");
        info!("Local PutGet Test End");
        Ok(())
    }

    /// 로컬 파일 삭제 테스트(원본 `Delete`).
    pub async fn delete(&mut self) -> Result<(), ScenarioError> {
        self.mark_start();
        info!("Local Delete Test Initialize");
        if !self.target_exists() {
            return Ok(());
        }
        self.add_clients(false, |client| client.delete());
        info!("Local Delete Test Start");
        self.start().await;
        // 삭제는 시간 제한 없이 파일이 없을 때까지
        self.watch(false, Report::Delete).await;
        self.test_stop().await;
        self.print(Report::Delete, true);
        self.save_result_to_json("Local Delete");
        info!("Local Delete Test End");
        Ok(())
    }

    /// 원본 `SaveResultToJson(testType)`.
    fn save_result_to_json(&self, test_type: &str) {
        let Some(save) = self.config.save.as_deref().filter(|s| !s.trim().is_empty()) else {
            return;
        };
        let mut result = self.stats.to_up_down_result(
            test_type,
            &self.config,
            &self.main_config,
            Decimal::from(self.watcher.end_time()),
        );
        result.start_time = self.test_start_time;
        result.end_time = DotnetDateTime::now();
        let file_path = if has_extension(save) {
            // 파일명이 포함된 경우 그대로 사용
            save.to_string()
        } else {
            // 디렉토리만 지정된 경우 파일명 생성
            let timestamp = DotnetDateTime::now().format("yyyyMMdd_HHmmss");
            let file_name = format!(
                "Local_{}_{timestamp}.json",
                sanitize_file_name(test_type).replace(' ', "_")
            );
            path_combine(save, &file_name)
        };
        if result.save_to_json(&file_path) {
            info!("테스트 결과가 JSON 파일로 저장되었습니다: {file_path}");
        } else {
            error!("JSON 파일 저장에 실패했습니다.");
        }
    }
}
