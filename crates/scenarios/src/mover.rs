//! `Test/MoverTest.cs`: 소스 버킷에 더미 오브젝트를 올리고 ifsMover로 타겟 버킷에 이관한 뒤 완료될 때까지 상태를 확인한다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - 소스·타겟 버킷이 없으면 만든다(`S3Client.DoesS3BucketExist` → `PutBucket`, 응답 상태는 보지 않는다). 실패하면 `false`.
//! - `FileCount`개의 더미 파일(`{FilePath}/FILE_000`…, 크기는 `Rand.NextInt64(MaxFileSize)`)을 `{ObjectPrefix}_{i}`로 최대 3번
//!   시도해 올리고, 소스 버킷의 `ListObjects` 개수가 `FileCount`와 같은지 본 뒤 Mover REST로 이관을 시작한다(`JobId = N`).
//! - 상태는 100ms마다 조회하고(`TimeWatcher(2)`가 다음 출력 시각을 알려 주면 조회 직후 요약 출력) `Status == "COMPLETE"`가 될
//!   때까지 기다린다. 제한 시간이 없다. 끝나면 상태를 한 번 더 조회해 출력하고 `true`를 돌려준다.
//! - 오류는 모두 잡아 `ERROR` 로그(`형식: 메시지`)로 남기고 `false`다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 업로드 응답 상태가 200이 아니면 `ErrorCode : {response.Expiration}`을 남긴다(오류 코드가 아니라 만료 정보).
//!   원본 `catch (AggregateException)`은 실행되지 않는다(`GetAwaiter().GetResult()`).
//! - 소스 버킷이 비어 있으면 `S3Objects`가 `null`이라 `NullReferenceException`이 로그로 남는다.
//! - 상태 조회 결과가 `null`(작업을 못 찾음)이거나 `Status`가 `null`이면 `NullReferenceException`이다.
//! - 상태 요약의 `Total Execution Time`은 `{초,6:F3} sec`이다.

use std::time::Duration;

use testrust_clients::file_util::create_random_file;
use testrust_clients::mover::{
    MoverClient, MoverError, MoverStatus, RequestMoverStart, SourceConfig, TargetConfig,
};
use testrust_common::dotnet_http::status_name;
use testrust_config::{MainConfig, MoverConfig, UserData};
use testrust_model::TimeWatcher;
use testrust_s3::S3Client;
use testrust_s3::s3_client::PutBody;
use tracing::{error, info};

use crate::ScenarioError;
use crate::input::null_reference;
use crate::util::dummy_file_name;

/// Mover 호출 오류를 .NET 예외로.
fn mover_error(error: MoverError) -> ScenarioError {
    ScenarioError::new(error.dotnet_type(), error.to_string())
}

/// 원본 `MoverTest`.
pub struct MoverTest {
    default_config: MainConfig,
    config: MoverConfig,
    user: UserData,
    watcher: TimeWatcher,
    client: S3Client,
}

impl MoverTest {
    pub fn new(default_config: MainConfig, config: MoverConfig, user: UserData) -> Self {
        let client = S3Client::from_user(&user, false, 3, false);
        Self {
            default_config,
            config,
            user,
            watcher: TimeWatcher::new(2),
            client,
        }
    }

    /// 원본 `Start`: 이관이 정상적으로 완료되면 `true`.
    pub async fn start(&mut self) -> bool {
        // 버킷 생성
        if !self.put_bucket(&self.config.source_bucket.clone()).await {
            error!("Source Bucket Create Failed!!!");
            return false;
        }
        if !self.put_bucket(&self.config.target_bucket.clone()).await {
            error!("Target Bucket Create Failed!!!");
            return false;
        }

        match self.run().await {
            Ok(done) => done,
            Err(e) => {
                error!("{e}");
                false
            }
        }
    }

    /// `Start`의 `try` 블록.
    async fn run(&mut self) -> Result<bool, ScenarioError> {
        // 오브젝트 업로드
        for i in 0..self.config.file_count {
            // 더미파일 생성
            let dummy_file = dummy_file_name(i, Some(&self.default_config.file_path));
            create_random_file(
                std::path::Path::new(&dummy_file),
                self.random_file_size()?,
                false,
            );
            let mut is_created = false;
            for _ in 0..3 {
                // 성공시 다음으로
                if self
                    .put_object(
                        &self.config.source_bucket.clone(),
                        &format!("{}_{i}", self.default_config.object_prefix),
                        &dummy_file,
                    )
                    .await
                {
                    is_created = true;
                    break;
                }
            }
            if !is_created {
                error!("Object Upload Failed!!!");
                return Ok(false);
            }
        }

        // 오브젝트 목록 확인
        let response = self
            .client
            .list_objects(&self.config.source_bucket, None, None, 1000, None)
            .await
            .map_err(|e| ScenarioError::s3(e, &["NoSuchBucket"]))?;
        let contents = response.output.contents();
        // `Response.S3Objects`가 `null`
        if contents.is_empty() {
            return Err(null_reference());
        }
        if self.config.file_count as usize != contents.len() {
            error!("Object Count is not same!!!");
            return Ok(false);
        }

        // 테스트 클라이언트 생성
        let mover = MoverClient::new(self.config.url.clone());

        // ifsMover 실행
        let start = RequestMoverStart {
            user_id: Some(self.config.user_id.clone()),
            kind: Some("s3".to_string()),
            source: Some(SourceConfig {
                endpoint: Some(self.user.url.clone()),
                bucket: Some(self.config.source_bucket.clone()),
                access: Some(self.user.access_key.clone()),
                secret: Some(self.user.secret_key.clone()),
                ..SourceConfig::default()
            }),
            target: Some(TargetConfig {
                endpoint: Some(self.user.url.clone()),
                bucket: Some(self.config.target_bucket.clone()),
                access: Some(self.user.access_key.clone()),
                secret: Some(self.user.secret_key.clone()),
                ..TargetConfig::default()
            }),
        };
        let job_id = mover.mover_start(&start).await.map_err(mover_error)?;
        if job_id < 0 {
            error!("Mover Start Failed!!!");
            return Ok(false);
        }
        info!("JobId = {job_id}");

        // ifsMover 완료 체크
        self.watcher.start();
        let mut end = false;

        while !end {
            // 상태를 가져온다
            let result = mover
                .mover_status(&self.config.user_id, job_id)
                .await
                .map_err(mover_error)?;
            // 2초가 지났을 경우 요약 정보 출력
            if self.watcher.is_next() {
                self.print(result.as_ref())?;
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            let Some(status) = result.and_then(|r| r.status) else {
                return Err(null_reference());
            };
            if status == "COMPLETE" {
                end = true;
            }
        }
        let status = mover
            .mover_status(&self.config.user_id, job_id)
            .await
            .map_err(mover_error)?;
        self.print(status.as_ref())?;
        Ok(true)
    }

    /// 원본 `config.GetRandomFileSize()`: `Rand.NextInt64(MaxFileSize)`. 음수면 `ArgumentOutOfRangeException`.
    fn random_file_size(&self) -> Result<i64, ScenarioError> {
        let max = self.config.max_file_size;
        if max < 0 {
            return Err(ScenarioError::new(
                "System.ArgumentOutOfRangeException",
                format!(
                    "maxValue ('{max}') must be a non-negative value. (Parameter 'maxValue')\nActual value was {max}."
                ),
            ));
        }
        Ok(self.config.random_file_size()?)
    }

    /// 원본 `Print`: 작업 상태를 로그로 남긴다. `status`가 `null`이면 `NullReferenceException`.
    fn print(&self, status: Option<&MoverStatus>) -> Result<(), ScenarioError> {
        let Some(status) = status else {
            return Err(null_reference());
        };
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        info!(
            "\n--------------------------------------------------------------\n Status : {}\tProgress : {}\n Total Count   : {:>6} Size : {}\n Move Count    : {:>6} Size : {}\n Skip Count    : {:>6} Size : {}\n Failed Count  : {:>6} Size : {}\n Deleted Count : {:>6} Size : {}\n Total Execution Time : {:>6.3} sec\n--------------------------------------------------------------",
            text(&status.status),
            text(&status.progress),
            status.total_count,
            text(&status.total_size),
            status.moved_count,
            text(&status.moved_size),
            status.skipped_count,
            text(&status.skipped_size),
            status.failed_count,
            text(&status.failed_size),
            status.deleted_count,
            text(&status.deleted_size),
            self.watcher.now()
        );
        Ok(())
    }

    /// 원본 `PutBucket`: 버킷이 없을 경우에만 생성한다.
    async fn put_bucket(&self, bucket_name: &str) -> bool {
        if self.client.does_s3_bucket_exist(bucket_name).await {
            return true;
        }
        match self.client.put_bucket(bucket_name, None, None, None).await {
            Ok(_) => true,
            Err(e) => {
                error!("{}", ScenarioError::s3(e, &[]));
                false
            }
        }
    }

    /// 원본 `PutObject`: 로컬 파일을 오브젝트로 업로드한다.
    async fn put_object(&self, bucket_name: &str, key: &str, file_path: &str) -> bool {
        match self
            .client
            .put_object(
                bucket_name,
                key,
                PutBody::File(file_path.into()),
                true,
                None,
            )
            .await
        {
            Ok(response) => {
                if response.status == 200 {
                    return true;
                }
                // 원본은 `response.Expiration`(만료 정보, 보통 비어 있음)을 오류 코드 자리에 쓴다.
                error!(
                    "StatusCode : {}, ErrorCode : {}",
                    status_name(response.status),
                    ""
                );
                false
            }
            Err(e) => {
                error!("{}", ScenarioError::s3(e, &[]));
                false
            }
        }
    }
}
