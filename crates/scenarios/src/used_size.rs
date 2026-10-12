//! `Test/UsedSizeTest.cs`: Put·Copy·Multipart 업로드와 삭제의 각 단계마다 DB(`bucket` 테이블)에 기록된 사용량(파일 개수·바이트)이
//! 기대와 같은지 확인한다. `Start`가 일반 시나리오, `StartVersions`가 같은 시나리오에서 복사 원본을 바꾼 판이다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - DB 연결(`DBConnect`)은 한 번 맺은 연결을 `Start`와 `StartVersions`가 함께 쓴다. 연결에 실패하면 `e.Message`만 로그로
//!   남기고 -1이다. 다만 원본은 `DB = new MySqlConnection(...)`을 `Open()` 앞에서 대입하므로 실패한 뒤에도 `DB`가 `null`이
//!   아니다. 그래서 디스패처가 이어서 부르는 `StartVersions`는 DB 연결이 된 것으로 보고 S3 단계로 들어가며(버킷 정리
//!   `ListVersions`에서 `ArgumentNullException`이 나는 것이 보통이다), 사용량 확인에 닿으면
//!   `InvalidOperationException: Connection must be valid and open.`이다.
//! - S3 오류는 `AmazonS3Exception`만 잡아 `e.Message`를 로그로 남기고 `false`다. 다른 오류(네트워크 등)는 그대로 올라간다.
//! - 사용량 조회의 SQL은 버킷 이름을 문자열로 끼워 넣는다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 같은 버킷 안 멀티파트 복사 뒤 확인(`CheckUsedSize`)이 `source`가 아닌 `target`을 본다.
//! - `MultipartCopy`의 첫 구간은 `0 ~ 5MiB`(끝 포함이라 5MiB + 1바이트)이고, 다음 시작은 `end + 1`이다.
//! - `MultipartUpload`는 `remaining > 1`인 동안 돈다(크기가 1 이하이면 파트가 없다).
//! - `StartVersions`는 같은 키를 자기 자신에게 복사한다(`CopyObject(source, key2, source, key2)` 등, 메타데이터 지시자 없이).
//! - `CleanObject`는 `ListVersions` 첫 페이지만 지우고, 객체 버전이 없으면 `response.Versions`가 `null`이라
//!   `ArgumentNullException`이 난다(.NET SDK v4는 빈 목록을 `null`로 둔다).
//! - `StartVersions`는 끝에 버킷을 지우지 않는다.
//!
//! .NET과 다른 점
//!
//! - DB 드라이버가 다르다(`MySql.Data` → `mysql_async`). 연결 오류 문구는 서버가 보낸 메시지는 그대로, 그 밖의 연결 실패
//!   (거부·시간 초과·이름 확인 실패·잘못된 포트)는 `MySql.Data`의 `Unable to connect to any of the specified MySQL hosts`
//!   (끝에 마침표가 없다)로 맞춘다. 실행 비교(`used_size/db-*`)로 확인했다.
//!   TLS는 쓰지 않는다(`MySql.Data`의 `SslMode=Preferred`는 서버가 지원하면 TLS를 쓴다).
//! - 연결 문자열을 만들지 않고 `DbConfig` 값을 바로 쓴다(비밀번호의 `;`·`=` 같은 문자가 연결 문자열 구문을 깨는 경우는 없다).

use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, OptsBuilder};
use testrust_common::dotnet_format::fixed;
use testrust_common::dotnet_http::status_name;
use testrust_config::util::random_text_long;
use testrust_config::{DbConfig, UsedSizeConfig, UserData};
use testrust_model::TimeWatcher;
use testrust_s3::s3_client::{PartETag, PutBody};
use testrust_s3::{S3Client, S3Error};
use tracing::{error, info};

use crate::ScenarioError;

/// 원본 `Utility.MiB`.
const MIB: i64 = 1024 * 1024;

/// `MySql.Data`의 연결 실패 문구.
const UNABLE_TO_CONNECT: &str = "Unable to connect to any of the specified MySQL hosts";

/// 원본 `UsageData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageData {
    pub name: String,
    pub used_size: i64,
    pub file_count: i32,
}

impl UsageData {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            used_size: 0,
            file_count: 0,
        }
    }

    pub fn add_usage(&mut self, used_size: i64) {
        self.used_size += used_size;
        self.file_count += 1;
    }

    pub fn sub_usage(&mut self, used_size: i64) {
        self.used_size -= used_size;
        self.file_count -= 1;
    }

    pub fn set_usage(&mut self, used_size: i64, file_count: i32) {
        self.used_size = used_size;
        self.file_count = file_count;
    }
}

/// 단계가 실패하면 로그를 남기고 -1을 돌려준다(원본 `{ log.Error(...); return -1; }`).
macro_rules! fail_if {
    ($cond:expr, $($arg:tt)*) => {
        if $cond {
            error!($($arg)*);
            return Ok(-1);
        }
    };
}

/// 원본 `UsedSizeTest`.
pub struct UsedSizeTest {
    config: UsedSizeConfig,
    db_config: DbConfig,
    watcher: TimeWatcher,
    client: S3Client,
    db: Db,
}

/// 원본 `MySqlConnection DB` 필드의 상태.
enum Db {
    /// `null`
    None,
    /// 대입했지만 `Open()`에 실패한 연결
    Unopened,
    Open(Conn),
}

/// `AmazonS3Exception`이면 `e.Message`를 로그로 남기고 `false`, 그 밖의 오류는 그대로 던진다.
fn caught(error: S3Error) -> Result<bool, ScenarioError> {
    if error.is_amazon_s3_exception() {
        error!("{error}");
        Ok(false)
    } else {
        Err(error.into())
    }
}

/// 드라이버 오류를 `MySqlException`으로.
fn db_error(error: mysql_async::Error) -> ScenarioError {
    let message = match &error {
        mysql_async::Error::Server(server) => server.message.clone(),
        other => other.to_string(),
    };
    ScenarioError::new("MySql.Data.MySqlClient.MySqlException", message)
}

impl UsedSizeTest {
    pub fn new(config: &UsedSizeConfig, db_config: &DbConfig, user: &UserData) -> Self {
        Self {
            config: config.clone(),
            db_config: db_config.clone(),
            watcher: TimeWatcher::new(0),
            client: S3Client::from_user(user, false, 3, false),
            db: Db::None,
        }
    }

    /// Put/Copy/Multipart 업로드 및 삭제 각 단계마다 DB에 기록된 사용량이 예상과 일치하는지 확인한다.
    /// 모든 단계 검증에 성공하면 0, 실패하면 -1.
    pub async fn start(&mut self) -> Result<i32, ScenarioError> {
        self.run(false).await
    }

    /// `start`와 같은 시나리오를 복사 원본을 바꿔 반복한다.
    pub async fn start_versions(&mut self) -> Result<i32, ScenarioError> {
        self.run(true).await
    }

    /// `Start`와 `StartVersions` 본문. 두 원본의 차이는 `versions`로 가른다.
    async fn run(&mut self, versions: bool) -> Result<i32, ScenarioError> {
        self.watcher.start();
        // DB 연결
        if !self.db_connect().await {
            return Ok(-1);
        }

        let prefix = self.config.bucket_prefix.clone();
        let file_size = self.config.file_size;
        let part_size = self.config.multi_part_size;
        let mut source = UsageData::new(format!("{prefix}-source"));
        let mut target = UsageData::new(format!("{prefix}-target"));
        let (s, t) = (source.name.clone(), target.name.clone());

        // 버킷 초기화
        self.clean_bucket(&s).await?;
        self.clean_bucket(&t).await?;

        // 버킷 생성
        self.create_bucket(&s).await?;
        self.create_bucket(&t).await?;

        // 오브젝트 업로드
        let key1 = "PutObject";
        fail_if!(!self.put_object(&s, key1).await?, "PutObject({s}) Fail");
        source.add_usage(file_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "PutObject({s}) CheckUsedSize Fail"
        );

        // 오브젝트 덮어쓰기
        fail_if!(
            !self.put_object(&s, key1).await?,
            "PutObject({s}) Overwrite Fail"
        );
        fail_if!(
            !self.check_used_size(&source).await?,
            "PutObject({s}) Overwrite CheckUsedSize Fail"
        );

        // 오브젝트 복사
        let key2 = "CopyObject";
        fail_if!(
            !self.copy_object(&s, key1, &s, key2).await?,
            "CopyObject Fail"
        );
        source.add_usage(file_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "CopyObject({s}) CheckUsedSize Fail"
        );

        // 오브젝트 복사로 덮어쓰기(StartVersions는 자기 자신에게 복사한다)
        let overwrite_source = if versions { key2 } else { key1 };
        fail_if!(
            !self.copy_object(&s, overwrite_source, &s, key2).await?,
            "CopyObject({s}) Overwrite Fail"
        );
        fail_if!(
            !self.check_used_size(&source).await?,
            "CopyObject({s}) CheckUsedSize Fail"
        );

        // 오브젝트 다른 버킷으로 복사
        let other_source = if versions { key1 } else { key2 };
        fail_if!(
            !self.copy_object(&s, other_source, &t, key2).await?,
            "CopyObject({s}, {t}) Anther Fail"
        );
        target.add_usage(file_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "CopyObject({s}) CheckUsedSize Fail"
        );
        fail_if!(
            !self.check_used_size(&target).await?,
            "CopyObject({t}) CheckUsedSize Fail"
        );

        // 멀티파트로 업로드
        let key3 = "MultipartUpload";
        fail_if!(
            !self.multipart_upload(&s, key3).await?,
            "MultipartUpload({s}) Fail"
        );
        source.add_usage(part_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "MultipartUpload({s}) CheckUsedSize Fail"
        );

        // 멀티파트로 덮어쓰기
        fail_if!(
            !self.multipart_upload(&s, key3).await?,
            "MultipartUpload({s}) Overwrite Fail"
        );
        fail_if!(
            !self.check_used_size(&source).await?,
            "MultipartUpload({s}) Overwrite CheckUsedSize Fail"
        );

        // 멀티파트 복사
        let key4 = "MultipartCopy";
        fail_if!(
            !self.multipart_copy(&s, key3, &s, key4).await?,
            "MultipartCopy({s}) Fail"
        );
        source.add_usage(part_size);
        // 원본 그대로 source가 아닌 target을 확인한다.
        fail_if!(
            !self.check_used_size(&target).await?,
            "MultipartCopy({s}) CheckUsedSize Fail"
        );

        // 멀티파트 복사로 덮어쓰기
        let multipart_overwrite_source = if versions { key4 } else { key3 };
        fail_if!(
            !self
                .multipart_copy(&s, multipart_overwrite_source, &s, key4)
                .await?,
            "MultipartCopy({s}) Overwrite Fail"
        );
        fail_if!(
            !self.check_used_size(&source).await?,
            "MultipartCopy({s}) Overwrite CheckUsedSize Fail"
        );

        // 멀티파트 다른 버킷으로 복사
        fail_if!(
            !self.multipart_copy(&s, key4, &t, key4).await?,
            "MultipartCopy({s}, {t}) Fail"
        );
        target.add_usage(part_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "MultipartCopy({s}) CheckUsedSize Fail"
        );
        fail_if!(
            !self.check_used_size(&target).await?,
            "MultipartCopy({t}) CheckUsedSize Fail"
        );

        // 오브젝트 삭제
        fail_if!(
            !self.delete_object(&s, key1).await?,
            "DeleteObject({s}) Fail"
        );
        source.sub_usage(file_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "DeleteObject({s}) CheckUsedSize Fail"
        );

        // 멀티파트 삭제
        fail_if!(
            !self.delete_object(&s, key3).await?,
            "DeleteObject({s}) Fail"
        );
        source.sub_usage(part_size);
        fail_if!(
            !self.check_used_size(&source).await?,
            "DeleteObject({s}) CheckUsedSize Fail"
        );

        // 모든 오브젝트 삭제
        fail_if!(!self.clean_object(&s).await?, "DeleteAllObject({s}) Fail");
        fail_if!(!self.clean_object(&t).await?, "DeleteAllObject({t}) Fail");
        source.set_usage(0, 0);
        target.set_usage(0, 0);
        fail_if!(
            !self.check_used_size(&source).await?,
            "DeleteAllObject({s}) CheckUsedSize Fail"
        );
        fail_if!(
            !self.check_used_size(&target).await?,
            "DeleteAllObject({t}) CheckUsedSize Fail"
        );

        if versions {
            info!(
                "Versions Test End. Elapsed Time: {} sec",
                fixed(self.watcher.now(), 3)
            );
        } else {
            // 버킷 삭제
            self.client.delete_bucket(&s).await?;
            info!(
                "Test End. Elapsed Time: {} sec",
                fixed(self.watcher.now(), 3)
            );
        }
        Ok(0)
    }

    /// 원본 `DBConnect()`: DB 연결이 없으면 새로 연결한다. 실패하면 `e.Message`만 로그로 남기고 `false`.
    /// 원본은 `DB = new MySqlConnection(...)`을 `Open()` 앞에서 대입하므로, 연결에 실패해도 다음 호출은 `true`다.
    async fn db_connect(&mut self) -> bool {
        if !matches!(self.db, Db::None) {
            return true;
        }
        match self.open_db().await {
            Ok(conn) => {
                self.db = Db::Open(conn);
                true
            }
            Err(message) => {
                error!("{message}");
                self.db = Db::Unopened;
                false
            }
        }
    }

    /// `new MySqlConnection(connectionString).Open()`.
    async fn open_db(&self) -> Result<Conn, String> {
        let config = &self.db_config;
        let port = u16::try_from(config.port).map_err(|_| UNABLE_TO_CONNECT.to_string())?;
        let opts = OptsBuilder::default()
            .ip_or_hostname(config.host.clone())
            .tcp_port(port)
            .user(Some(config.user.clone()))
            .pass(Some(config.password.clone()))
            .db_name(Some(config.database.clone()));
        // MySql.Data의 기본 연결 제한 시간(15초)
        match tokio::time::timeout(Duration::from_secs(15), Conn::new(opts)).await {
            Ok(Ok(conn)) => Ok(conn),
            Ok(Err(mysql_async::Error::Server(server))) => Err(server.message),
            Ok(Err(_)) | Err(_) => Err(UNABLE_TO_CONNECT.to_string()),
        }
    }

    /// DB에 기록된 버킷의 파일 개수와 사용량이 기대값과 일치하는지 확인한다(원본 `CheckUsedSize`).
    async fn check_used_size(&mut self, usage: &UsageData) -> Result<bool, ScenarioError> {
        if !self.db_connect().await {
            return Ok(false);
        }
        let sql = format!(
            "SELECT filecount, used FROM bucket WHERE bucket = '{}'",
            usage.name
        );
        // 열리지 않은 연결로 `ExecuteReader()`를 부르면 `InvalidOperationException`.
        let Db::Open(conn) = &mut self.db else {
            return Err(ScenarioError::new(
                "System.InvalidOperationException",
                "Connection must be valid and open.",
            ));
        };
        let row: Option<mysql_async::Row> = conn.query_first(sql).await.map_err(db_error)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let result_file_count: i32 = column(&row, 0)?;
        let result_used_size: i64 = column(&row, 1)?;
        if result_file_count == usage.file_count && result_used_size == usage.used_size {
            return Ok(true);
        }
        error!(
            "FileCount: {result_file_count} != {}, UsedSize: {result_used_size} != {}",
            usage.file_count, usage.used_size
        );
        Ok(false)
    }

    /// 버킷이 존재하면 오브젝트를 모두 삭제한 뒤 버킷 자체를 삭제한다.
    async fn clean_bucket(&self, bucket: &str) -> Result<(), ScenarioError> {
        if self.client.does_s3_bucket_exist(bucket).await {
            self.clean_object(bucket).await?;
            self.delete_bucket(bucket).await?;
        }
        Ok(())
    }

    /// 설정된 `FileSize` 크기의 랜덤 텍스트로 오브젝트를 업로드한다.
    async fn put_object(&self, bucket: &str, key: &str) -> Result<bool, ScenarioError> {
        let body = random_text_long(self.config.file_size as usize);
        match self
            .client
            .put_object(bucket, key, PutBody::Text(body), true, None)
            .await
        {
            Ok(response) if response.status == 200 => Ok(true),
            Ok(response) => {
                error!(
                    "PutObject({bucket}, {key}) Fail({})",
                    status_name(response.status)
                );
                Ok(false)
            }
            Err(e) => caught(e),
        }
    }

    /// 오브젝트를 다른 버킷 또는 키로 복사한다.
    async fn copy_object(
        &self,
        source_bucket: &str,
        source_key: &str,
        target_bucket: &str,
        target_key: &str,
    ) -> Result<bool, ScenarioError> {
        match self
            .client
            .copy_object(source_bucket, source_key, target_bucket, target_key, None)
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => caught(e),
        }
    }

    /// 설정된 `MultiPartSize` 크기를 5MiB 단위 파츠로 나누어 멀티파트 업로드한다.
    async fn multipart_upload(&self, bucket: &str, key: &str) -> Result<bool, ScenarioError> {
        let result: Result<(), S3Error> = async {
            let init = self.client.initiate_multipart_upload(bucket, key).await?;
            let upload_id = init.output.upload_id().unwrap_or_default().to_string();
            let mut parts = Vec::new();
            let mut remaining_size = self.config.multi_part_size;
            let mut part_size = 5 * MIB;
            let mut part_number = 1;
            while remaining_size > 1 {
                // 마지막 파트는 남은 사이즈로 설정
                if remaining_size < 5 * MIB {
                    part_size = remaining_size;
                    remaining_size = 0;
                } else {
                    remaining_size -= part_size;
                }
                let data = random_text_long(part_size as usize).into_bytes();
                let response = self
                    .client
                    .upload_part(
                        bucket,
                        key,
                        &upload_id,
                        part_number,
                        PutBody::Bytes(data),
                        0,
                        -1,
                        true,
                    )
                    .await?;
                parts.push(PartETag::new(part_number, response.output.e_tag()));
                part_number += 1;
            }
            self.client
                .complete_multipart_upload(bucket, key, &upload_id, &parts)
                .await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(true),
            Err(e) => caught(e),
        }
    }

    /// 오브젝트를 5MiB 단위 파츠로 나누어 멀티파트 복사(CopyPart)한다.
    async fn multipart_copy(
        &self,
        source_bucket: &str,
        source_key: &str,
        target_bucket: &str,
        target_key: &str,
    ) -> Result<bool, ScenarioError> {
        let result: Result<(), S3Error> = async {
            let init = self
                .client
                .initiate_multipart_upload(target_bucket, target_key)
                .await?;
            let upload_id = init.output.upload_id().unwrap_or_default().to_string();
            let mut parts = Vec::new();
            let part_size = 5 * MIB;
            let mut start = 0i64;
            let mut end = part_size;
            let size = self.config.multi_part_size;
            let mut part_number = 1;
            while start < size {
                let response = self
                    .client
                    .copy_part(
                        source_bucket,
                        source_key,
                        target_bucket,
                        target_key,
                        &upload_id,
                        part_number,
                        start,
                        end,
                        None,
                    )
                    .await?;
                let e_tag = response.output.copy_part_result().and_then(|r| r.e_tag());
                parts.push(PartETag::new(part_number, e_tag));

                part_number += 1;
                start = end + 1;
                end = (start + part_size - 1).min(size - 1);
                if end > self.config.multi_part_size {
                    end = self.config.multi_part_size;
                }
            }
            self.client
                .complete_multipart_upload(target_bucket, target_key, &upload_id, &parts)
                .await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(true),
            Err(e) => caught(e),
        }
    }

    /// 단일 오브젝트를 삭제한다(`NoContent`여야 성공).
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<bool, ScenarioError> {
        match self.client.delete_object(bucket, key, None, None).await {
            Ok(response) if response.status == 204 => Ok(true),
            Ok(response) => {
                error!(
                    "DeleteObject({bucket}, {key}) Fail({})",
                    status_name(response.status)
                );
                Ok(false)
            }
            Err(e) => caught(e).map(|_| false),
        }
    }

    /// 버킷의 모든 버전(오브젝트)을 조회하여 일괄 삭제한다(첫 페이지만).
    async fn clean_object(&self, bucket: &str) -> Result<bool, ScenarioError> {
        let response = match self
            .client
            .list_versions(bucket, None, None, None, 1000, None)
            .await
        {
            Ok(response) => response,
            Err(e) => return caught(e).map(|_| false),
        };
        // `response.Versions`가 `null`이면 `Select`에서 ArgumentNullException
        let Some(versions) = response.output.entries() else {
            return Err(ScenarioError::new(
                "System.ArgumentNullException",
                "Value cannot be null. (Parameter 'source')",
            ));
        };
        let keys: Vec<(String, Option<String>)> = versions
            .iter()
            .map(|v| {
                (
                    v.key().unwrap_or_default().to_string(),
                    v.version_id().map(str::to_string),
                )
            })
            .collect();
        match self.client.delete_objects(bucket, &keys, None, None).await {
            Ok(response) if response.status == 200 => Ok(true),
            Ok(response) => {
                error!(
                    "DeleteObjects({bucket}) Fail({})",
                    status_name(response.status)
                );
                Ok(false)
            }
            Err(e) => caught(e).map(|_| false),
        }
    }

    /// 버킷을 생성한다.
    async fn create_bucket(&self, bucket: &str) -> Result<(), ScenarioError> {
        match self.client.put_bucket(bucket, None, None, None).await {
            Ok(response) => {
                if response.status != 200 {
                    error!(
                        "CreateBucket({bucket}) Fail({})",
                        status_name(response.status)
                    );
                }
                Ok(())
            }
            Err(e) => caught(e).map(|_| ()),
        }
    }

    /// 버킷을 삭제한다(`NoContent`여야 성공).
    async fn delete_bucket(&self, bucket: &str) -> Result<(), ScenarioError> {
        match self.client.delete_bucket(bucket).await {
            Ok(response) => {
                if response.status != 204 {
                    error!(
                        "DeleteBucket({bucket}) Fail({})",
                        status_name(response.status)
                    );
                }
                Ok(())
            }
            Err(e) => caught(e).map(|_| ()),
        }
    }
}

/// `reader.GetInt32/GetInt64(index)`: 숫자 열을 읽는다. 읽을 수 없으면 `InvalidCastException`.
fn column<T: mysql_async::prelude::FromValue>(
    row: &mysql_async::Row,
    index: usize,
) -> Result<T, ScenarioError> {
    row.get_opt::<T, _>(index)
        .and_then(Result::ok)
        .ok_or_else(|| {
            ScenarioError::new(
                "System.InvalidCastException",
                "Specified cast is not valid.",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_data_counts() {
        let mut usage = UsageData::new("b");
        usage.add_usage(10);
        usage.add_usage(5);
        usage.sub_usage(10);
        assert_eq!((usage.file_count, usage.used_size), (1, 5));
        usage.set_usage(0, 0);
        assert_eq!((usage.file_count, usage.used_size), (0, 0));
    }
}
