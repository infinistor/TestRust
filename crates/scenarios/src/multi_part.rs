//! `Test/MultiPartTest.cs`: 로컬 파일을 `partSize` 단위로 미리 읽어 여러 작업으로 병렬 업로드하는 수동 멀티파트 업로드.
//!
//! 원본과 같게 맞춘 동작
//!
//! - 주 스레드가 파트를 하나씩 읽고(`SemaphoreSlim` 동시 업로드 수 대기 → 업로드 작업 시작), 읽은 버퍼만 작업에 넘긴다.
//!   파트 업로드 성공 로그(`PartNumber : n is Upload Success!`)의 순서는 완료 순서라 실행마다 다를 수 있다.
//! - 한 파트라도 실패하면(`null`) 실패 로그를 남기고 `AbortMultipartUpload`를 부른 뒤 끝낸다(완료 로그 없음).
//! - 업로드 초기화·완료·중단 요청의 예외는 잡지 않아 호출한 쪽으로 올라간다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 마지막 로그의 `complete time = {Watcher.Now}ms`는 초 단위 값(`TimeWatcher.Now`)에 `ms`를 붙인다.
//! - `partSize == 0`이면 `filePosition`이 늘지 않아 빈 파트를 끝없이 올린다. `partSize < 0`이면 `new byte[음수]`가
//!   `OverflowException`을 던진다.
//! - 파트 업로드가 실패해도 나머지 파트를 모두 읽어 올린 뒤에야 중단한다.

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use testrust_common::dotnet_format::decimal_text;
use testrust_common::dotnet_http::status_name;
use testrust_config::UserData;
use testrust_model::TimeWatcher;
use testrust_s3::s3_client::{PartETag, PutBody};
use testrust_s3::{S3Client, S3Error};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::ScenarioError;
use crate::files::{file_exists, full_path, io_error};

/// 원본 `MultiPartTest`.
pub struct MultiPartTest {
    client: S3Client,
    watcher: TimeWatcher,
}

impl MultiPartTest {
    /// 원본 `MultiPartTest(UserData user)`: 자체 `S3Client`를 만든다.
    pub fn new(user: &UserData) -> Self {
        Self {
            client: S3Client::from_user(user, false, 3, false),
            watcher: TimeWatcher::new(0),
        }
    }

    /// 원본 `Start(bucketName, key, filePath, partSize, threadCount = 10)`.
    pub async fn start(
        &mut self,
        bucket_name: &str,
        key: &str,
        file_path: &str,
        part_size: i64,
        thread_count: usize,
    ) -> Result<(), ScenarioError> {
        info!("Manual Upload Start");
        self.watcher.start();

        if !file_exists(file_path) {
            error!("파일이 존재하지 않습니다. (Path = {file_path})");
            return Ok(());
        }
        let full = full_path(file_path);
        let size = std::fs::metadata(&full)
            .map_err(|e| io_error(&full, &e))?
            .len() as i64;

        // 업로드 초기화
        let init = self
            .client
            .initiate_multipart_upload(bucket_name, key)
            .await?;
        let upload_id = init.output.upload_id().unwrap_or_default().to_string();
        info!("InitiateMultipartUpload : {upload_id}");

        // 업로드 시작
        let semaphore = Arc::new(Semaphore::new(thread_count));
        let mut tasks: Vec<JoinHandle<Option<PartETag>>> = Vec::new();
        let mut file_position = 0i64;
        let mut part_number = 1i64;
        while file_position < size {
            let bytes_to_read = part_size.min(size - file_position);
            if bytes_to_read < 0 {
                return Err(ScenarioError::new(
                    "System.OverflowException",
                    "Arithmetic operation resulted in an overflow.",
                ));
            }
            let mut buffer = vec![0u8; bytes_to_read as usize];

            // 메인 스레드에서 미리 파일 읽기(동기)
            let mut file = std::fs::File::open(&full).map_err(|e| io_error(&full, &e))?;
            file.seek(SeekFrom::Start(file_position as u64))
                .map_err(|e| io_error(&full, &e))?;
            let _read = file.read(&mut buffer).map_err(|e| io_error(&full, &e))?;

            // 이제 버퍼만 업로드 작업에 넘긴다(병렬).
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("세마포어는 닫지 않는다");
            let pn = part_number as i32;
            let client = self.client.clone();
            let (bucket, key, upload_id) =
                (bucket_name.to_string(), key.to_string(), upload_id.clone());
            tasks.push(tokio::spawn(async move {
                let result = upload_part(&client, &bucket, &key, &upload_id, pn, buffer).await;
                drop(permit);
                result
            }));

            file_position = file_position.wrapping_add(part_size);
            part_number += 1;
        }
        info!("tasks : {}. Waiting...", tasks.len());

        // 모든 업로드 작업 종료 대기
        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            results.push(task.await.expect("업로드 작업은 패닉하지 않는다"));
        }

        // 결과 처리
        let mut parts = Vec::new();
        for part in results {
            match part {
                Some(part) => parts.push(part),
                None => {
                    error!("하나 이상의 파트 업로드에 실패했습니다. 멀티파트 업로드를 중단합니다.");
                    self.client
                        .abort_multipart_upload(bucket_name, key, &upload_id)
                        .await?;
                    return Ok(());
                }
            }
        }

        // 멀티파트 업로드 종료(모든 파트 정상 업로드)
        info!("parts : {}. CompleteMultipartUpload...", parts.len());
        self.client
            .complete_multipart_upload(bucket_name, key, &upload_id, &parts)
            .await?;
        info!(
            "Manual Upload End. complete time = {}ms",
            decimal_text(self.watcher.now())
        );
        Ok(())
    }
}

/// 업로드 작업 하나: 성공하면 파트 번호와 ETag, 실패하면 로그만 남기고 `None`.
async fn upload_part(
    client: &S3Client,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    pn: i32,
    buffer: Vec<u8>,
) -> Option<PartETag> {
    let size = buffer.len() as i64;
    match client
        .upload_part(
            bucket_name,
            key,
            upload_id,
            pn,
            PutBody::Bytes(buffer),
            0,
            size,
            true,
        )
        .await
    {
        Ok(response) if response.status == 200 => {
            info!("PartNumber : {pn} is Upload Success!");
            Some(PartETag {
                part_number: Some(pn),
                e_tag: response.output.e_tag().map(str::to_string),
                ..PartETag::default()
            })
        }
        Ok(response) => {
            error!(
                "PartNumber : {pn} is Upload Failed! Status: {}",
                status_name(response.status)
            );
            None
        }
        Err(S3Error::Service { status, code, .. }) => {
            error!(
                "PartNumber : {pn} 업로드 실패. Status: {}, ErrorCode: {code}",
                status_name(status)
            );
            None
        }
        Err(e) => {
            error!("PartNumber : {pn} 업로드 실패\n{}", ScenarioError::from(e));
            None
        }
    }
}
