//! `Test/MultiDownloadTest.cs`: 오브젝트 목록을 만든 뒤 여러 작업으로 병렬 다운로드한다.
//!
//! - [`MultiDownloadTest::start`]: 버킷의 `prefix` 아래 오브젝트를 `ListObjects`(마커)로 모두 조회해 큐에 담는다
//!   (디렉터리 구분 문자로 끝나는 키·공백 키는 건너뛴다).
//! - [`MultiDownloadTest::file`]: 키 목록 파일을 한 줄씩 읽어 큐에 담는다(`./` 접두어 제거, 빈 줄과 디렉터리 구분 문자로 끝나는
//!   줄은 건너뛴다).
//! - 이어서 다운로드 루트 아래에 `00..FF/00..FF` 하위 디렉터리 65,536개를 미리 만들고, 작업마다 자기 `S3Client`로 큐에서 키를
//!   꺼내 `GetObject`한 뒤 `{루트}/{XX}/{YY}/{HASH}`(`HASH` = `MD5("{bucket}/{key}")` 대문자 16진수)에 저장한다.
//!   주 스레드는 큐가 빌 때까지 2초마다 진행 상황을 출력하고, 마지막에 한 번 더 출력한다.
//!
//! 저장 경로는 TESTCore HEAD(`c83e35f`)에서 고친 `CreateDownloadPath`를 따른다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - `TotalCount`는 목록의 모든 항목(건너뛴 것과 파일의 빈 줄 포함)을 센다. 그래서 `Read`는 처리한 수가 아니라
//!   `TotalCount - 남은 수`다.
//! - AWSSDK v4는 빈 목록을 `null`로 돌려주므로 오브젝트가 하나도 없는 응답은 `S3Objects.Count`에서
//!   `NullReferenceException`이 나 로그를 남기고 목록 조회를 끝낸다(`No objects found` 로그는 나오지 않는다).
//! - `Print`는 경과 시간이 0이면 `DivideByZeroException`을 던진다(시나리오가 이 오류로 끝난다).
//! - `Created {downloadPath} directories`의 시간은 목록 조회 시간을 포함한 누적 값이다(`File`은 `Watcher`를 다시
//!   시작하지 않아 `Print`의 시간도 누적이다).
//! - 하위 디렉터리 생성이 실패하면 오류 한 줄만 남기고 나머지는 만들지 않는다.
//! - 다운로드 저장(`SaveFile`)이 실패해도 `TotalSize`에는 `ContentLength`가 더해진다.
//!
//! 실행 비교 사례(`tests/parity/cli/run/scenarios/multi_download/`)는 다운로드 폴더에 `00`이라는 파일을 미리 두어
//! 첫 하위 디렉터리(`00/00`) 생성이 바로 실패하게 한다. 65,536개를 실제로 만들면 원본 기준으로도 수 분이 걸린다.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rust_decimal::Decimal;
use testrust_common::dotnet_format::{align, fixed, fixed_aligned};
use testrust_config::UserData;
use testrust_model::TimeWatcher;
use testrust_model::units::file_size_unit;
use testrust_s3::S3Client;
use tracing::{error, info};

use crate::ScenarioError;
use crate::files::{create_dir_error, io_error, save_file};
use crate::input::{decode_text, null_reference};
use crate::util::md5_hex_from_string;

/// 원본 `FORMAT_VALUE`.
const FORMAT_VALUE: i32 = 9;

/// 원본 `Path.DirectorySeparatorChar`.
const SEPARATOR: char = std::path::MAIN_SEPARATOR;

/// 원본 `MultiDownloadTest`.
pub struct MultiDownloadTest {
    user: UserData,
    watcher: TimeWatcher,
    file_list: Arc<Mutex<VecDeque<String>>>,
    total_count: i64,
    total_size: Arc<AtomicI64>,
}

impl MultiDownloadTest {
    /// 원본 `MultiDownloadTest(UserData User)`.
    pub fn new(user: UserData) -> Self {
        Self {
            user,
            watcher: TimeWatcher::new(0),
            file_list: Arc::new(Mutex::new(VecDeque::new())),
            total_count: 0,
            total_size: Arc::new(AtomicI64::new(0)),
        }
    }

    fn remaining(&self) -> usize {
        self.file_list.lock().expect("큐 잠금").len()
    }

    /// 원본 `Start(threadCount, bucketName, directory, downloadPath)`: 버킷 목록을 받아 다운로드한다.
    pub async fn start(
        &mut self,
        thread_count: i32,
        bucket_name: &str,
        directory: Option<&str>,
        download_path: &str,
    ) -> Result<(), ScenarioError> {
        let mut is_truncated = true;
        let client = S3Client::from_user(&self.user, false, 3, false);
        let mut marker = String::new();
        self.watcher.start();
        while is_truncated {
            match self
                .list_page(&client, bucket_name, directory, &mut marker)
                .await
            {
                Ok(Some(truncated)) => is_truncated = truncated,
                Ok(None) => break,
                Err(e) => {
                    error!("{e}");
                    break;
                }
            }
        }
        let listing_time = self.watcher.now();

        info!(
            "Downloaded {} objects. Elapsed: {} sec",
            self.total_count,
            fixed(listing_time, 4)
        );

        // 디렉토리 생성
        create_directory(download_path).await;
        let create_time = self.watcher.now();
        info!(
            "Created {download_path} directories. Elapsed: {} sec",
            fixed(create_time, 4)
        );

        self.watcher.start();
        self.download_all(thread_count, bucket_name, download_path)
            .await
    }

    /// `ListObjects` 한 쪽을 받아 큐에 담는다. 이어서 조회해야 하면 `Some(true)`, 끝이면 `Some(false)`,
    /// 오브젝트가 없어 그만두면 `None`.
    async fn list_page(
        &mut self,
        client: &S3Client,
        bucket_name: &str,
        directory: Option<&str>,
        marker: &mut String,
    ) -> Result<Option<bool>, ScenarioError> {
        let response = client
            .list_objects(bucket_name, directory, Some(marker), 1000, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &["NoSuchBucket"]))?;
        let is_truncated = response.output.is_truncated() == Some(true);
        let contents = response.output.contents();
        // .NET SDK는 잘린 응답에 `NextMarker`가 없으면 마지막 키를 쓴다.
        *marker = response
            .output
            .next_marker()
            .or_else(|| {
                is_truncated
                    .then(|| contents.last().and_then(|o| o.key()))
                    .flatten()
            })
            .unwrap_or_default()
            .to_string();
        // 빈 목록은 `null`이라 `.Count`에서 `NullReferenceException`.
        if contents.is_empty() {
            return Err(null_reference());
        }
        self.total_count += contents.len() as i64;
        if self.total_count == 0 {
            info!(
                "No objects found in {bucket_name}/{}",
                directory.unwrap_or_default()
            );
            return Ok(None);
        }
        if is_truncated {
            info!("NextMarker: {marker}, Count: {}", self.total_count);
        }

        let mut list = self.file_list.lock().expect("큐 잠금");
        for object in contents {
            let key = object.key().unwrap_or_default();
            if key.trim().is_empty() || key.ends_with(SEPARATOR) {
                continue;
            }
            list.push_back(key.to_string());
        }
        Ok(Some(is_truncated))
    }

    /// 원본 `File(threadCount, bucketName, filePath, downloadPath)`: 키 목록 파일을 읽어 다운로드한다.
    pub async fn file(
        &mut self,
        thread_count: i32,
        bucket_name: &str,
        file_path: &str,
        download_path: &str,
    ) -> Result<(), ScenarioError> {
        self.watcher.start();

        // 파일에서 목록 읽기
        let full = crate::files::full_path(file_path);
        let bytes = std::fs::read(&full).map_err(|e| io_error(&full, &e))?;
        for line in read_lines(&decode_text(&bytes)) {
            self.total_count += 1;
            if line.trim().is_empty() {
                continue;
            }
            if line.ends_with(SEPARATOR) {
                continue;
            }
            let line = line.strip_prefix("./").unwrap_or(line);
            let key = line.replace(['\r', '\n'], "");
            if key.trim().is_empty() {
                continue;
            }
            self.file_list.lock().expect("큐 잠금").push_back(key);
        }

        let listing_time = self.watcher.now();
        info!(
            "Downloaded {} objects. Elapsed: {} sec",
            self.total_count,
            fixed(listing_time, 4)
        );

        // 디렉토리 생성
        create_directory(download_path).await;
        let create_time = self.watcher.now();
        info!(
            "Created {download_path} directories. Elapsed: {} sec",
            fixed(create_time, 4)
        );

        self.download_all(thread_count, bucket_name, download_path)
            .await
    }

    /// 두 시나리오가 함께 쓰는 뒷부분: 작업을 띄우고, 큐가 빌 때까지 2초마다 출력하고, 마지막에 한 번 더 출력한다.
    async fn download_all(
        &mut self,
        thread_count: i32,
        bucket_name: &str,
        download_path: &str,
    ) -> Result<(), ScenarioError> {
        let mut tasks = Vec::new();
        for _ in 0..thread_count {
            let worker = Worker {
                user: self.user.clone(),
                bucket_name: bucket_name.to_string(),
                dir_path: download_path.to_string(),
                file_list: self.file_list.clone(),
                total_size: self.total_size.clone(),
            };
            tasks.push(tokio::spawn(worker.run()));
        }

        while self.remaining() != 0 {
            // 2초가 지났을 경우 요약 정보 출력
            if self.watcher.is_next() {
                self.print()?;
            } else {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }

        for task in tasks {
            let _ = task.await;
        }
        self.print()
    }

    /// 원본 `Print()`: 남은 개수, 다운로드 개수, 평균 처리량, 대역폭 등 진행 상황을 로그로 출력한다.
    fn print(&self) -> Result<(), ScenarioError> {
        let times = self.watcher.now();
        let remaining = self.remaining() as i64;
        let read = self.total_count - remaining;

        let divide_by_zero = || {
            ScenarioError::new(
                "System.DivideByZeroException",
                "Attempted to divide by zero.",
            )
        };
        let read_average = Decimal::from(read)
            .checked_div(times)
            .ok_or_else(divide_by_zero)?;
        let read_bandwidth = Decimal::from(self.total_size.load(Ordering::SeqCst))
            .checked_div(times)
            .ok_or_else(divide_by_zero)?;

        info!(
            "\n--------------------------------------------------------------\n Remaining     : {}\n Read          : {}\n Read Average  : {} file/sec\n Bandwidth     : {}/s\n Times         : {} sec\n--------------------------------------------------------------",
            align(remaining.to_string(), FORMAT_VALUE),
            align(read.to_string(), FORMAT_VALUE),
            fixed_aligned(read_average, FORMAT_VALUE, 3),
            file_size_unit(read_bandwidth, false, false),
            fixed_aligned(times, FORMAT_VALUE, 3)
        );
        Ok(())
    }
}

/// 원본 `DownloadObjects(bucketName, dirPath)`를 도는 작업 하나.
struct Worker {
    user: UserData,
    bucket_name: String,
    dir_path: String,
    file_list: Arc<Mutex<VecDeque<String>>>,
    total_size: Arc<AtomicI64>,
}

impl Worker {
    /// 큐에 남은 오브젝트 키가 없어질 때까지 하나씩 꺼내 다운로드하여 로컬에 저장한다.
    async fn run(self) {
        let mut dir_path = self.dir_path.clone();
        if !dir_path.ends_with(SEPARATOR) {
            dir_path.push(SEPARATOR);
        }
        let client = S3Client::from_user(&self.user, false, 3, false);
        loop {
            let key = self.file_list.lock().expect("큐 잠금").pop_front();
            let Some(key) = key else {
                break;
            };
            if let Err(e) = self.download(&client, &dir_path, &key).await {
                error!("{e}");
            }
        }
    }

    async fn download(
        &self,
        client: &S3Client,
        dir_path: &str,
        key: &str,
    ) -> Result<(), ScenarioError> {
        let response = client
            .get_object(&self.bucket_name, key, None, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &["NoSuchKey", "InvalidObjectState"]))?;
        let download_path = create_download_path(dir_path, &self.bucket_name, key);
        let content_length = response.output.content_length().unwrap_or(0);
        save_file(Some(&download_path), response.output.body).await;
        self.total_size.fetch_add(content_length, Ordering::SeqCst);
        Ok(())
    }
}

/// 원본 `CreateDirectory(downloadPath)`: 분산 저장용 `00..FF`/`00..FF` 하위 디렉터리를 미리 만든다.
/// 실패하면 오류 한 줄을 남기고 나머지는 만들지 않는다.
async fn create_directory(download_path: &str) {
    let download_path = download_path.to_string();
    let result = tokio::task::spawn_blocking(move || create_directories(&download_path, 256, 256))
        .await
        .expect("디렉터리 생성 작업은 패닉하지 않는다");
    if let Err(e) = result {
        error!("{e}");
    }
}

/// `{루트}/{XX}/{YY}`(`XX < outer`, `YY < inner`, 대문자 16진수 두 자리)를 차례로 만든다. 원본은 `256`, `256`이다.
fn create_directories(download_path: &str, outer: u32, inner: u32) -> Result<(), ScenarioError> {
    for i in 0..outer {
        for j in 0..inner {
            let path = format!("{download_path}{SEPARATOR}{i:02X}{SEPARATOR}{j:02X}");
            let full = crate::files::full_path(&path);
            if !full.is_dir() {
                std::fs::create_dir_all(&full).map_err(|e| create_dir_error(&full, &e))?;
            }
        }
    }
    Ok(())
}

/// 원본 `CreateDownloadPath(rootPath, bucketName, key)`: 버킷과 키의 MD5로 분산 저장 경로를 만든다.
fn create_download_path(root_path: &str, bucket_name: &str, key: &str) -> String {
    let key_path = md5_hex_from_string(&format!("{bucket_name}/{key}")).to_uppercase();
    format!(
        "{root_path}/{}/{}/{key_path}",
        &key_path[..2],
        &key_path[2..4]
    )
}

/// `StreamReader.ReadLine()`을 반복한 결과: `\r\n`, `\r`, `\n`으로 나누고 마지막 줄바꿈 뒤의 빈 조각은 버린다.
fn read_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                lines.push(&text[start..i]);
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_follow_stream_reader() {
        assert_eq!(read_lines("a\r\nb\nc\rd"), ["a", "b", "c", "d"]);
        assert_eq!(read_lines("a\n\nb\n"), ["a", "", "b"]);
        assert_eq!(read_lines(""), Vec::<&str>::new());
        assert_eq!(read_lines("\r\n"), [""]);
    }

    #[test]
    fn creates_hex_directory_grid() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("dl").display().to_string();
        create_directories(&base, 17, 2).unwrap();
        for (outer, inner) in [("00", "00"), ("00", "01"), ("0F", "00"), ("10", "01")] {
            assert!(root.path().join("dl").join(outer).join(inner).is_dir());
        }
        assert!(!root.path().join("dl/00/02").exists());
        assert!(!root.path().join("dl/11").exists());
        // 이미 있는 디렉터리는 그대로 두고, 파일이 막고 있으면 원본 예외 메시지로 멈춘다.
        create_directories(&base, 1, 1).unwrap();
        std::fs::write(root.path().join("dl/00/01x"), "").unwrap();
        let blocked = root.path().join("blocked").display().to_string();
        std::fs::write(&blocked, "").unwrap();
        let error = create_directories(&blocked, 1, 1).unwrap_err();
        assert_eq!(error.dotnet_type, "System.IO.IOException");
        assert!(error.message.starts_with(&format!(
            "Cannot create '{blocked}' because a file or directory"
        )));
    }

    #[test]
    fn download_path_is_spread_by_md5() {
        let path = create_download_path("root/", "bucket", "a/b.txt");
        let hash = md5_hex_from_string("bucket/a/b.txt").to_uppercase();
        assert_eq!(path, format!("root//{}/{}/{hash}", &hash[..2], &hash[2..4]));
    }
}
