//! TESTCore `Client/UpDownClient.cs` 이식: 스레드 하나가 맡는 부하 테스트 클라이언트.
//!
//! 원본은 스레드마다 `UpDownClient`를 하나씩 만들어 동기 메서드(`Write`, `Read`, `Mix` ...)를 돌리고, 다른
//! 스레드가 `Stats`를 2초마다 읽는다. 여기서는 `Arc<UpDownClient>`를 tokio 작업 하나에서 돌리고 통계는
//! 원자 값으로 공유한다.
//!
//! - 측정 구간: 연산 하나(요청 + 응답 본문 전부 읽기)가 끝난 뒤 성공·실패 횟수를 올린다. 원본과 같다.
//! - 원본 메서드에서 잡지 않은 예외(목록 조회 실패, AWSSDK v4가 빈 목록을 `null`로 돌려줘 생기는
//!   `NullReferenceException`, 0으로 나누기 등)는 `Err(UpDownError)`로 돌려준다. 원본에서는 이 예외가
//!   스레드를 끝냈다. 처리는 호출하는 테스트(5단계)가 맡는다.

mod ops;
mod runs;

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use testrust_config::{EnumBucketTypes, UpDownClientConfig, UserData, UtilError};
use testrust_model::{QuitFlag, TestClient, TestStats};
use testrust_s3::{S3Client, S3Error};

/// 원본 `TAG_KEY_NAME`.
pub const TAG_KEY_NAME: &str = "TagValue";

/// 원본 메서드 밖으로 나가던 예외.
#[derive(Debug, thiserror::Error)]
pub enum UpDownError {
    /// S3 호출 예외(`AmazonS3Exception` 등).
    #[error("{}: {}", .0.dotnet_type(), .0)]
    S3(#[from] S3Error),
    /// 이름 생성 중 오류(`DivideByZeroException` 등).
    #[error("{0}")]
    Util(#[from] UtilError),
    /// `NullReferenceException`: AWSSDK v4는 빈 목록을 `null`로 둔다.
    #[error("System.NullReferenceException: Object reference not set to an instance of an object.")]
    NullReference,
    /// `ArgumentOutOfRangeException`(목록 인덱스 범위 초과).
    #[error(
        "System.ArgumentOutOfRangeException: Index was out of range. Must be non-negative and less than the size of the collection. (Parameter 'index')"
    )]
    IndexOutOfRange,
    /// `DivideByZeroException`.
    #[error("System.DivideByZeroException: Attempted to divide by zero.")]
    DivideByZero,
    /// `InvalidOperationException`(분산 Prepare 실패).
    #[error("System.InvalidOperationException: {0}")]
    InvalidOperation(String),
    /// 파일 오류.
    #[error("System.IO.IOException: {0}")]
    Io(#[from] std::io::Error),
}

/// 원본 `UpDownClient(bucketName, threadNumber, filePath, config, user)`.
pub struct UpDownClient {
    bucket_name: String,
    thread_number: i32,
    file_path: PathBuf,
    config: UpDownClientConfig,
    user: UserData,
    client: S3Client,
    object_count: AtomicI32,
    stats: TestStats,
    quit: QuitFlag,
    prepared_read_etag: std::sync::Mutex<Option<String>>,
}

impl TestClient for UpDownClient {
    fn stats(&self) -> &TestStats {
        &self.stats
    }

    fn quit(&self) -> bool {
        self.quit.get()
    }

    fn set_quit(&self, quit: bool) {
        self.quit.set(quit);
    }
}

impl UpDownClient {
    /// 종료 플래그를 상위 토큰(테스트·프로세스)에 묶는다. 상위가 취소되면 `Quit`이 된다.
    pub fn with_quit(mut self, parent: &tokio_util::sync::CancellationToken) -> Self {
        self.quit = QuitFlag::child_of(parent);
        self
    }

    pub fn new(
        bucket_name: impl Into<String>,
        thread_number: i32,
        file_path: impl Into<PathBuf>,
        config: UpDownClientConfig,
        user: UserData,
    ) -> Self {
        let client = S3Client::from_user(&user, config.is_admin, config.retry_count, false);
        Self {
            bucket_name: bucket_name.into(),
            thread_number,
            file_path: file_path.into(),
            config,
            user,
            client,
            object_count: AtomicI32::new(0),
            stats: TestStats::default(),
            quit: QuitFlag::default(),
            prepared_read_etag: std::sync::Mutex::new(None),
        }
    }

    pub fn config(&self) -> &UpDownClientConfig {
        &self.config
    }

    /// 원본 `ThreadPrefix`: `{ThreadPrefix}_{번호:000}`.
    pub fn thread_prefix(&self) -> String {
        format!("{}_{:03}", self.config.thread_prefix, self.thread_number)
    }

    fn object_count(&self) -> i32 {
        self.object_count.load(Ordering::Relaxed)
    }

    fn set_object_count(&self, value: i32) {
        self.object_count.store(value, Ordering::Relaxed);
    }

    /// 원본 `NextObjectName`: 현재 번호로 이름을 만들고 번호를 1 올린다.
    fn next_object_name(&self) -> Result<String, UpDownError> {
        let count = self.object_count.fetch_add(1, Ordering::Relaxed);
        Ok(self.config.bucket_type.next_object_name(
            &self.thread_prefix(),
            &self.config.object_prefix,
            count,
            self.config.division_count,
        )?)
    }

    /// 원본 `RandomObjectName`.
    fn random_object_name(&self) -> Result<String, UpDownError> {
        Ok(self.config.bucket_type.random_object_name(
            &self.config.object_prefix,
            &self.thread_prefix(),
            self.object_count(),
            self.config.division_count,
        )?)
    }

    /// 원본 `ListingPrefix`.
    fn listing_prefix(&self) -> String {
        let bucket_type = self.config.bucket_type;
        if bucket_type == EnumBucketTypes::Thread {
            return String::new();
        }
        if !self.config.distributed {
            return if bucket_type == EnumBucketTypes::Time {
                String::new()
            } else {
                self.thread_prefix()
            };
        }
        let separator = if bucket_type == EnumBucketTypes::One {
            "-"
        } else {
            "/"
        };
        format!("{}{separator}", self.thread_prefix())
    }

    /// 원본 `PrepareDistributedRead`.
    pub fn prepare_distributed_read(&self) -> Result<(), UpDownError> {
        if self.config.etag_check {
            let etag = crate::file_util::file_etag(&self.file_path)?;
            *self
                .prepared_read_etag
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(etag);
        }
        Ok(())
    }

    /// 원본 `NewS3Client()`.
    fn new_s3_client(&self) -> S3Client {
        S3Client::from_user(
            &self.user,
            self.config.is_admin,
            self.config.retry_count,
            false,
        )
    }

    /// `ETagCheck`이면 파일의 MD5.
    fn file_etag_if_checked(&self) -> Result<Option<String>, UpDownError> {
        if self.config.etag_check {
            Ok(Some(crate::file_util::file_etag(&self.file_path)?))
        } else {
            Ok(None)
        }
    }
}
