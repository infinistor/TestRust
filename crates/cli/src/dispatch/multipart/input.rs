//! 입력 파일 DTO: `Data/MultiParts.cs`와 SDK의 `PartETag`.
//!
//! `JsonSerializer.Deserialize<MultiParts>(text)`(기본 옵션: 대소문자 구분, 모르는 속성은 건너뜀)처럼 읽는다.

use testrust_common::json::{Deserializer, FromJson, JsonError, Token};
use testrust_s3::s3_client::PartETag;

/// `System.Nullable<int>`의 변환 오류 이름.
const NULLABLE_INT: &str = "System.Nullable`1[System.Int32]";

/// `Amazon.S3.Model.PartETag`(읽은 값).
#[derive(Debug, Default)]
struct PartInput(PartETag);

impl FromJson for PartInput {
    fn type_name() -> String {
        "Amazon.S3.Model.PartETag".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            let part = &mut o.0;
            match name {
                "ETag" => part.e_tag = d.read_nullable()?,
                "PartNumber" => {
                    part.part_number = match d.read_token()? {
                        Token::Null => None,
                        Token::Number(text) => Some(
                            text.parse::<i32>()
                                .map_err(|_| d.conversion_error(NULLABLE_INT))?,
                        ),
                        _ => return Err(d.conversion_error(NULLABLE_INT)),
                    }
                }
                "ChecksumCRC32" => part.checksum_crc32 = d.read_nullable()?,
                "ChecksumCRC32C" => part.checksum_crc32_c = d.read_nullable()?,
                "ChecksumCRC64NVME" => part.checksum_crc64_nvme = d.read_nullable()?,
                "ChecksumSHA1" => part.checksum_sha1 = d.read_nullable()?,
                "ChecksumSHA256" => part.checksum_sha256 = d.read_nullable()?,
                // Rust SDK의 `CompletedPart`에 없는 체크섬: 읽기만 한다.
                "ChecksumMD5" | "ChecksumSHA512" | "ChecksumXXHASH128" | "ChecksumXXHASH3"
                | "ChecksumXXHASH64" => {
                    d.read_nullable::<String>()?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// `TestCore.Data.MultiParts`
#[derive(Debug, Default)]
pub(super) struct MultiParts {
    /// `null`이면 `None`. 목록 안의 `null` 요소는 이미 걸러져 있다.
    pub(super) parts: Option<Vec<PartETag>>,
}

impl FromJson for MultiParts {
    fn type_name() -> String {
        "TestCore.Data.MultiParts".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Parts" => {
                    let tok = d.read_token()?;
                    o.parts = d
                        .read_list::<PartInput>(tok)?
                        // `PartETags`의 `null` 요소는 요청 XML에서 빠진다.
                        .map(|list| list.into_iter().flatten().map(|p| p.0).collect());
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}
