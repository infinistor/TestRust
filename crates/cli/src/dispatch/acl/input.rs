//! 입력 파일 DTO: `Data/S3/MyAccessControlList.cs`, `Data/S3/MyGrant.cs`와 SDK의 `Owner`, `S3Grantee`.
//!
//! `JsonSerializer.Deserialize<MyAccessControlList>(text)`(기본 옵션: 대소문자 구분, 모르는 속성은 건너뜀)처럼 읽는다.
//! `S3Grantee.Type`은 읽지 않는다(예제 JSON에 있지만 값은 무시되고 아래 규칙으로 정해진다).

use aws_sdk_s3::types::{AccessControlPolicy, Grant, Grantee, Owner, Permission, Type};
use testrust_common::json::{Deserializer, FromJson, JsonError, Token};

use crate::dispatch::CommandError;
use crate::dispatch::bucket::format::null_reference;

/// `Amazon.S3.Model.Owner`
#[derive(Debug, Default)]
struct OwnerInput {
    display_name: Option<String>,
    id: Option<String>,
}

impl FromJson for OwnerInput {
    fn type_name() -> String {
        "Amazon.S3.Model.Owner".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "DisplayName" => o.display_name = d.read_nullable()?,
                "Id" => o.id = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// `Amazon.S3.Model.S3Grantee`
#[derive(Debug, Default)]
struct GranteeInput {
    canonical_user: Option<String>,
    display_name: Option<String>,
    email_address: Option<String>,
    uri: Option<String>,
}

impl FromJson for GranteeInput {
    fn type_name() -> String {
        "Amazon.S3.Model.S3Grantee".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "CanonicalUser" => o.canonical_user = d.read_nullable()?,
                "DisplayName" => o.display_name = d.read_nullable()?,
                "EmailAddress" => o.email_address = d.read_nullable()?,
                "URI" => o.uri = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl GranteeInput {
    /// SDK의 `S3Grantee.Type`: 이메일 > URI(그룹) > 표준 사용자 순으로 정해지고, 하나도 없으면 `null`이다.
    fn grantee_type(&self) -> Option<&'static str> {
        if self.email_address.is_some() {
            Some("AmazonCustomerByEmail")
        } else if self.uri.is_some() {
            Some("Group")
        } else if self.canonical_user.is_some() {
            Some("CanonicalUser")
        } else {
            None
        }
    }

    fn to_sdk(&self) -> Result<Grantee, CommandError> {
        // Rust SDK의 `Grantee`는 `Type`이 필수라 `Type`이 없는 입력은 표식(`UNSET`)으로 만들고, 서명 전에
        // `xsi:type` 속성을 지운다(.NET은 `xsi:type` 없이 보낸다).
        Grantee::builder()
            .set_id(self.canonical_user.clone())
            .set_display_name(self.display_name.clone())
            .set_email_address(self.email_address.clone())
            .set_uri(self.uri.clone())
            .r#type(Type::from(
                self.grantee_type().unwrap_or(testrust_s3::UNSET),
            ))
            .build()
            .map_err(|e| CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string()))
    }
}

/// `TestCore.Data.S3.MyGrant`
#[derive(Debug, Default)]
struct MyGrant {
    permission: Option<String>,
    grantee: Option<GranteeInput>,
}

impl FromJson for MyGrant {
    fn type_name() -> String {
        "TestCore.Data.S3.MyGrant".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Permission" => o.permission = d.read_nullable()?,
                "Grantee" => o.grantee = d.read_nullable()?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// `TestCore.Data.S3.MyAccessControlList`
#[derive(Debug, Default)]
pub(super) struct MyAccessControlList {
    owner: Option<OwnerInput>,
    grants: Option<Vec<Option<MyGrant>>>,
}

impl FromJson for MyAccessControlList {
    fn type_name() -> String {
        "TestCore.Data.S3.MyAccessControlList".into()
    }

    fn from_json(d: &mut Deserializer<'_>, tok: Token) -> Result<Option<Self>, JsonError> {
        d.read_object(tok, &Self::type_name(), Self::default(), |d, o, name| {
            match name {
                "Owner" => o.owner = d.read_nullable()?,
                "Grants" => {
                    let tok = d.read_token()?;
                    if !matches!(tok, Token::Null | Token::StartArray) {
                        // 속성 형식이 `IList<MyGrant>`라 오류 메시지의 형식 이름이 `IList`1`이다.
                        return Err(d.conversion_error(
                            "System.Collections.Generic.IList`1[TestCore.Data.S3.MyGrant]",
                        ));
                    }
                    o.grants = d.read_list(tok)?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

impl MyAccessControlList {
    /// 원본 `GetS3AccessControlList()`. 요소가 `null`이면 `NullReferenceException`이다.
    pub(super) fn to_policy(&self) -> Result<AccessControlPolicy, CommandError> {
        let owner = self.owner.as_ref().map(|o| {
            Owner::builder()
                .set_display_name(o.display_name.clone())
                .set_id(o.id.clone())
                .build()
        });
        let mut grants = Vec::new();
        for grant in self.grants.iter().flatten() {
            let grant = grant.as_ref().ok_or_else(null_reference)?;
            grants.push(
                Grant::builder()
                    .set_grantee(
                        grant
                            .grantee
                            .as_ref()
                            .map(GranteeInput::to_sdk)
                            .transpose()?,
                    )
                    .set_permission(grant.permission.as_deref().map(Permission::from))
                    .build(),
            );
        }
        Ok(AccessControlPolicy::builder()
            .set_owner(owner)
            .set_grants((!grants.is_empty()).then_some(grants))
            .build())
    }

    /// `Permission`이 `null`인 `Grant`가 있는지. .NET은 요청을 만들 때 `ArgumentNullException`을 던진다.
    pub(super) fn has_null_permission(&self) -> bool {
        self.grants
            .iter()
            .flatten()
            .flatten()
            .any(|g| g.permission.is_none())
    }
}
