//! 원본 `CommandDispatcher` 중 버킷·객체 ACL 조회·설정.
//!
//! 원본 동작 중 그대로 둔 것
//!
//! - `PutBucketAcl`은 `--acl`(미리 정해 둔 ACL)을 읽어 `PutACLRequest.CannedACL`에 넣지만 요청에는
//!   `request.AccessControlList`만 넘겨 `--acl`이 무시된다. `--file`도 `--acl`도 없으면 오류 로그만 남기고
//!   (또는 입력 파일이 없어 `파일을 찾을 수 없습니다.`를 출력한 뒤에도) 본문 없이 `PUT ?acl`을 보낸다.
//! - `PutObjectAcl`도 `--file`이 없을 때(파일이 없거나 `--file`·`--acl` 모두 없을 때)에도 요청을 보낸다.
//!   `--acl`만 주면 `x-amz-acl` 헤더만 보낸다. `--file`과 `--acl`을 함께 주면 `--file`만 쓴다.
//! - 입력 파일의 `Permission`이 없는 `Grant`가 있으면 .NET SDK가 요청을 만들다
//!   `ArgumentNullException (Parameter 'key')`을 던진다(`PutBucketAcl Start` 로그 뒤).
//! - `S3Grantee.Type`은 입력에서 읽지 않고 `EmailAddress` > `URI` > `CanonicalUser` 순으로 정해진다.
//! - `GetBucketAcl`·`GetObjectAcl`의 성공 로그는 `Get bucket ACL!`·`Get Object ACL!`로 대소문자가 다르다.
//!
//! Rust SDK의 `Grantee`는 `Type`이 필수라 `CanonicalUser`·`URI`·`EmailAddress`가 모두 없는 `Grantee`(예:
//! `DisplayName`만 있는 입력)는 표식(`UNSET`)으로 만들고, S3 클라이언트가 서명 전에 `xsi:type`을 지워 .NET처럼 생략한다.

mod input;

use super::input::blank;
use std::path::Path;
use std::time::Instant;

use aws_sdk_s3::types::ObjectCannedAcl;
use testrust_common::dotnet_http::status_name;
use testrust_common::json::{ReadOptions, deserialize};
use tracing::{error, info};

use super::bucket::{constant, format};
use super::output;
use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;
use input::MyAccessControlList;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::GetBucketAcl,
    MenuList::PutBucketAcl,
    MenuList::GetObjectAcl,
    MenuList::PutObjectAcl,
];

/// `PutBucketAcl` 도움말의 예제(`MyAccessControlList`).
const BUCKET_ACL_EXAMPLE: &str = r#"{
  "Owner": {
    "DisplayName": "MainDisplayName",
    "Id": "MainUserId"
  },
  "Grants": [
    {
      "Permission": "FULL_CONTROL",
      "Grantee": {
        "CanonicalUser": "AltUserId",
        "DisplayName": "AltDisplayName",
        "EmailAddress": null,
        "Type": {
          "Value": "CanonicalUser"
        },
        "URI": null
      }
    }
  ]
}"#;

/// `PutObjectAcl` 도움말의 예제(SDK `S3AccessControlList`).
const OBJECT_ACL_EXAMPLE: &str = r#"{
  "Grants": [
    {
      "Grantee": {
        "CanonicalUser": "AltUserId",
        "DisplayName": "AltDisplayName",
        "EmailAddress": null,
        "Type": {
          "Value": "CanonicalUser"
        },
        "URI": null
      },
      "Permission": {
        "HeaderName": "x-amz-grant-full-control",
        "Value": "FULL_CONTROL"
      }
    }
  ],
  "Owner": {
    "DisplayName": "MainDisplayName",
    "Id": "MainUserId"
  }
}"#;

/// 메뉴별 도움말.
fn help_text(menu: MenuList) -> Option<String> {
    use MenuList::*;
    Some(match menu {
        GetBucketAcl => [
            usage::main_flag(usage::GET_BUCKET_ACL, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        PutBucketAcl => [
            usage::main_flag(usage::PUT_BUCKET_ACL, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::optional_value(usage::ACL, "string", " : 권한 설정 정보"),
            usage::optional_value(usage::FILE, "string", " : 권한 설정 파일 경로"),
            "\nGrant\n".to_string(),
            BUCKET_ACL_EXAMPLE.to_string(),
        ]
        .concat(),
        GetObjectAcl => [
            usage::main_flag(usage::GET_OBJECT_ACL, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::optional_value(usage::VERSION_ID, "string", ""),
        ]
        .concat(),
        PutObjectAcl => [
            usage::main_flag(usage::PUT_OBJECT_ACL, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::optional_value(usage::ACL, "string", " : 권한 설정 정보"),
            usage::optional_value(usage::FILE, "string", " : 권한 설정 파일 경로"),
            "\nGrant\n".to_string(),
            OBJECT_ACL_EXAMPLE.to_string(),
        ]
        .concat(),
        _ => return Option::None,
    })
}

/// 객체 ACL 연산에서 .NET SDK가 전용 예외로 던지는 오류 코드.
fn object_model(error: testrust_s3::S3Error) -> CommandError {
    CommandError::s3(error, &["NoSuchKey"])
}

/// `--file`로 준 ACL 입력을 읽는다. 파일이 없으면 `파일을 찾을 수 없습니다.`를 출력하고 `None`.
fn read_policy(file_path: &str) -> Result<Option<MyAccessControlList>, CommandError> {
    if !Path::new(file_path).is_file() {
        println!("{}", usage::ERROR_FILE);
        return Ok(Option::None);
    }
    let text = format::read_all_text(file_path)?;
    let item = deserialize::<MyAccessControlList>(&text, ReadOptions::default())
        .map_err(format::json_error)?;
    // `item.GetS3AccessControlList()`: `null`이면 NullReferenceException.
    item.map(Some).ok_or_else(format::null_reference)
}

/// 요청을 만들 때 `Permission`이 없는 `Grant`가 있으면 .NET이 던지는 예외.
fn check_permissions(item: &Option<MyAccessControlList>) -> Result<(), CommandError> {
    if item
        .as_ref()
        .is_some_and(MyAccessControlList::has_null_permission)
    {
        return Err(CommandError::new(
            "System.ArgumentNullException",
            "Value cannot be null. (Parameter 'key')",
        ));
    }
    Ok(())
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let Some(help) = help_text(menu) else {
        return not_ported(menu);
    };
    if ctx.options.help {
        println!("{help}");
        return Ok(0);
    }
    let o = &ctx.options;
    if blank(&o.bucket_name) {
        println!("{}", usage::ERROR_BUCKET);
        return Ok(0);
    }
    if matches!(menu, GetObjectAcl | PutObjectAcl) && blank(&o.key) {
        println!("{}", usage::ERROR_KEY);
        return Ok(0);
    }
    let bucket_name = o.bucket_name.clone().unwrap_or_default();
    let key = o.key.clone().unwrap_or_default();
    match menu {
        GetBucketAcl => {
            info!("GetBucketAcl Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .get_bucket_acl(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                if o.print {
                    output::print_json(response.output.grants());
                }
                info!("{bucket_name} Get bucket ACL! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Get Bucket ACL failed({})",
                    status_name(response.status)
                );
            }
        }
        PutBucketAcl => {
            let mut policy = Option::None;
            if !blank(&o.file_path) {
                policy = read_policy(o.file_path.as_deref().unwrap_or_default())?;
            } else if ctx.acl().is_none() {
                error!("--file 로 설정파일 경로를 입력하거나 --acl 로 권한을 입력해야 합니다.");
            }
            // `--acl`은 `request.CannedACL`에만 들어가고 요청에는 쓰이지 않는다(원본 버그).
            let sdk_policy = policy
                .as_ref()
                .map(MyAccessControlList::to_policy)
                .transpose()?;
            info!("PutBucketAcl Start");
            check_permissions(&policy)?;
            let sw = Instant::now();
            let response = ctx
                .client()
                .put_bucket_acl(&bucket_name, Option::None, sdk_policy)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{bucket_name} Put Bucket Acl! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Put Bucket Acl failed({})",
                    status_name(response.status)
                );
            }
        }
        GetObjectAcl => {
            info!("GetObjectAcl Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .get_object_acl(&bucket_name, &key, o.version_id.as_deref())
                .await
                .map_err(object_model)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                if o.print {
                    output::print_json(response.output.grants());
                }
                info!("{key} Get Object ACL! complete time = {ms}ms");
            } else {
                error!(
                    "{key} Get Object ACL failed({})",
                    status_name(response.status)
                );
            }
        }
        PutObjectAcl => {
            let mut policy = Option::None;
            let mut canned = Option::None;
            if !blank(&o.file_path) {
                policy = read_policy(o.file_path.as_deref().unwrap_or_default())?;
            } else if let Some(acl) = ctx.acl() {
                canned = Some(ObjectCannedAcl::from(constant::canned_acl(acl).as_str()));
            } else {
                error!("--file 설정파일 경로를 입력하거나, --acl 권한을 입력해야 합니다.");
            }
            let sdk_policy = policy
                .as_ref()
                .map(MyAccessControlList::to_policy)
                .transpose()?;
            info!("PutObjectAcl Start");
            check_permissions(&policy)?;
            let sw = Instant::now();
            let response = ctx
                .client()
                .put_object_acl(&bucket_name, &key, canned, sdk_policy)
                .await
                .map_err(object_model)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{key} Put Object Acl! complete time = {ms}ms");
            } else {
                error!(
                    "{key} Put Object Acl failed({})",
                    status_name(response.status)
                );
            }
        }
        _ => unreachable!("help_text가 없는 메뉴는 앞에서 걸러진다"),
    }
    Ok(0)
}
