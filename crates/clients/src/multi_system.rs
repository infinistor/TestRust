//! TESTCore `Client/MultiSystemClient.cs` 이식: 통합 게이트웨이(MultiGateway)와 구·신 시스템에 번갈아
//! 올리고 게이트웨이로 읽어 확인하는 부하 클라이언트.
//!
//! 원본 그대로 둔 점:
//! - 읽기 확인은 파일 MD5를 Base64로(`Utility.GetMD5`), 응답 본문 MD5를 16진수로(`Utility.GetETag`) 만들어
//!   비교한다. 형식이 달라 내용이 같아도 항상 `md5sum does not match`로 실패한다.
//! - 업로드는 `UseChunkEncoding` 기본값(`true`)으로 보낸다.
//! - 예외는 원본 `log.Error(e)`처럼 `형식: 메시지`로, `GetObject`는 `log.Error("GetObject(...)", e)`처럼
//!   메시지 다음 줄에 예외를 붙여 남긴다.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};

use testrust_common::dotnet_http::status_name;
use testrust_config::enum_bucket_types::DEFAULT_DIVISION_COUNT;
use testrust_config::{MultiSystemClientConfig, UtilError};
use testrust_model::QuitFlag;
use testrust_s3::s3_client::{PartETag, PutBody};
use testrust_s3::{S3Client, S3Error};
use tracing::error;

use crate::file_util::{bytes_etag, file_md5_base64};

/// 메서드 밖으로 나가던 예외(이름 생성 오류, 원본 `GetMD5`의 파일 오류).
#[derive(Debug, thiserror::Error)]
pub enum MultiSystemError {
    #[error("{0}")]
    Util(#[from] UtilError),
    #[error("{}", crate::local::io_exception(.0))]
    Io(#[from] std::io::Error),
}

/// 원본 `MultiSystemClient(config, bucketName, threadNumber, filePath, multiGateway, oldSystem, newSystem)`.
pub struct MultiSystemClient {
    config: MultiSystemClientConfig,
    bucket_name: String,
    thread_number: i32,
    file_path: PathBuf,
    multi_gateway: S3Client,
    old_system: S3Client,
    new_system: S3Client,
    quit: QuitFlag,
    pub object_count: AtomicI32,
    pub write_count: AtomicI32,
    pub write_error_count: AtomicI32,
    pub read_count: AtomicI32,
    pub read_error_count: AtomicI32,
    pub head_count: AtomicI32,
    pub head_error_count: AtomicI32,
    pub delete_count: AtomicI32,
    pub delete_error_count: AtomicI32,
    pub part_count: AtomicI32,
    pub loop_end_count: AtomicI32,
    pub found_count: AtomicI32,
}

fn inc(counter: &AtomicI32) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// 원본 `log.Error(e)`.
fn log_exception(error: &S3Error) {
    error!("{}: {error}", error.dotnet_type());
}

impl MultiSystemClient {
    /// 종료 플래그를 상위 토큰(테스트·프로세스)에 묶는다. 상위가 취소되면 `Quit`이 된다.
    pub fn with_quit(mut self, parent: &tokio_util::sync::CancellationToken) -> Self {
        self.quit = QuitFlag::child_of(parent);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: MultiSystemClientConfig,
        bucket_name: impl Into<String>,
        thread_number: i32,
        file_path: impl Into<PathBuf>,
        multi_gateway: S3Client,
        old_system: S3Client,
        new_system: S3Client,
    ) -> Self {
        Self {
            config,
            bucket_name: bucket_name.into(),
            thread_number,
            file_path: file_path.into(),
            multi_gateway,
            old_system,
            new_system,
            quit: QuitFlag::default(),
            object_count: AtomicI32::new(0),
            write_count: AtomicI32::new(0),
            write_error_count: AtomicI32::new(0),
            read_count: AtomicI32::new(0),
            read_error_count: AtomicI32::new(0),
            head_count: AtomicI32::new(0),
            head_error_count: AtomicI32::new(0),
            delete_count: AtomicI32::new(0),
            delete_error_count: AtomicI32::new(0),
            part_count: AtomicI32::new(0),
            loop_end_count: AtomicI32::new(0),
            found_count: AtomicI32::new(0),
        }
    }

    pub fn quit(&self) -> bool {
        self.quit.get()
    }

    pub fn set_quit(&self, quit: bool) {
        self.quit.set(quit);
    }

    fn thread_prefix(&self) -> String {
        format!("{}_{:03}", self.config.thread_prefix, self.thread_number)
    }

    /// 원본 `NextObjectName`(분할 수는 기본값 1000).
    fn next_object_name(&self) -> Result<String, MultiSystemError> {
        let count = self.object_count.fetch_add(1, Ordering::Relaxed);
        Ok(self.config.bucket_type.next_object_name(
            &self.thread_prefix(),
            &self.config.object_prefix,
            count,
            DEFAULT_DIVISION_COUNT,
        )?)
    }

    /// 짝수 번째는 구 시스템, 홀수 번째는 신 시스템.
    fn system(&self, index: i32) -> &S3Client {
        if index % 2 == 0 {
            &self.old_system
        } else {
            &self.new_system
        }
    }

    fn md5(&self) -> Result<String, MultiSystemError> {
        Ok(file_md5_base64(&self.file_path)?)
    }

    fn count_write(&self, ok: bool) {
        inc(if ok {
            &self.write_count
        } else {
            &self.write_error_count
        });
    }

    async fn multipart_counted(&self, client: &S3Client, object_name: &str) {
        match self.multipart_upload(client, object_name).await {
            Some(count) => {
                inc(&self.write_count);
                self.part_count.fetch_add(count, Ordering::Relaxed);
            }
            None if !self.quit.get() => inc(&self.write_error_count),
            None => {}
        }
    }

    async fn get_counted(&self, object_name: &str, md5: &str) -> bool {
        let ok = self
            .get_object_md5(&self.multi_gateway, object_name, md5)
            .await;
        inc(if ok {
            &self.read_count
        } else {
            &self.read_error_count
        });
        ok
    }

    /// 원본 `Prepare()`: 게이트웨이로 올린다.
    pub async fn prepare(&self) -> Result<(), MultiSystemError> {
        for _ in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            let ok = self.put_object(&self.multi_gateway, &object_name).await;
            self.count_write(ok);
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PrepareMultipart()`: 구·신 시스템에 번갈아 멀티파트로 올린다.
    pub async fn prepare_multipart(&self) -> Result<(), MultiSystemError> {
        for i in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.multipart_counted(self.system(i), &object_name).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `Get()`.
    pub async fn get(&self) -> Result<(), MultiSystemError> {
        let md5 = self.md5()?;
        for _ in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.get_counted(&object_name, &md5).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PutGet()`.
    pub async fn put_get(&self) -> Result<(), MultiSystemError> {
        let md5 = self.md5()?;
        for _ in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            let ok = self.put_object(&self.multi_gateway, &object_name).await;
            self.count_write(ok);
            if self.quit.get() {
                break;
            }
            self.get_counted(&object_name, &md5).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `PutGetMultipart()`.
    pub async fn put_get_multipart(&self) -> Result<(), MultiSystemError> {
        let md5 = self.md5()?;
        for i in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.multipart_counted(self.system(i), &object_name).await;
            if self.quit.get() {
                break;
            }
            self.get_counted(&object_name, &md5).await;
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `Mix()`: 번갈아 올리고 게이트웨이로 읽고, 읽기에 성공하면 지운다.
    pub async fn mix(&self) -> Result<(), MultiSystemError> {
        let md5 = self.md5()?;
        for i in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            let ok = self.put_object(self.system(i), &object_name).await;
            self.count_write(ok);
            if self.quit.get() {
                break;
            }
            if !self.read_then_delete(&object_name, &md5).await {
                break;
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 원본 `MixMultipart()`.
    pub async fn mix_multipart(&self) -> Result<(), MultiSystemError> {
        let md5 = self.md5()?;
        for i in 0..self.config.file_count {
            if self.quit.get() {
                break;
            }
            let object_name = self.next_object_name()?;
            self.multipart_counted(self.system(i), &object_name).await;
            if self.quit.get() {
                break;
            }
            if !self.read_then_delete(&object_name, &md5).await {
                break;
            }
        }
        self.quit.set(true);
        Ok(())
    }

    /// 읽기에 성공하면 지운다. 그 사이 `Quit`이 켜지면 `false`(반복 중단).
    async fn read_then_delete(&self, object_name: &str, md5: &str) -> bool {
        if self.get_counted(object_name, md5).await {
            if self.quit.get() {
                return false;
            }
            let ok = self
                .delete_object(&self.multi_gateway, object_name, None)
                .await;
            inc(if ok {
                &self.delete_count
            } else {
                &self.delete_error_count
            });
        }
        true
    }

    /// 원본 `PutObject(client, objectName)`.
    async fn put_object(&self, client: &S3Client, object_name: &str) -> bool {
        let body = PutBody::File(self.file_path.clone());
        match client
            .put_object(&self.bucket_name, object_name, body, true, None)
            .await
        {
            Ok(response) if response.status == 200 => true,
            Ok(response) => {
                error!(
                    "PutObject Failed({}). S3://{}/{object_name}",
                    status_name(response.status),
                    self.bucket_name
                );
                false
            }
            Err(e) => {
                log_exception(&e);
                false
            }
        }
    }

    /// 원본 `MultipartUpload`: 성공하면 파트 수.
    async fn multipart_upload(&self, client: &S3Client, object_name: &str) -> Option<i32> {
        if self.quit.get() {
            return None;
        }
        let bucket = &self.bucket_name;
        let result: Result<Option<i32>, S3Error> = async {
            let init = client
                .initiate_multipart_upload(bucket, object_name)
                .await?;
            if init.status != 200 {
                error!(
                    "MultipartUpload Failed({}). S3://{bucket}/{object_name}",
                    status_name(init.status)
                );
                return Ok(None);
            }
            let upload_id = init.output.upload_id().unwrap_or_default().to_string();
            let mut part_number = 1;
            let mut start = 0i64;
            let mut parts = Vec::new();
            while start < self.config.file_size && !self.quit.get() {
                let response = client
                    .upload_part(
                        bucket,
                        object_name,
                        &upload_id,
                        part_number,
                        PutBody::File(self.file_path.clone()),
                        start,
                        self.config.part_size,
                        true,
                    )
                    .await?;
                if response.status != 200 {
                    error!(
                        "MultipartUpload Failed({}). S3://{bucket}/{object_name}",
                        status_name(response.status)
                    );
                    client
                        .abort_multipart_upload(bucket, object_name, &upload_id)
                        .await?;
                    return Ok(None);
                }
                parts.push(PartETag::new(part_number, response.output.e_tag()));
                start += self.config.part_size;
                part_number += 1;
            }
            if self.quit.get() {
                client
                    .abort_multipart_upload(bucket, object_name, &upload_id)
                    .await?;
                return Ok(None);
            }
            let complete = client
                .complete_multipart_upload(bucket, object_name, &upload_id, &parts)
                .await?;
            if complete.status == 200 {
                return Ok(Some(parts.len() as i32));
            }
            error!(
                "MultipartUpload Failed({}). S3://{bucket}/{object_name}",
                status_name(complete.status)
            );
            Ok(None)
        }
        .await;
        match result {
            Ok(count) => count,
            Err(e) => {
                log_exception(&e);
                None
            }
        }
    }

    /// 원본 `GetObjectMd5`(모듈 문서의 Base64·16진수 비교 참고).
    async fn get_object_md5(&self, client: &S3Client, object_name: &str, md5: &str) -> bool {
        let bucket = &self.bucket_name;
        let response = match client.get_object(bucket, object_name, None, None).await {
            Ok(response) => response,
            Err(e) => {
                error!(
                    "GetObject({bucket}, {object_name})\n{}: {e}",
                    e.dotnet_type()
                );
                return false;
            }
        };
        if response.status != 200 {
            error!(
                "GetObject Failed({}). S3://{bucket}/{object_name}",
                status_name(response.status)
            );
            return false;
        }
        let content_length = response.output.content_length().unwrap_or(0);
        if self.config.file_size != content_length {
            error!(
                "GetObject Failed. S3://{bucket}/{object_name} - FileSize does not match!({} != {content_length})",
                self.config.file_size
            );
            return false;
        }
        let body = match response.output.body.collect().await {
            Ok(body) => body.into_bytes(),
            Err(e) => {
                error!("GetObject({bucket}, {object_name})\nSystem.IO.IOException: {e}");
                return false;
            }
        };
        let etag = bytes_etag(&body);
        if !md5.eq_ignore_ascii_case(&etag) {
            error!(
                "GetObject Failed. S3://{bucket}/{object_name} - md5sum does not match!({md5} != {etag})"
            );
            return false;
        }
        true
    }

    /// 원본 `DeleteObject`: 204면 성공.
    async fn delete_object(
        &self,
        client: &S3Client,
        object_name: &str,
        version_id: Option<&str>,
    ) -> bool {
        match client
            .delete_object(&self.bucket_name, object_name, version_id, None)
            .await
        {
            Ok(response) if response.status == 204 => true,
            Ok(response) => {
                error!(
                    "DeleteObject Failed({}). S3://{}/{object_name}_{}",
                    status_name(response.status),
                    self.bucket_name,
                    version_id.unwrap_or_default()
                );
                false
            }
            Err(e) => {
                log_exception(&e);
                false
            }
        }
    }
}
