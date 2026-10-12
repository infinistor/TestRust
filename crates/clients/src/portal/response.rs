//! TESTCore `Portal/Response/*` 이식. 속성 이름은 원본 그대로 PascalCase이며 대소문자를 구분한다.

use std::ops::Deref;

use serde::Serialize;
use testrust_common::DotnetDateTime;

use super::data::{
    EnumResponseResult, EnumVolumePermission, EnumVolumeReplicationType, EnumVolumeSecurityLevel,
    EnumVolumeStatus,
};
use crate::json::{Decimal, Deserializer, FromJson, JsonError, Token};

/// Portal 기본 응답
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortalResponse {
    pub is_need_login: bool,
    pub access_denied: bool,
    pub result: EnumResponseResult,
    pub code: Option<String>,
    pub message: Option<String>,
}

impl PortalResponse {
    /// 기본 응답의 속성을 읽는다. 해당 속성이 아니면 `false`.
    fn set_property(
        d: &mut Deserializer<'_>,
        object: &mut Self,
        name: &str,
    ) -> Result<bool, JsonError> {
        match name {
            "IsNeedLogin" => object.is_need_login = d.read_value()?,
            "AccessDenied" => object.access_denied = d.read_value()?,
            "Result" => object.result = d.read_value()?,
            "Code" => object.code = d.read_nullable()?,
            "Message" => object.message = d.read_nullable()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl FromJson for PortalResponse {
    fn type_name() -> String {
        "TestCore.Portal.Response.PortalResponse".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), Self::set_property)
    }
}

/// 데이터가 붙은 Portal 응답(`PortalResponseData<T> : PortalResponse`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalResponseData<T> {
    pub base: PortalResponse,
    pub data: Option<T>,
}

impl<T> Deref for PortalResponseData<T> {
    type Target = PortalResponse;

    fn deref(&self) -> &PortalResponse {
        &self.base
    }
}

impl<T: FromJson> FromJson for PortalResponseData<T> {
    fn type_name() -> String {
        format!(
            "TestCore.Portal.Response.PortalResponseData`1[{}]",
            T::type_name()
        )
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        let init = Self {
            base: PortalResponse::default(),
            data: None,
        };
        d.read_object(tok, &Self::type_name(), init, |d, o, name| {
            if name == "Data" {
                o.data = d.read_nullable()?;
                return Ok(true);
            }
            PortalResponse::set_property(d, &mut o.base, name)
        })
    }
}

/// 사용자 정보 응답
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResponseSystemUser {
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub display_user_name: Option<String>,
    pub notification: bool,
    pub noti_sect: Option<String>,
    pub user_sect: Option<String>,
    pub role: Option<String>,
    pub email: Option<String>,
    pub department: Option<String>,
    pub remark: Option<String>,
    pub reg_date: DotnetDateTime,
    pub valid_user: bool,
    pub read_only: bool,
}

impl FromJson for ResponseSystemUser {
    fn type_name() -> String {
        "TestCore.Portal.Response.ResponseSystemUser".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "UserId" => o.user_id = d.read_nullable()?,
                "UserName" => o.user_name = d.read_nullable()?,
                "DisplayUserName" => o.display_user_name = d.read_nullable()?,
                "Notification" => o.notification = d.read_value()?,
                "NotiSect" => o.noti_sect = d.read_nullable()?,
                "UserSect" => o.user_sect = d.read_nullable()?,
                "Role" => o.role = d.read_nullable()?,
                "Email" => o.email = d.read_nullable()?,
                "Department" => o.department = d.read_nullable()?,
                "Remark" => o.remark = d.read_nullable()?,
                "RegDate" => o.reg_date = d.read_value()?,
                "ValidUser" => o.valid_user = d.read_value()?,
                "ReadOnly" => o.read_only = d.read_value()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// 볼륨 정보 응답
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResponseVolume {
    pub id: i64,
    pub name: Option<String>,
    pub status: EnumVolumeStatus,
    pub permission: EnumVolumePermission,
    pub quota_size: Decimal,
    pub used_size: Decimal,
    pub free_size: Decimal,
    pub security_level: EnumVolumeSecurityLevel,
    pub remark: Option<String>,
    pub usage: f32,
    pub file_count: i64,
    pub replication_type: EnumVolumeReplicationType,
}

impl FromJson for ResponseVolume {
    fn type_name() -> String {
        "TestCore.Portal.Response.ResponseVolume".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Id" => o.id = d.read_value()?,
                "Name" => o.name = d.read_nullable()?,
                "Status" => o.status = d.read_value()?,
                "Permission" => o.permission = d.read_value()?,
                "QuotaSize" => o.quota_size = d.read_value()?,
                "UsedSize" => o.used_size = d.read_value()?,
                "FreeSize" => o.free_size = d.read_value()?,
                "SecurityLevel" => o.security_level = d.read_value()?,
                "Remark" => o.remark = d.read_nullable()?,
                "Usage" => o.usage = d.read_value()?,
                "FileCount" => o.file_count = d.read_value()?,
                "ReplicationType" => o.replication_type = d.read_value()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}
