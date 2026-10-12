//! TESTCore `Portal/PortalManager.cs` 이식: Portal API(볼륨·사용자·접근 허용 IP) 호출.
//!
//! 원본과 같게 맞춘 동작:
//!
//! - 주소는 `{URL}/{상수}`처럼 이어 붙이며 상수가 `/`로 시작하므로 경로에 `//`가 생긴다
//!   (`{URL}//api/v1/Health`, `SystemUsers//{id}`). 그대로 요청한다.
//! - `PutAccessIp`는 `RequestAddAccessIp(userId, tenantId, ...)`에 `(volumeName, userId, ...)`를 넘긴다.
//!   그래서 JSON의 `UserId`에 볼륨 이름이, `TenantId`에 사용자 ID가 들어간다. 반대로 `DeleteAccessIp`의
//!   쿼리는 `UserId={userId}&TenantId={volumeName}`이다.
//! - 응답 `Result`가 `Success`가 아니면 `Portal*Exception`이며 메시지는 `ERROR({Code}) : {Message}`.
//! - 응답 본문이 JSON `null`이면 `result.Result`에서 `NullReferenceException`이 난다.
//! - `HealthCheck` 실패는 `HealthCheck Error : {Result}`를 오류 로그로 남기고 `false`를 돌려준다.
//!
//! 다른 점: `GetUserCredential`은 `AccessKey`·`SecretKey`가 `null`이어도 빈 문자열로 담는다
//! (`UserData`가 `String`이라 `null`을 표현하지 못한다).

pub mod data;
pub mod error;
pub mod request;
pub mod response;

use testrust_config::{PortalConfig, UserData};

pub use data::{
    EnumDiskSizeUnit, EnumResponseResult, EnumVolumePermission, EnumVolumeReplicationType,
    EnumVolumeSecurityLevel, EnumVolumeStatus, S3CredentialData, to_disk_size_unit,
};
pub use error::PortalError;
use request::{RequestAddAccessIp, RequestAddUser, RequestAddVolume, RequestUpdateQuotaSize};
pub use response::{PortalResponse, PortalResponseData, ResponseSystemUser, ResponseVolume};

use crate::json::FromJson;
use crate::khttp::{KHttpClient, KHttpError};
use testrust_common::to_dotnet_json;

const GET_HEALTH_CHECK: &str = "/api/v1/Health";
const DEFAULT_VOLUME_URL: &str = "/api/v1/Volumes";
const DEFAULT_SYSTEM_USER_URL: &str = "/api/v1/SystemUsers";
const DEFAULT_ACCESS_IPS_URL: &str = "/api/v1/AccessIps";
const GET_USER_URL: &str = "/api/v1/SystemUsers/";

/// 원본 `PortalManager`.
pub struct PortalManager {
    client: KHttpClient,
    pub config: PortalConfig,
}

/// 응답이 JSON `null`이면 `NullReferenceException`.
fn non_null<T>(value: Option<T>) -> Result<T, PortalError> {
    value.ok_or(PortalError::NullReference)
}

impl PortalManager {
    /// API 키가 HTTP 토큰이 아니면 `FormatException`(원본 생성자의 `new KHttpClient(config.ApiKey)`).
    pub fn new(config: PortalConfig) -> Result<Self, KHttpError> {
        Ok(Self {
            client: KHttpClient::new(Some(&config.api_key))?,
            config,
        })
    }

    async fn get<T: FromJson>(&self, path: &str) -> Result<T, PortalError> {
        non_null(
            self.client
                .get::<T>(&format!("{}/{path}", self.config.url))
                .await?,
        )
    }

    async fn post<T: FromJson>(&self, path: &str, body: &str) -> Result<T, PortalError> {
        non_null(
            self.client
                .post::<T>(&format!("{}/{path}", self.config.url), body)
                .await?,
        )
    }

    async fn delete<T: FromJson>(&self, url: &str) -> Result<T, PortalError> {
        non_null(self.client.delete::<T>(url).await?)
    }

    /// PortalManager 접속 확인
    pub async fn health_check(&self) -> Result<bool, PortalError> {
        let result: PortalResponse = self.get(GET_HEALTH_CHECK).await?;
        if result.result == EnumResponseResult::Success {
            return Ok(true);
        }
        tracing::error!("HealthCheck Error : {}", result.result);
        Ok(false)
    }

    // ---- 볼륨 ----

    /// 볼륨 조회. 성공이 아니면 `None`.
    pub async fn get_volume(
        &self,
        volume_name: &str,
    ) -> Result<Option<ResponseVolume>, PortalError> {
        let result: PortalResponseData<ResponseVolume> = self
            .get(&format!("{DEFAULT_VOLUME_URL}/{volume_name}"))
            .await?;
        if result.result == EnumResponseResult::Success {
            Ok(result.data)
        } else {
            Ok(None)
        }
    }

    /// 볼륨 생성
    pub async fn create_volume(
        &self,
        volume_name: &str,
        size: u64,
        password: &str,
    ) -> Result<(), PortalError> {
        let volume = RequestAddVolume {
            name: volume_name.into(),
            quota_size: size,
            password: password.into(),
            ..RequestAddVolume::default()
        };
        let result: PortalResponse = self
            .post(DEFAULT_VOLUME_URL, &to_dotnet_json(&volume))
            .await?;
        expect_success(&result, PortalError::volume)
    }

    /// 볼륨 시작
    pub async fn start_volume(&self, volume_name: &str) -> Result<(), PortalError> {
        let result: PortalResponse = self
            .post(&format!("{DEFAULT_VOLUME_URL}/{volume_name}/Start"), "")
            .await?;
        expect_success(&result, PortalError::volume)
    }

    /// 볼륨 중지
    pub async fn stop_volume(&self, volume_name: &str) -> Result<(), PortalError> {
        let result: PortalResponse = self
            .post(&format!("{DEFAULT_VOLUME_URL}/{volume_name}/Stop"), "")
            .await?;
        expect_success(&result, PortalError::volume)
    }

    /// 볼륨 삭제
    pub async fn delete_volume(&self, volume_name: &str) -> Result<(), PortalError> {
        let url = format!("{}/{DEFAULT_VOLUME_URL}/{volume_name}", self.config.url);
        let result: PortalResponse = self.delete(&url).await?;
        expect_success(&result, PortalError::volume)
    }

    /// 볼륨 할당
    pub async fn assign_volume(
        &self,
        volume_name: &str,
        user_id: &str,
        size: u64,
    ) -> Result<(), PortalError> {
        let quota = RequestUpdateQuotaSize::new(size);
        let result: PortalResponse = self
            .post(
                &format!("{DEFAULT_VOLUME_URL}/{volume_name}/Users/{user_id}"),
                &to_dotnet_json(&quota),
            )
            .await?;
        expect_success(&result, PortalError::volume)
    }

    // ---- 사용자 ----

    /// 사용자 조회. 성공이 아니면 `None`.
    pub async fn get_user(&self, user_id: &str) -> Result<Option<ResponseSystemUser>, PortalError> {
        let result: PortalResponseData<ResponseSystemUser> = self
            .get(&format!("{DEFAULT_SYSTEM_USER_URL}/{user_id}"))
            .await?;
        if result.result == EnumResponseResult::Success {
            Ok(result.data)
        } else {
            Ok(None)
        }
    }

    /// 사용자 존재 여부(원본은 `GET_USER_URL` 뒤에 `/`를 또 붙여 `SystemUsers//{id}`를 호출한다)
    pub async fn is_user(&self, user_id: &str) -> Result<bool, PortalError> {
        let result: PortalResponseData<String> =
            self.get(&format!("{GET_USER_URL}/{user_id}")).await?;
        Ok(result.result == EnumResponseResult::Success)
    }

    /// 사용자 생성
    pub async fn create_user(
        &self,
        volume_name: &str,
        user_name: &str,
        size: u64,
        password: &str,
    ) -> Result<(), PortalError> {
        let user = RequestAddUser {
            volume_name: volume_name.into(),
            user_id: user_name.into(),
            quota_size: size,
            password: password.into(),
            ..RequestAddUser::default()
        };
        let result: PortalResponseData<String> = self
            .post(DEFAULT_SYSTEM_USER_URL, &to_dotnet_json(&user))
            .await?;
        expect_success(&result, PortalError::user)
    }

    /// 사용자 자격 증명 조회. 성공이 아니면 `None`.
    pub async fn get_user_credential(
        &self,
        volume_name: &str,
        user_id: &str,
    ) -> Result<Option<UserData>, PortalError> {
        let result: PortalResponseData<S3CredentialData> = self
            .get(&format!(
                "{DEFAULT_VOLUME_URL}/{volume_name}/Users/{user_id}"
            ))
            .await?;
        if result.result != EnumResponseResult::Success {
            return Ok(None);
        }
        // 성공이어도 Data가 없으면 `result.Data.AccessKey`에서 NullReferenceException
        let data = non_null(result.data.as_ref())?;
        Ok(Some(UserData::new(
            self.config.url.clone(),
            "",
            data.access_key.clone().unwrap_or_default(),
            data.secret_key.clone().unwrap_or_default(),
        )))
    }

    /// 사용자 삭제
    pub async fn delete_user(&self, user_id: &str) -> Result<(), PortalError> {
        let url = format!("{}/{DEFAULT_SYSTEM_USER_URL}/{user_id}", self.config.url);
        let result: PortalResponse = self.delete(&url).await?;
        expect_success(&result, PortalError::user)
    }

    // ---- 접근 허용 IP ----

    /// 사용자의 접근 허용 IP를 등록한다. `bucket_name`이 없거나 공백이면 전체 버킷이다.
    pub async fn put_access_ip(
        &self,
        volume_name: &str,
        user_id: &str,
        access_ip: &str,
        bucket_name: Option<&str>,
    ) -> Result<(), PortalError> {
        // 원본의 인자 순서 그대로(생성자는 userId, tenantId 순이지만 volumeName, userId를 넘긴다).
        let access_ips =
            RequestAddAccessIp::with_bucket(volume_name, user_id, access_ip, bucket_name);
        let result: PortalResponse = self
            .post(DEFAULT_ACCESS_IPS_URL, &to_dotnet_json(&access_ips))
            .await?;
        expect_success(&result, PortalError::access_ips)
    }

    /// 사용자의 접근 허용 IP를 삭제한다. `bucket_name`이 없거나 공백이면 전체 버킷이다.
    pub async fn delete_access_ip(
        &self,
        volume_name: &str,
        user_id: &str,
        bucket_name: Option<&str>,
    ) -> Result<(), PortalError> {
        let mut url = format!(
            "{}/{DEFAULT_ACCESS_IPS_URL}?UserId={user_id}&TenantId={volume_name}",
            self.config.url
        );
        if let Some(bucket) = bucket_name.filter(|b| !b.trim().is_empty()) {
            url.push_str(&format!("&BucketName={bucket}"));
        }
        let result: PortalResponse = self.delete(&url).await?;
        expect_success(&result, PortalError::access_ips)
    }
}

/// `Result`가 `Success`가 아니면 해당 예외를 만든다.
fn expect_success(
    response: &PortalResponse,
    exception: fn(&PortalResponse) -> PortalError,
) -> Result<(), PortalError> {
    if response.result == EnumResponseResult::Success {
        Ok(())
    } else {
        Err(exception(response))
    }
}
