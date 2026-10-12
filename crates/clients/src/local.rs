//! TESTCore `Client/LocalClient.cs` 이식: S3 대신 로컬 디렉터리에 같은 부하 패턴을 실행한다.
//!
//! 파일 입출력만 하므로 원본처럼 동기 메서드로 두고, 호출하는 테스트가 스레드(`spawn_blocking`)에서 돌린다.
//!
//! 원본 그대로 둔 점:
//! - `Delete()`는 삭제에 실패한 파일을 목록에서 빼지 않아 `Quit`까지 같은 파일을 계속 시도한다.
//! - `Read()`는 대상 디렉터리 전체(다른 스레드가 만든 파일 포함)에서 임의의 파일을 고른다.
//! - 오류 로그는 원본 `log.Error(message, exception)`처럼 메시지 다음 줄에 예외(형식: 메시지)를 붙인다.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use md5::{Digest, Md5};
use rand::Rng;
use testrust_config::{UpDownClientConfig, UtilError};
use testrust_model::{QuitFlag, TestClient, TestStats};
use tracing::{error, warn};

use crate::file_util::file_etag;

/// 원본 `DEFAULT_BUFFER_SIZE`(80KB).
const DEFAULT_BUFFER_SIZE: usize = 81920;

/// 원본 `LocalClient(targetPath, threadNumber, tempFilePath, config, useMultipart)`.
pub struct LocalClient {
    target_path: PathBuf,
    thread_number: i32,
    temp_file_path: PathBuf,
    config: UpDownClientConfig,
    use_multipart: bool,
    object_count: AtomicI32,
    stats: TestStats,
    quit: QuitFlag,
}

impl TestClient for LocalClient {
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

/// 메서드 밖으로 나가던 예외(이름 생성 오류, 원본 `GetETag`의 파일 오류).
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("{0}")]
    Util(#[from] UtilError),
    #[error("{}", io_exception(.0))]
    Io(#[from] io::Error),
}

/// `log.Error(message, exception)`: 메시지 다음 줄에 예외.
fn log_exception(message: String, error: &io::Error) {
    error!("{message}\n{}", io_exception(error));
}

pub(crate) fn io_exception(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => format!("System.IO.FileNotFoundException: {error}"),
        io::ErrorKind::PermissionDenied => format!("System.UnauthorizedAccessException: {error}"),
        _ => format!("System.IO.IOException: {error}"),
    }
}

impl LocalClient {
    /// 종료 플래그를 상위 토큰(테스트·프로세스)에 묶는다. 상위가 취소되면 `Quit`이 된다.
    pub fn with_quit(mut self, parent: &tokio_util::sync::CancellationToken) -> Self {
        self.quit = QuitFlag::child_of(parent);
        self
    }

    pub fn new(
        target_path: impl Into<PathBuf>,
        thread_number: i32,
        temp_file_path: impl Into<PathBuf>,
        config: UpDownClientConfig,
        use_multipart: bool,
    ) -> Self {
        Self {
            target_path: target_path.into(),
            thread_number,
            temp_file_path: temp_file_path.into(),
            config,
            use_multipart,
            object_count: AtomicI32::new(0),
            stats: TestStats::default(),
            quit: QuitFlag::default(),
        }
    }

    fn thread_prefix(&self) -> String {
        format!("{}_{:03}", self.config.thread_prefix, self.thread_number)
    }

    fn next_object_name(&self) -> Result<String, LocalError> {
        let count = self.object_count.fetch_add(1, Ordering::Relaxed);
        Ok(self.config.bucket_type.next_object_name(
            &self.thread_prefix(),
            &self.config.object_prefix,
            count,
            self.config.division_count,
        )?)
    }

    /// 원본 `GetTargetFilePath`: `Path.Combine(targetPath, objectName)`.
    fn target_file_path(&self, object_name: &str) -> PathBuf {
        self.target_path.join(object_name)
    }

    fn etag_if_checked(&self) -> Result<Option<String>, LocalError> {
        if self.config.etag_check {
            Ok(Some(file_etag(&self.temp_file_path)?))
        } else {
            Ok(None)
        }
    }

    fn put(&self, target: &Path) -> bool {
        if self.use_multipart {
            self.put_file_multipart(&self.temp_file_path, target)
        } else {
            self.put_file(&self.temp_file_path, target)
        }
    }

    fn get(&self, target: &Path, etag: Option<&str>) -> bool {
        if self.use_multipart {
            self.get_file_multipart(target, etag)
        } else {
            self.get_file(target, etag)
        }
    }

    /// 원본 `Prepare(maxCount, check, start)`.
    pub fn prepare(&self, max_count: i32, check: bool, start: i32) -> Result<(), LocalError> {
        self.object_count.store(start, Ordering::Relaxed);
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let target = self.target_file_path(&self.next_object_name()?);
            if check
                && let Ok(meta) = fs::metadata(&target)
                && meta.is_file()
                && meta.len() as i64 == self.config.file_size
            {
                continue;
            }
            if self.put(&target) {
                self.stats.write.add_success(1);
            } else {
                error!("Failed to Create {}", target.display());
                self.quit.set(true);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `Write()`.
    pub fn write(&self) -> Result<(), LocalError> {
        while !self.quit.get() {
            let target = self.target_file_path(&self.next_object_name()?);
            if self.put(&target) {
                self.stats.write.add_success(1);
            } else {
                self.stats.write.add_error(1);
            }
        }
        Ok(())
    }

    /// 원본 `Read()`: 대상 디렉터리의 파일 중 임의로 골라 읽는다.
    pub fn read(&self) -> Result<(), LocalError> {
        let files = self.target_file_list();
        if files.is_empty() {
            warn!(
                "Thread {}: No files found in target directory",
                self.thread_number
            );
            self.quit.set(true);
            return Ok(());
        }
        while !self.quit.get() {
            let file = &files[rand::rng().random_range(0..files.len())];
            if self.get(file, None) {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        Ok(())
    }

    /// 원본 `ReadV2(maxCount, start)`.
    pub fn read_v2(&self, max_count: i32, start: i32) -> Result<(), LocalError> {
        self.object_count.store(start, Ordering::Relaxed);
        let etag = self.etag_if_checked()?;
        for _ in start..max_count {
            if self.quit.get() {
                break;
            }
            let target = self.target_file_path(&self.next_object_name()?);
            if self.get(&target, etag.as_deref()) {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PutGet()`: 쓴 파일을 바로 읽어 확인한다(`Quit` 확인은 반복 시작에서만).
    pub fn put_get(&self) -> Result<(), LocalError> {
        let etag = self.etag_if_checked()?;
        while !self.quit.get() {
            let target = self.target_file_path(&self.next_object_name()?);
            if self.put(&target) {
                self.stats.write.add_success(1);
            } else {
                self.stats.write.add_error(1);
            }
            if self.get(&target, etag.as_deref()) {
                self.stats.read.add_success(1);
            } else {
                self.stats.read.add_error(1);
            }
        }
        Ok(())
    }

    /// 원본 `Delete()`: 목록의 첫 파일부터 지운다. 실패한 파일은 목록에 남는다.
    pub fn delete(&self) -> Result<(), LocalError> {
        let mut files: VecDeque<PathBuf> = self.target_file_list().into();
        if files.is_empty() {
            warn!(
                "Thread {}: No files found in target directory",
                self.thread_number
            );
            self.quit.set(true);
            return Ok(());
        }
        while !self.quit.get() && !files.is_empty() {
            let file = files[0].clone();
            if self.delete_file(&file) {
                files.pop_front();
                self.stats.delete.add_success(1);
            } else {
                self.stats.delete.add_error(1);
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PutFile`: 원본 파일 전체를 읽어 한 번에 쓴다.
    fn put_file(&self, source: &Path, target: &Path) -> bool {
        let result = (|| -> io::Result<()> {
            create_target_dir(target)?;
            let data = fs::read(source)?;
            fs::write(target, data)
        })();
        match result {
            Ok(()) => true,
            Err(e) => {
                log_exception(
                    format!(
                        "Thread {}: PutFile failed - {}",
                        self.thread_number,
                        target.display()
                    ),
                    &e,
                );
                false
            }
        }
    }

    /// 원본 `PutFileMultipart`: 80KB 단위로 복사한다.
    fn put_file_multipart(&self, source: &Path, target: &Path) -> bool {
        let result = (|| -> io::Result<()> {
            create_target_dir(target)?;
            let mut input = fs::File::open(source)?;
            let mut output = fs::File::create(target)?;
            let mut buffer = vec![0u8; DEFAULT_BUFFER_SIZE];
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    return Ok(());
                }
                output.write_all(&buffer[..read])?;
            }
        })();
        match result {
            Ok(()) => true,
            Err(e) => {
                log_exception(
                    format!(
                        "Thread {}: PutFileMultipart failed - {}",
                        self.thread_number,
                        target.display()
                    ),
                    &e,
                );
                false
            }
        }
    }

    fn not_found(&self, target: &Path) -> bool {
        if target.is_file() {
            return false;
        }
        error!(
            "Thread {}: File not found - {}",
            self.thread_number,
            target.display()
        );
        true
    }

    fn etag_mismatch(&self, target: &Path) {
        error!(
            "Thread {}: ETag mismatch - {}",
            self.thread_number,
            target.display()
        );
    }

    /// 원본 `GetFile`: 파일 전체를 읽고, `etag`가 있으면 MD5를 비교한다.
    fn get_file(&self, target: &Path, etag: Option<&str>) -> bool {
        if self.not_found(target) {
            return false;
        }
        match fs::read(target) {
            Ok(data) => {
                if let Some(expected) = etag.filter(|e| !e.is_empty())
                    && hex::encode(Md5::digest(&data)) != expected
                {
                    self.etag_mismatch(target);
                    return false;
                }
                true
            }
            Err(e) => {
                log_exception(
                    format!(
                        "Thread {}: GetFile failed - {}",
                        self.thread_number,
                        target.display()
                    ),
                    &e,
                );
                false
            }
        }
    }

    /// 원본 `GetFileMultipart`: 80KB 단위로 읽고, `etag`가 있으면 MD5를 비교한다.
    fn get_file_multipart(&self, target: &Path, etag: Option<&str>) -> bool {
        if self.not_found(target) {
            return false;
        }
        let etag = etag.filter(|e| !e.is_empty());
        let result = (|| -> io::Result<Option<String>> {
            let mut input = fs::File::open(target)?;
            let mut buffer = vec![0u8; DEFAULT_BUFFER_SIZE];
            let mut md5 = etag.map(|_| Md5::new());
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                if let Some(md5) = md5.as_mut() {
                    md5.update(&buffer[..read]);
                }
            }
            Ok(md5.map(|m| hex::encode(m.finalize())))
        })();
        match result {
            Ok(actual) => {
                if let (Some(expected), Some(actual)) = (etag, actual)
                    && actual != expected
                {
                    self.etag_mismatch(target);
                    return false;
                }
                true
            }
            Err(e) => {
                log_exception(
                    format!(
                        "Thread {}: GetFileMultipart failed - {}",
                        self.thread_number,
                        target.display()
                    ),
                    &e,
                );
                false
            }
        }
    }

    /// 원본 `DeleteFile`.
    fn delete_file(&self, target: &Path) -> bool {
        if self.not_found(target) {
            return false;
        }
        match fs::remove_file(target) {
            Ok(()) => true,
            Err(e) => {
                log_exception(
                    format!(
                        "Thread {}: DeleteFile failed - {}",
                        self.thread_number,
                        target.display()
                    ),
                    &e,
                );
                false
            }
        }
    }

    /// 원본 `GetTargetFileList`: `Directory.GetFiles(targetPath, "*", AllDirectories)`.
    /// .NET처럼 디렉터리를 너비 우선으로 돌며, 각 디렉터리 안에서는 운영체제가 돌려주는 순서를 따른다.
    fn target_file_list(&self) -> Vec<PathBuf> {
        if !self.target_path.is_dir() {
            return Vec::new();
        }
        match list_files(&self.target_path) {
            Ok(files) => files,
            Err(e) => {
                log_exception(
                    format!("Thread {}: GetTargetFileList failed", self.thread_number),
                    &e,
                );
                Vec::new()
            }
        }
    }
}

fn create_target_dir(target: &Path) -> io::Result<()> {
    match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() && !dir.is_dir() => fs::create_dir_all(dir),
        _ => Ok(()),
    }
}

fn list_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                queue.push_back(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}
