//! 원본 `CommandDispatcher` 중 버킷 생성·삭제·조회·목록, 버전 관리, 소유권 설정.
//!
//! 원본 동작 중 그대로 둔 것
//!
//! - `GetBucketLocation`은 성공 로그가 `Get Bucket Logging!`(실패는 `Get Bucket Logging failed`)이다.
//! - `ListDirectoryBuckets`는 `response.Buckets`가 `null`(버킷이 하나도 없는 응답)이면 `NullReferenceException`으로
//!   끝난다(`--print=false`여도 로그의 `Buckets.Count`에서 난다).
//! - `ListBuckets`·`ListDirectoryBuckets`의 이름 정렬은 `string.CompareTo`(현재 문화권, ICU)다. [`constant::culture_compare`]가
//!   ASCII 이름의 순서를 같게 맞춘다(그 밖의 문자는 서수 비교).
//! - `PutBucketVersioning`의 `--versioning Off`는 `Status` 없는 설정을 보낸다.
//! - `HeadBucket`은 `GET /{bucket}?acl`로 확인하며 `NoSuchBucket`만 없는 것으로 본다(접근 거부 등은 있는 것).
//! - `--ownership`은 파서에서 `ObjectOwnership.FindValue`(대소문자 무시)를 거친 값이다.

pub(super) mod constant;
pub(super) mod format;

use super::input::blank;
use std::time::Instant;

use aws_sdk_s3::types::{BucketCannedAcl, BucketVersioningStatus, ObjectOwnership};
use testrust_common::dotnet_http::status_name;
use tracing::{error, info};

use super::output::{self, pad_right, utf16_len};
use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::CreateBucket,
    MenuList::DeleteBucket,
    MenuList::HeadBucket,
    MenuList::ListBuckets,
    MenuList::ListDirectoryBuckets,
    MenuList::GetBucketLocation,
    MenuList::GetBucketVersioning,
    MenuList::PutBucketVersioning,
    MenuList::GetBucketOwnershipControls,
    MenuList::PutBucketOwnershipControls,
    MenuList::DeleteBucketOwnershipControls,
];

/// 메뉴별 도움말.
fn help_text(menu: MenuList) -> Option<String> {
    use MenuList::*;
    Some(match menu {
        CreateBucket => [
            usage::main_flag(usage::CREATE_BUCKET, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::optional_value(usage::ACL, "string", " : 버킷에 적용할 ACL"),
            usage::optional_value(usage::OWNERSHIP, "string", " : 버킷에 적용할 소유자"),
            usage::optional_value(usage::LOCK_ENABLE, "string", " : 버킷에 lock 모드 설정"),
        ]
        .concat(),
        DeleteBucket => [
            usage::main_flag(usage::DELETE_BUCKET, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        HeadBucket => [
            usage::main_flag(usage::HEAD_BUCKET, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        ListBuckets => [
            usage::main_flag(usage::LIST_BUCKETS, ""),
            usage::optional_value(usage::PREFIX, "string", ""),
            usage::optional_value(usage::CONTINUATION_TOKEN, "string", ""),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        ListDirectoryBuckets => [
            usage::main_flag(usage::LIST_DIRECTORY_BUCKETS, ""),
            usage::optional_value(usage::CONTINUATION_TOKEN, "string", ""),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        GetBucketLocation => [
            usage::main_flag(usage::GET_BUCKET_LOCATION, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        GetBucketVersioning => [
            usage::main_flag(usage::GET_BUCKET_VERSIONING, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        PutBucketVersioning => [
            usage::main_flag(usage::PUT_BUCKET_VERSIONING, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::VERSIONING, "string", ""),
        ]
        .concat(),
        GetBucketOwnershipControls => [
            usage::main_flag(usage::GET_BUCKET_OWNERSHIP_CONTROLS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        PutBucketOwnershipControls => [
            usage::main_flag(usage::PUT_BUCKET_OWNERSHIP_CONTROLS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::OWNERSHIP, "string", ""),
        ]
        .concat(),
        DeleteBucketOwnershipControls => [
            usage::main_flag(usage::DELETE_BUCKET_OWNERSHIP_CONTROLS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        _ => return Option::None,
    })
}

/// 버킷 목록 출력용 항목(원본 `BucketData`).
struct BucketData {
    name: String,
    modified: String,
}

/// 원본의 버킷 목록 출력: 이름순 정렬 후 가장 긴 이름에 맞춰 `PadRight`.
fn print_buckets(mut items: Vec<BucketData>) {
    let max_bucket_length = items.iter().map(|b| utf16_len(&b.name)).max().unwrap_or(0);
    items.sort_by(|x, y| constant::culture_compare(&x.name, &y.name));
    println!();
    for item in &items {
        println!(
            "{} {}",
            pad_right(&item.name, max_bucket_length),
            item.modified
        );
    }
    println!();
}

fn bucket_data<'a>(
    buckets: impl IntoIterator<
        Item = (
            Option<&'a str>,
            Option<&'a aws_sdk_s3::primitives::DateTime>,
        ),
    >,
) -> Vec<BucketData> {
    buckets
        .into_iter()
        .map(|(name, date)| BucketData {
            name: name.unwrap_or_default().to_string(),
            modified: date
                .map(crate::dispatch::output::invariant_time)
                .unwrap_or_default(),
        })
        .collect()
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
    // 모든 메뉴가 버킷 이름을 필요로 하지만 ListBuckets·ListDirectoryBuckets는 확인하지 않는다.
    if !matches!(menu, ListBuckets | ListDirectoryBuckets) && blank(&o.bucket_name) {
        println!("{}", usage::ERROR_BUCKET);
        return Ok(0);
    }
    let bucket_name = o.bucket_name.clone().unwrap_or_default();
    let print = o.print;
    match menu {
        CreateBucket => {
            let acl = ctx
                .acl()
                .map(|a| BucketCannedAcl::from(constant::canned_acl(a).as_str()));
            let ownership = o
                .ownership
                .as_deref()
                .map(|v| ObjectOwnership::from(constant::object_ownership(v).as_str()));
            info!("CreateBucket Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .put_bucket(&bucket_name, acl, Some(ctx.options.flag), ownership)
                .await
                .map_err(|e| {
                    CommandError::s3(e, &["BucketAlreadyExists", "BucketAlreadyOwnedByYou"])
                })?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{bucket_name} Create! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} : Create failed({})",
                    status_name(response.status)
                );
            }
        }
        DeleteBucket => {
            info!("DeleteBucket Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .delete_bucket(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 204 {
                info!("{bucket_name} Delete! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Delete failed({})",
                    status_name(response.status)
                );
            }
        }
        HeadBucket => {
            info!("HeadBucket Start");
            if ctx.client().does_s3_bucket_exist(&bucket_name).await {
                info!("{bucket_name} is exist!");
            } else {
                error!("{bucket_name} is not exist.");
            }
        }
        ListBuckets => {
            info!("ListingBuckets Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .list_buckets(
                    o.prefix.as_deref(),
                    o.max_keys,
                    o.continuation_token.as_deref(),
                )
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                let buckets = response.output.buckets();
                if print {
                    if buckets.is_empty() {
                        println!("버킷이 없습니다.");
                    } else {
                        print_buckets(bucket_data(
                            buckets.iter().map(|b| (b.name(), b.creation_date())),
                        ));
                    }
                }
                info!("List Bucket({})! complete time = {ms}ms", buckets.len());
            } else {
                error!("List Bucket failed({})", status_name(response.status));
            }
        }
        ListDirectoryBuckets => {
            info!("ListDirectoryBuckets Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .list_directory_buckets(o.max_keys, o.continuation_token.as_deref())
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                let buckets = response.output.buckets();
                // .NET은 버킷이 없는 응답의 `Buckets`를 `null`로 두어 `foreach`·`Count`에서 예외가 난다.
                if buckets.is_empty() {
                    return Err(CommandError::new(
                        "System.NullReferenceException",
                        "Object reference not set to an instance of an object.",
                    ));
                }
                if print {
                    print_buckets(bucket_data(
                        buckets.iter().map(|b| (b.name(), b.creation_date())),
                    ));
                }
                info!("List Bucket({})! complete time = {ms}ms", buckets.len());
            } else {
                error!("List Bucket failed({})", status_name(response.status));
            }
        }
        GetBucketLocation => {
            info!("GetBucketLocation Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .get_bucket_location(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                if print {
                    // `response.Location`(`S3Region`)의 JSON: `{ "Value": "..." }`
                    let location = response
                        .output
                        .location_constraint()
                        .map(|l| l.as_str().to_string())
                        .unwrap_or_default();
                    println!(
                        "{{\n  \"Value\": {}\n}}",
                        serde_json::Value::String(location)
                    );
                }
                // 원본 로그 문구 그대로(`Get Bucket Logging`).
                info!("{bucket_name} Get Bucket Logging! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Get Bucket Logging failed({})",
                    status_name(response.status)
                );
            }
        }
        GetBucketVersioning => {
            info!("GetBucketVersioning Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .get_bucket_versioning(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                if print {
                    output::print_json(&response.output);
                }
                info!("{bucket_name} Get Bucket Versioning! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Get Bucket Versioning failed({})",
                    status_name(response.status)
                );
            }
        }
        PutBucketVersioning => {
            let Some(versioning) = o.versioning.as_deref().filter(|v| !v.trim().is_empty()) else {
                println!("{}", usage::ERROR_VERSIONING);
                return Ok(0);
            };
            // `VersionStatus.Off`는 `Status` 없이 보낸다.
            let status = match constant::version_status(versioning).as_str() {
                "Off" => Option::None,
                other => Some(BucketVersioningStatus::from(other)),
            };
            info!("PutBucketVersioning Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .put_bucket_versioning(&bucket_name, status)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{bucket_name} Put Bucket Versioning! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Put Bucket Versioning failed({})",
                    status_name(response.status)
                );
            }
        }
        GetBucketOwnershipControls => {
            info!("GetBucketOwnershipControls Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .get_bucket_ownership_controls(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                if print {
                    match response.output.ownership_controls() {
                        Some(controls) => output::print_json(controls),
                        Option::None => println!("null"),
                    }
                }
                info!("{bucket_name} Get Bucket Ownership Controls! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Get Bucket Ownership Controls failed({})",
                    status_name(response.status)
                );
            }
        }
        PutBucketOwnershipControls => {
            let Some(ownership) = o.ownership.as_deref() else {
                println!("{}", usage::ERROR_OWNERSHIP);
                return Ok(0);
            };
            let ownership = ObjectOwnership::from(constant::object_ownership(ownership).as_str());
            info!("PutBucketOwnershipControls Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .put_bucket_ownership_controls(&bucket_name, ownership)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{bucket_name} Put Bucket Ownership Controls! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Put Bucket Ownership Controls failed({})",
                    status_name(response.status)
                );
            }
        }
        DeleteBucketOwnershipControls => {
            info!("DeleteBucketOwnershipControls Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .delete_bucket_ownership_controls(&bucket_name)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 204 {
                info!("{bucket_name} Delete Bucket Ownership Controls! complete time = {ms}ms");
            } else {
                error!(
                    "{bucket_name} Delete Bucket Ownership Controls failed({})",
                    status_name(response.status)
                );
            }
        }
        _ => unreachable!("help_text가 없는 메뉴는 앞에서 걸러진다"),
    }
    Ok(0)
}
