//! TESTCore `Portal/Data/*` 이식: 열거형, 디스크 크기 단위 변환, S3 자격 증명.

use serde::Serialize;

use crate::json::{Deserializer, FromJson, JsonError, Token};
use testrust_common::dotnet_enum;

dotnet_enum!(
    /// 디스크 크기 단위
    EnumDiskSizeUnit,
    "TestCore.Portal.Data.EnumDiskSizeUnit",
    { TB = 0, GB = 1, MB = 2 }
);

dotnet_enum!(
    /// 응답 결과
    EnumResponseResult,
    "TestCore.Portal.Data.EnumResponseResult",
    { Error = -1, Warning = 0, Success = 1 }
);

dotnet_enum!(
    /// 볼륨 권한
    EnumVolumePermission,
    "TestCore.Portal.Data.EnumVolumePermission",
    { Public = 0, Private = 1 }
);

dotnet_enum!(
    /// 볼륨 복제 타입(원본의 `OnePlusZeo` 철자 그대로)
    EnumVolumeReplicationType,
    "TestCore.Portal.Data.EnumVolumeReplicationType",
    { OnePlusZeo = 1, OnePlusOne = 2, OnePlusTwo = 3 }
);

dotnet_enum!(
    /// 볼륨 보안 레벨
    EnumVolumeSecurityLevel,
    "TestCore.Portal.Data.EnumVolumeSecurityLevel",
    { Low = 0, Middle = 1, High = 2 }
);

dotnet_enum!(
    /// 볼륨 상태
    EnumVolumeStatus,
    "TestCore.Portal.Data.EnumVolumeStatus",
    { Processing = 0, Online = 256, Offline = 257 }
);

const SIZE_TOKEN: u64 = 1000;
const MB: u64 = SIZE_TOKEN * SIZE_TOKEN;
const GB: u64 = SIZE_TOKEN * MB;
const TB: u64 = SIZE_TOKEN * GB;

impl EnumDiskSizeUnit {
    /// 원본 `ToByte(unit, size)`: 디스크 크기 단위를 Byte로 변환한다(10진 단위, 오버플로는 감싼다).
    pub fn to_byte(self, size: u64) -> u64 {
        match self {
            Self::TB => size.wrapping_mul(TB),
            Self::GB => size.wrapping_mul(GB),
            Self::MB => size.wrapping_mul(MB),
            _ => size,
        }
    }
}

/// 원본 `ToDiskSizeUnit(this ulong size)`: Byte를 적절한 단위의 (크기, 단위)로 바꾼다(내림).
pub fn to_disk_size_unit(size: u64) -> (u64, EnumDiskSizeUnit) {
    if size >= TB {
        (size / TB, EnumDiskSizeUnit::TB)
    } else if size >= GB {
        (size / GB, EnumDiskSizeUnit::GB)
    } else if size >= MB {
        (size / MB, EnumDiskSizeUnit::MB)
    } else {
        (size, EnumDiskSizeUnit::MB)
    }
}

/// S3 자격 증명. 시크릿 키의 JSON 속성 이름은 `AccessSecret`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct S3CredentialData {
    #[serde(rename = "AccessKey")]
    pub access_key: Option<String>,
    #[serde(rename = "AccessSecret")]
    pub secret_key: Option<String>,
}

impl FromJson for S3CredentialData {
    fn type_name() -> String {
        "TestCore.Portal.Data.S3CredentialData".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "AccessKey" => o.access_key = d.read_nullable()?,
                "AccessSecret" => o.secret_key = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_size_units() {
        assert_eq!(
            to_disk_size_unit(1_500_000_000_000),
            (1, EnumDiskSizeUnit::TB)
        );
        assert_eq!(to_disk_size_unit(3_000_000_000), (3, EnumDiskSizeUnit::GB));
        assert_eq!(to_disk_size_unit(2_000_000), (2, EnumDiskSizeUnit::MB));
        assert_eq!(to_disk_size_unit(999_999), (999_999, EnumDiskSizeUnit::MB));
        assert_eq!(to_disk_size_unit(0), (0, EnumDiskSizeUnit::MB));
        assert_eq!(EnumDiskSizeUnit::GB.to_byte(3), 3_000_000_000);
        assert_eq!(EnumDiskSizeUnit(9).to_byte(7), 7);
    }

    #[test]
    fn enum_names() {
        assert_eq!(EnumResponseResult(2).to_string(), "2");
        assert_eq!(EnumResponseResult::Warning.to_string(), "Warning");
        assert_eq!(EnumResponseResult(-1).to_string(), "Error");
    }
}
