//! `Test/MultiSystemTest.cs`: 통합 게이트웨이(MultiGateway)와 구·신 시스템에 파일을 올리고, 게이트웨이로
//! 목록·읽기·삭제가 일관되게 동작하는지 확인하는 테스트(List·Prepare·PutGet·Mix).
//!
//! 원본과 같게 맞춘 동작
//!
//! - 실행은 `RunWithShutdown`이다. Ctrl+C는 테스트의 `_quit`을 켜고(여기서는 실행 동안 Ctrl+C 처리기에 등록한 테스트 토큰),
//!   끝나면(예외로 끝나도) `StopTasks`로 모든 클라이언트를 멈추고 스레드를 기다린다.
//! - `ListCore`는 TESTCore c83e35f에서 고친 판(기대 이름을 서수 순서로 정렬하고, 페이지를 넘겨도 개수를 이어서 센다)이다.
//! - 진행 출력의 시간은 `TimeWatcher.Now`(종료 시간 없음)다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 스레드별 버킷(`BucketType == Thread`)이어도 `_mainConfig.BucketName` 하나만 만들고(지역 변수 `bucketName`은
//!   `_bucketName`에 반영되지 않는다), 클라이언트는 항상 `_bucketName`을 쓴다.
//! - `Prepare`·`PutGet` 진행 출력은 모두 `PrintPrepare`(쓰기만 센다). `PrintPrepareMultipart`는 파트 수를 세지만
//!   출력하지 않아 `PrintPrepare`와 같은 출력이다.
//! - `List`의 `Prepare` 단계는 `skip`이 아니면 항상 게이트웨이 단일 업로드(`Prepare()`)다.
//! - `Average`는 호출마다 값을 쌓으므로 최종 출력도 평균에 한 번 더 반영된다.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rust_decimal::Decimal;
use std::path::Path;
use testrust_clients::file_util::create_random_file;
use testrust_clients::multi_system::MultiSystemClient;
use testrust_common::dotnet_format::{align, decimal_text, fixed_aligned};
use testrust_config::enum_bucket_types::DEFAULT_DIVISION_COUNT;
use testrust_config::{
    EnumBucketTypes, MainConfig, MultiSystemClientConfig, MultiSystemConfig, UserData,
};
use testrust_model::units::file_size_unit;
use testrust_model::up_down_stats::log_info;
use testrust_model::{Average, TimeWatcher};
use testrust_s3::S3Client;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::ScenarioError;
use crate::runner::{TestTasks, idle};
use crate::shutdown::{Handler, activate};
use crate::util::dummy_file_name;

const SEPARATOR: &str = "\n--------------------------------------------------------------";
const FORMAT_VALUE: i32 = 9;
const FORMAT_ERROR_VALUE: i32 = 5;
const FORMAT_AVERAGE_VALUE: i32 = 6;

/// 스레드가 돌릴 클라이언트 동작.
#[derive(Debug, Clone, Copy)]
enum Work {
    Prepare,
    PrepareMultipart,
    PutGet,
    PutGetMultipart,
    Mix,
    MixMultipart,
}

/// 진행 상황 출력 종류.
#[derive(Debug, Clone, Copy)]
enum Print {
    Prepare,
    All,
}

/// 실행할 테스트.
#[derive(Debug, Clone, Copy)]
enum Action {
    List(bool),
    Prepare(bool),
    PutGet(bool),
    Mix(bool),
}

/// 원본 `MultiSystemTest`.
pub struct MultiSystemTest {
    tasks: TestTasks<MultiSystemClient>,
    /// 원본 `_quit`: 프로세스 토큰의 자식. 실행 동안 Ctrl+C 처리기에 등록한다(`shutdown::activate`).
    token: CancellationToken,
    watcher: TimeWatcher,
    main_config: MainConfig,
    config: MultiSystemConfig,
    client_config: MultiSystemClientConfig,
    bucket_name: String,
    multi_gateway: S3Client,
    old_system: S3Client,
    new_system: S3Client,
    one_min_read: Average,
    one_min_write: Average,
    one_min_delete: Average,
}

/// `int`를 `decimal`로.
fn decimal(value: i32) -> Decimal {
    Decimal::from(value)
}

/// `string.CompareOrdinal`(UTF-16 코드 단위 순서).
fn ordinal(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

impl MultiSystemTest {
    /// `cancel`은 프로세스 토큰(테스트 토큰의 상위).
    pub fn new(
        main_config: &MainConfig,
        config: &MultiSystemConfig,
        user: &UserData,
        bucket_name: &str,
        cancel: &CancellationToken,
    ) -> Self {
        let client = |user: UserData| S3Client::from_user(&user, false, 3, false);
        let client_config = MultiSystemClientConfig::new(
            &main_config.thread_prefix,
            &main_config.object_prefix,
            config.file_count,
            main_config.file_size,
            main_config.part_size,
            config.bucket_type,
        );
        Self {
            tasks: TestTasks::new(),
            token: cancel.child_token(),
            watcher: TimeWatcher::new(0),
            main_config: main_config.clone(),
            config: config.clone(),
            client_config,
            bucket_name: bucket_name.to_string(),
            multi_gateway: client(config.multi_gateway(user)),
            old_system: client(config.old_system(user)),
            new_system: client(config.new_system(user)),
            one_min_read: Average::default(),
            one_min_write: Average::default(),
            one_min_delete: Average::default(),
        }
    }

    /// 여러 시스템에 파일을 업로드하여 ListObject가 올바르게 동작하는지 확인하는 테스트(원본 `List`).
    pub async fn list(&mut self, skip: bool) -> Result<(), ScenarioError> {
        self.run_with_shutdown(Action::List(skip)).await
    }

    /// 여러 시스템에 파일을 업로드하는 테스트(원본 `Prepare`).
    pub async fn prepare(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        self.run_with_shutdown(Action::Prepare(multipart)).await
    }

    /// 업로드·다운로드가 동작하는지 확인하는 테스트(원본 `PutGet`).
    pub async fn put_get(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        self.run_with_shutdown(Action::PutGet(multipart)).await
    }

    /// 업로드·다운로드·삭제가 올바르게 동작하는지 확인하는 테스트(원본 `Mix`).
    pub async fn mix(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        self.run_with_shutdown(Action::Mix(multipart)).await
    }

    /// 원본 `RunWithShutdown`: Ctrl+C를 종료 요청으로 전달하고, 끝나면 작업 스레드를 정리한다.
    async fn run_with_shutdown(&mut self, action: Action) -> Result<(), ScenarioError> {
        // 실행 동안만 Ctrl+C를 `_quit`으로 받는다(끝나면 처리기를 지운다).
        let _handler = activate(&self.token, Handler::Scoped);
        let result = match action {
            Action::List(skip) => self.list_core(skip).await,
            Action::Prepare(multipart) => self.prepare_core(multipart).await,
            Action::PutGet(multipart) => self.put_get_core(multipart).await,
            Action::Mix(multipart) => self.mix_core(multipart).await,
        };
        self.stop_tasks().await;
        result
    }

    /// 원본 `_quit`.
    fn quit(&self) -> bool {
        self.token.is_cancelled()
    }

    /// 원본 `StopTasks()`: 신규 작업을 중지하고 이미 시작한 요청이 마무리될 때까지 기다린다.
    async fn stop_tasks(&mut self) {
        self.tasks.stop();
        self.tasks.join().await;
    }

    /// 스레드별 버킷이 아니면 구·신 시스템에 버킷을 만든다.
    async fn create_buckets(&self) {
        if self.config.bucket_type != EnumBucketTypes::Thread {
            self.old_system.create_bucket(&self.bucket_name).await;
            self.new_system.create_bucket(&self.bucket_name).await;
        }
    }

    /// 스레드(클라이언트)를 만든다. 도중에 `_quit`이면 `false`(원본 `if (_quit) return;`).
    async fn add_clients(&mut self, work: Work) -> bool {
        for index in 0..self.config.thread_count {
            if self.quit() {
                return false;
            }
            // 스레드별 버킷일 경우 버킷 생성(원본은 `_mainConfig.BucketName` 하나만 만든다)
            if self.config.bucket_type == EnumBucketTypes::Thread {
                self.old_system
                    .create_bucket(&self.main_config.bucket_name)
                    .await;
                self.new_system
                    .create_bucket(&self.main_config.bucket_name)
                    .await;
            }
            // 더미파일 생성
            let dummy_file = dummy_file_name(index, Some(&self.main_config.file_path));
            create_random_file(Path::new(&dummy_file), self.main_config.file_size, false);

            // 테스트 클래스와 스레드 생성
            let client = Arc::new(
                MultiSystemClient::new(
                    self.client_config.clone(),
                    &self.bucket_name,
                    index,
                    dummy_file,
                    self.multi_gateway.clone(),
                    self.old_system.clone(),
                    self.new_system.clone(),
                )
                .with_quit(&self.token),
            );
            let worker = client.clone();
            match work {
                Work::Prepare => self
                    .tasks
                    .add(client, async move { worker.prepare().await }),
                Work::PrepareMultipart => self
                    .tasks
                    .add(client, async move { worker.prepare_multipart().await }),
                Work::PutGet => self
                    .tasks
                    .add(client, async move { worker.put_get().await }),
                Work::PutGetMultipart => self
                    .tasks
                    .add(client, async move { worker.put_get_multipart().await }),
                Work::Mix => self.tasks.add(client, async move { worker.mix().await }),
                Work::MixMultipart => self
                    .tasks
                    .add(client, async move { worker.mix_multipart().await }),
            }
        }
        true
    }

    /// 원본 `TaskStart()`: 생성된 테스트 스레드를 모두 시작한다.
    async fn task_start(&mut self) -> bool {
        if self.quit() {
            return false;
        }
        if self.tasks.is_empty() {
            error!("Task Start Failed");
            return false;
        }
        self.tasks.start().await;
        true
    }

    /// 감시 루프(원본 `while (TaskCheck())`): 2초마다 진행 상황을 출력한다.
    async fn watch(&mut self, print: Print) {
        loop {
            // 원본 `TaskCheck()`: 종료 요청이면 스레드를 정리하고 끝낸다.
            if self.quit() {
                self.stop_tasks().await;
                break;
            }
            if !self.tasks.check() {
                break;
            }
            if self.watcher.is_next() {
                self.print(print);
            } else {
                idle().await;
            }
        }
    }

    /// 업로드 후 게이트웨이의 목록이 기대한 오브젝트 이름과 순서대로 같은지 확인한다(원본 `ListCore`).
    async fn list_core(&mut self, skip: bool) -> Result<(), ScenarioError> {
        info!("Start MultiSystem ListObjects Test");
        // 버킷 생성
        self.create_buckets().await;

        // 파일 업로드
        if !skip {
            info!("Prepare Initialize");
            if !self.add_clients(Work::Prepare).await {
                return Ok(());
            }
            info!("Prepare Start");
            if !self.task_start().await {
                return Ok(());
            }
            self.watcher.start();
            self.watch(Print::Prepare).await;
            // 최종 결과 출력
            self.print(Print::Prepare);
            info!("Prepare End");
        }

        // 파일 목록 확인
        if self.quit() {
            return Ok(());
        }
        // 스레드별로 업로드한 오브젝트 이름 목록(ListObjects 정렬 순서)
        let mut expected: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for thread_number in 0..self.config.thread_count {
            for object_count in 0..self.config.file_count {
                let name = self.config.bucket_type.next_object_name(
                    &format!("{}_{thread_number:03}", self.main_config.thread_prefix),
                    &self.main_config.object_prefix,
                    object_count,
                    DEFAULT_DIVISION_COUNT,
                )?;
                // Distinct: 처음 나온 것만 남긴다.
                if seen.insert(name.clone()) {
                    expected.push(name);
                }
            }
        }
        expected.sort_by(|a, b| ordinal(a, b));

        let total_count = expected.len();
        let mut count = 0usize;
        let mut matched = true;
        let mut next_marker = String::new();

        while !self.quit() {
            let response = self
                .multi_gateway
                .list_objects(&self.bucket_name, None, Some(&next_marker), 1000, None)
                .await?;
            if self.quit() {
                return Ok(());
            }
            let list = response.output.contents();

            for item in list {
                if self.quit() {
                    return Ok(());
                }
                let object_name = if count < total_count {
                    expected[count].as_str()
                } else {
                    ""
                };
                let key = item.key().unwrap_or_default();
                if key != object_name {
                    error!("File name is not matched. Expected: {object_name}, Actual: {key}");
                    matched = false;
                    break;
                }
                count += 1;
            }
            if matched
                && response.output.is_truncated().unwrap_or(false)
                && let Some(last) = list.last()
            {
                next_marker = last.key().unwrap_or_default().to_string();
            } else {
                break;
            }
        }

        if self.quit() {
            return Ok(());
        }
        if matched && count == total_count {
            info!("MultiSystemUploadTest Success");
        } else {
            error!("MultiSystemUploadTest Failed");
        }
        Ok(())
    }

    /// 원본 `PrepareCore`.
    async fn prepare_core(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        info!("Start MultiSystem Prepare Test");
        self.create_buckets().await;

        info!("Initialize");
        let work = if multipart {
            Work::PrepareMultipart
        } else {
            Work::Prepare
        };
        if !self.add_clients(work).await {
            return Ok(());
        }
        info!("Start");
        if !self.task_start().await {
            return Ok(());
        }
        self.watcher.start();
        // 모든 스레드가 오브젝트를 업로드 할때까지 반복(멀티파트용 출력도 같은 내용이다)
        self.watch(Print::Prepare).await;
        self.print(Print::Prepare);
        info!("End");
        Ok(())
    }

    /// 원본 `PutGetCore`.
    async fn put_get_core(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        info!("Start MultiSystem PutGet Test");
        self.create_buckets().await;

        info!("Initialize");
        let work = if multipart {
            Work::PutGetMultipart
        } else {
            Work::PutGet
        };
        if !self.add_clients(work).await {
            return Ok(());
        }
        info!("Start");
        if !self.task_start().await {
            return Ok(());
        }
        self.watcher.start();
        self.watch(Print::Prepare).await;
        self.print(Print::Prepare);
        info!("End");
        Ok(())
    }

    /// 원본 `MixCore`.
    async fn mix_core(&mut self, multipart: bool) -> Result<(), ScenarioError> {
        info!("Start MultiSystem Mix Initialize");
        self.create_buckets().await;

        let work = if multipart {
            Work::MixMultipart
        } else {
            Work::Mix
        };
        if !self.add_clients(work).await {
            return Ok(());
        }
        info!("Test Start");
        if !self.task_start().await {
            return Ok(());
        }
        self.watcher.start();
        self.watch(Print::All).await;
        self.print(Print::All);
        info!("Mix End");
        Ok(())
    }

    fn print(&mut self, print: Print) {
        match print {
            Print::Prepare => self.print_prepare(),
            Print::All => self.print_all(),
        }
    }

    /// 원본 `PrintPrepare()`.
    fn print_prepare(&mut self) {
        let times = self.watcher.now();
        let write_count: Decimal = self
            .tasks
            .clients()
            .iter()
            .map(|c| decimal(c.write_count.load(Ordering::Relaxed)))
            .sum();
        self.one_min_write.add(write_count);

        let remaining_count = Decimal::from(self.config.total_file_count()) - write_count;
        let write_average = self.one_min_write.get();
        let write_bandwidth = write_average * Decimal::from(self.main_config.file_size);

        log_info(&format!(
            "{SEPARATOR}\n Remaining Count : {}\n Write Count     : {} (+ {})\n Write Average   : {} file/sec\n Bandwidth       : {}/s\n Times           : {} sec{SEPARATOR}",
            align(decimal_text(remaining_count), FORMAT_VALUE),
            align(decimal_text(write_count), FORMAT_VALUE),
            decimal_text(self.one_min_write.get_last()),
            fixed_aligned(write_average, FORMAT_VALUE, 3),
            file_size_unit(write_bandwidth, false, false),
            fixed_aligned(times, FORMAT_VALUE, 3),
        ));
    }

    /// 원본 `PrintAll()`.
    fn print_all(&mut self) {
        let times = self.watcher.now();
        let sum = |value: fn(&MultiSystemClient) -> i32| -> Decimal {
            self.tasks.clients().iter().map(|c| decimal(value(c))).sum()
        };
        let write_count = sum(|c| c.write_count.load(Ordering::Relaxed));
        let write_error_count = sum(|c| c.write_error_count.load(Ordering::Relaxed));
        let read_count = sum(|c| c.read_count.load(Ordering::Relaxed));
        let read_error_count = sum(|c| c.read_error_count.load(Ordering::Relaxed));
        let delete_count = sum(|c| c.delete_count.load(Ordering::Relaxed));
        let delete_error_count = sum(|c| c.delete_error_count.load(Ordering::Relaxed));

        self.one_min_read.add(read_count);
        self.one_min_write.add(write_count);
        self.one_min_delete.add(delete_count);

        let total_count = read_count + write_count + delete_count;
        let total_error_count = read_error_count + write_error_count + delete_error_count;

        let read_average = self.one_min_read.get();
        let write_average = self.one_min_write.get();
        let delete_average = self.one_min_delete.get();
        let total_average = if times.is_zero() {
            Decimal::ZERO
        } else {
            total_count / times
        };

        let file_size = Decimal::from(self.main_config.file_size);
        let read_bandwidth = read_average * file_size;
        let write_bandwidth = write_average * file_size;

        let v = |value: Decimal| align(decimal_text(value), FORMAT_VALUE);
        let e = |value: Decimal| align(decimal_text(value), FORMAT_ERROR_VALUE);
        let f1 = |value: Decimal| fixed_aligned(value, FORMAT_AVERAGE_VALUE, 1);
        log_info(&format!(
            "\n--------------------------------------------------------------\n Read Count   : {} (+ {}) Error : {} Average : {} file/sec Bandwidth : {}/s\n Write Count  : {} (+ {}) Error : {} Average : {} file/sec Bandwidth : {}/s\n Delete Count : {} (+ {}) Error : {} Average : {} file/sec\n Total Count  : {} Error : {} Average : {} file/sec\n Total Execution Time : {} sec{SEPARATOR}",
            v(read_count),
            v(self.one_min_read.get_last()),
            e(read_error_count),
            f1(read_average),
            file_size_unit(read_bandwidth, true, false),
            v(write_count),
            v(self.one_min_write.get_last()),
            e(write_error_count),
            f1(write_average),
            file_size_unit(write_bandwidth, true, false),
            v(delete_count),
            v(self.one_min_delete.get_last()),
            e(delete_error_count),
            f1(delete_average),
            v(total_count),
            e(total_error_count),
            f1(total_average),
            fixed_aligned(times, FORMAT_VALUE, 3),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinal_compares_code_units() {
        let mut names: Vec<String> = ["b", "B", "a/1", "a_1", "a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        names.sort_by(|a, b| ordinal(a, b));
        assert_eq!(names, ["B", "a", "a/1", "a_1", "b"]);
    }
}
