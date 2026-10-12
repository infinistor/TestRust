//! TESTCore `Portal/Request/*` 이식. `ToJsonString()`과 같은 JSON을 만든다
//! (`testrust_common::to_dotnet_json`, 속성은 선언 순서, 읽기 전용 속성 포함).

use serde::ser::{Serialize, SerializeStruct, Serializer};

use super::data::{
    EnumDiskSizeUnit, EnumVolumePermission, EnumVolumeReplicationType, EnumVolumeSecurityLevel,
    to_disk_size_unit,
};

/// 전체 버킷을 뜻하는 값
const DEFAULT_ALL_BUCKET: &str = "ALL";

/// 접근 허용 IP 등록 요청. JSON 속성: `UserId`, `TenantId`, `IpAddress`, `BucketName`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RequestAddAccessIp {
    pub user_id: String,
    pub tenant_id: String,
    pub ip_address: String,
    pub bucket_name: String,
}

impl RequestAddAccessIp {
    /// 전체 버킷을 대상으로 하는 요청(원본의 3개 인자 생성자).
    pub fn new(user_id: &str, tenant_id: &str, ip_address: &str) -> Self {
        Self {
            user_id: user_id.into(),
            tenant_id: tenant_id.into(),
            ip_address: ip_address.into(),
            bucket_name: DEFAULT_ALL_BUCKET.into(),
        }
    }

    /// 지정한 버킷을 대상으로 하는 요청(원본의 4개 인자 생성자). 비어 있거나 공백이면 전체 버킷이다.
    pub fn with_bucket(
        user_id: &str,
        tenant_id: &str,
        ip_address: &str,
        bucket_name: Option<&str>,
    ) -> Self {
        let bucket_name = match bucket_name {
            Some(name) if !name.trim().is_empty() => name,
            _ => DEFAULT_ALL_BUCKET,
        };
        Self {
            user_id: user_id.into(),
            tenant_id: tenant_id.into(),
            ip_address: ip_address.into(),
            bucket_name: bucket_name.into(),
        }
    }
}

/// 사용자 생성 요청. `UserName`은 `UserId`, `ConfirmPassword`는 `Password`를 그대로 쓴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAddUser {
    pub user_id: String,
    pub password: String,
    pub volume_name: String,
    pub quota_size: u64,
    pub disk_size_unit: EnumDiskSizeUnit,
}

impl Default for RequestAddUser {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            password: String::new(),
            volume_name: String::new(),
            quota_size: 0,
            disk_size_unit: EnumDiskSizeUnit::GB,
        }
    }
}

impl Serialize for RequestAddUser {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("RequestAddUser", 7)?;
        s.serialize_field("UserId", &self.user_id)?;
        s.serialize_field("UserName", &self.user_id)?;
        s.serialize_field("Password", &self.password)?;
        s.serialize_field("ConfirmPassword", &self.password)?;
        s.serialize_field("VolumeName", &self.volume_name)?;
        s.serialize_field("QuotaSize", &self.quota_size)?;
        s.serialize_field("DiskSizeUnit", &self.disk_size_unit)?;
        s.end()
    }
}

/// 볼륨 생성 요청. 기본값은 원본과 같다(복제 1+1, 비공개, 비밀번호 `qwe123`, 보안 레벨 낮음).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAddVolume {
    pub name: String,
    pub replication_type: EnumVolumeReplicationType,
    pub quota_size: u64,
    pub permission: EnumVolumePermission,
    pub password: String,
    pub security_level: EnumVolumeSecurityLevel,
}

impl Default for RequestAddVolume {
    fn default() -> Self {
        Self {
            name: String::new(),
            replication_type: EnumVolumeReplicationType::OnePlusOne,
            quota_size: 0,
            permission: EnumVolumePermission::Private,
            password: "qwe123".into(),
            security_level: EnumVolumeSecurityLevel::Low,
        }
    }
}

impl Serialize for RequestAddVolume {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("RequestAddVolume", 7)?;
        s.serialize_field("Name", &self.name)?;
        s.serialize_field("ReplicationType", &self.replication_type)?;
        s.serialize_field("QuotaSize", &self.quota_size)?;
        s.serialize_field("Permission", &self.permission)?;
        s.serialize_field("Password", &self.password)?;
        s.serialize_field("ConfirmPassword", &self.password)?;
        s.serialize_field("SecurityLevel", &self.security_level)?;
        s.end()
    }
}

/// 시스템 사용자 생성 요청(원본에서 호출하는 곳은 없다).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RequestSystemAddUser {
    pub user_name: String,
    pub password: String,
    pub confirm_password: String,
    pub department: String,
    pub email: String,
    pub remark: String,
    pub notification: bool,
    pub volume_name: String,
    pub quota_size: i64,
    pub disk_size_unit: EnumDiskSizeUnit,
}

impl Default for RequestSystemAddUser {
    fn default() -> Self {
        Self {
            user_name: String::new(),
            password: String::new(),
            confirm_password: String::new(),
            department: String::new(),
            email: String::new(),
            remark: String::new(),
            notification: false,
            volume_name: String::new(),
            quota_size: 0,
            disk_size_unit: EnumDiskSizeUnit::GB,
        }
    }
}

/// 할당량 변경 요청. Byte 단위 할당량을 적절한 단위로 바꿔 담는다.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RequestUpdateQuotaSize {
    pub quota_size: u64,
    pub disk_size_unit: EnumDiskSizeUnit,
}

impl RequestUpdateQuotaSize {
    pub fn new(quota_bytes: u64) -> Self {
        let (quota_size, disk_size_unit) = to_disk_size_unit(quota_bytes);
        Self {
            quota_size,
            disk_size_unit,
        }
    }
}
