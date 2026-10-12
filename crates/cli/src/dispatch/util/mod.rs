//! 원본 `CommandDispatcher` 중 유틸리티(Legal Hold, SSE-S3, 상위 API 업로드·다운로드, 버킷 비우기).
//!
//! 원본과 같게 맞춘 동작
//!
//! - 이 모듈의 S3 오류 응답은 `Amazon.S3.AmazonS3Exception`이다. 오류 코드별 전용 예외는 `Download`의 `NoSuchKey`
//!   (`NoSuchKeyException`)와 `CurrentClear`의 `ListObjects` `NoSuchBucket`(`NoSuchBucketException`)뿐이다.
//! - `Encryption --set-sse-s3=true`는 `BucketEncryptionHelper.SetSseS3`가 예외를 잡아 `StatusCode : ..., ErrorCode : ...`와
//!   예외 내용을 로그로 남기고 `null`을 돌려주므로, 이어지는 `response.HttpStatusCode`에서 `NullReferenceException`으로
//!   끝난다(종료 코드 -1).
//! - `Upload`는 `--key`가 없으면 파일 이름을 키로 쓰고, 파일이 없으면 `ArgumentException`이다.
//!
//! 원본 버그·특이점(그대로 둔다)
//!
//! - `SetSseS3` 실패 후의 `NullReferenceException`(위).
//! - `Upload`의 완료 로그는 `Upload : complete time`, `Download`는 `Download complete time`으로 형식이 다르다.
//! - 비우기 메뉴는 버킷 이름이 비면 `--xxx-clear` 줄 바꿈 뒤 ` 버킷명을 입력해야 합니다.`를 출력한다(`MainFlag`가 줄바꿈을 포함).
//!
//! .NET과 다른 점: [`clear`] 모듈 문서 참고(삭제 작업 로그 순서 고정, 버전·삭제 마커 합치는 순서).

use super::input::blank;
use std::path::Path;
use std::time::Instant;

use aws_sdk_s3::types::{
    ObjectLockLegalHold, ObjectLockLegalHoldStatus, ServerSideEncryption,
    ServerSideEncryptionByDefault, ServerSideEncryptionConfiguration, ServerSideEncryptionRule,
};
use testrust_common::dotnet_http::status_name;
use testrust_s3::S3Error;
use tracing::{error, info};

use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;
use testrust_scenarios::clear::ClearTest;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::SetObjectLock,
    MenuList::DelObjectLock,
    MenuList::Encryption,
    MenuList::Upload,
    MenuList::Download,
    MenuList::Clear,
    MenuList::BucketClear,
    MenuList::CurrentClear,
    MenuList::NoncurrentClear,
    MenuList::MarkerClear,
];

fn help_text(menu: MenuList) -> String {
    use MenuList::*;
    match menu {
        SetObjectLock | DelObjectLock => [
            usage::main_flag(
                if menu == SetObjectLock {
                    usage::SET_OBJECT_LOCK
                } else {
                    usage::DEL_OBJECT_LOCK
                },
                "",
            ),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::optional_value(usage::VERSION_ID, "string", ""),
        ]
        .concat(),
        Encryption => [
            usage::main_flag_value(
                usage::SET_SSE_S3,
                "bool",
                " : Value=true/false. 버킷에 sse-s3 설정 여부",
            ),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        Upload => [
            usage::main_flag(usage::UPLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
            usage::optional_value(usage::KEY, "string", ""),
            usage::optional_value(usage::PART_SIZE, "string", ""),
        ]
        .concat(),
        Download => [
            usage::main_flag(usage::DOWNLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
        ]
        .concat(),
        Clear => [
            usage::main_flag_value(
                usage::UTIL_CLEAR,
                "bool",
                " : Value=true/false. 해당 유저의 모든 객체, True일 경우 버킷도 삭제.",
            ),
            usage::optional_value(usage::PREFIX, " : 버킷명 Prefix", ""),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::optional_value(
                usage::MAX_KEYS,
                " : 삭제할 객체 개수. (default : 1000)",
                "int",
            ),
        ]
        .concat(),
        BucketClear => [
            usage::main_flag_value(
                usage::UTIL_BUCKET_CLEAR,
                "string",
                " : Value=버킷 이름. 버킷에 존재하는 모든 객체를 삭제. 버킷 보존.",
            ),
            usage::optional_value(usage::PREFIX, " : 객체명 Prefix", ""),
            usage::optional_value(usage::SUFFIX, " : 객체명 Suffix", ""),
            usage::optional_value(
                usage::MAX_KEYS,
                " : 삭제할 객체 개수. (default : 1000)",
                "int",
            ),
            usage::optional_value(
                usage::FLAG,
                " : 버킷 삭제 여부. True일 경우 버킷도 삭제.",
                "bool",
            ),
        ]
        .concat(),
        CurrentClear => [
            usage::main_flag_value(
                usage::UTIL_CURRENT_CLEAR,
                "string",
                " : 버킷에 존재하는 현재 버전 객체 삭제. 버킷 보존.",
            ),
            usage::USAGE_THREAD_COUNT.to_string(),
        ]
        .concat(),
        NoncurrentClear => [
            usage::main_flag_value(
                usage::UTIL_NONCURRENT_CLEAR,
                "string",
                " : 버킷에 존재하는 이전 버전 객체 삭제. 버킷 보존.",
            ),
            usage::USAGE_THREAD_COUNT.to_string(),
        ]
        .concat(),
        _ => [
            usage::main_flag_value(
                usage::UTIL_MARKER_CLEAR,
                "string",
                " : 버킷에 존재하는 모든 Delete Marker 삭제. 버킷 보존.",
            ),
            usage::USAGE_THREAD_COUNT.to_string(),
        ]
        .concat(),
    }
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    if !PORTED.contains(&menu) {
        return not_ported(menu);
    }
    if ctx.options.help {
        println!("{}", help_text(menu));
        return Ok(0);
    }

    // 버킷 이름 검증(비우기 메뉴는 문구가 다르다).
    let clear_flag = match menu {
        BucketClear => Some(usage::UTIL_BUCKET_CLEAR),
        CurrentClear => Some(usage::UTIL_CURRENT_CLEAR),
        NoncurrentClear => Some(usage::UTIL_NONCURRENT_CLEAR),
        MarkerClear => Some(usage::UTIL_MARKER_CLEAR),
        _ => Option::None,
    };
    if menu != Clear && blank(&ctx.options.bucket_name) {
        match clear_flag {
            Some(flag) => println!("{} 버킷명을 입력해야 합니다.", usage::main_flag(flag, "")),
            Option::None => println!("{}", usage::ERROR_BUCKET),
        }
        return Ok(0);
    }
    let bucket = ctx.options.bucket_name.clone().unwrap_or_default();
    let key = ctx.options.key.clone();
    let version_id = ctx.options.version_id.clone();
    let client = ctx.client().clone();

    match menu {
        SetObjectLock | DelObjectLock => {
            if blank(&key) {
                println!("{}", usage::ERROR_KEY);
                return Ok(0);
            }
            let key = key.unwrap_or_default();
            let (on, label) = if menu == SetObjectLock {
                (ObjectLockLegalHoldStatus::On, "ON")
            } else {
                (ObjectLockLegalHoldStatus::Off, "OFF")
            };
            info!("Set Object Legal Hold ({label}) Start");
            let sw = Instant::now();
            let legal_hold = ObjectLockLegalHold::builder().status(on).build();
            let response = client
                .put_object_legal_hold(&bucket, &key, legal_hold, version_id.as_deref())
                .await
                .map_err(|e| CommandError::s3(e, &[]))?;
            let millis = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{key} Set Object Legal Hold ({label})! complete time = {millis}ms");
            } else {
                error!(
                    "{key} Set Object Legal Hold ({label}) failed({})",
                    status_name(response.status)
                );
            }
        }
        Encryption => {
            info!("BucketEncryption Start");
            if ctx.options.flag {
                let build = || -> Result<ServerSideEncryptionConfiguration, aws_sdk_s3::error::BuildError> {
                    ServerSideEncryptionConfiguration::builder()
                        .rules(
                            ServerSideEncryptionRule::builder()
                                .apply_server_side_encryption_by_default(
                                    ServerSideEncryptionByDefault::builder()
                                        .sse_algorithm(ServerSideEncryption::Aes256)
                                        .build()?,
                                )
                                .build(),
                        )
                        .build()
                };
                let config = build().map_err(|e| {
                    CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string())
                })?;
                match client.put_bucket_encryption(&bucket, config).await {
                    Ok(response) if response.status == 200 => {
                        info!("PutBucketEncryption({bucket}) Success!!");
                    }
                    Ok(_) => error!("PutBucketEncryption({bucket}) failed"),
                    Err(e) => {
                        // `SetSseS3`가 예외를 로그로 남기고 `null`을 돌려준 뒤 호출한 쪽이 NullReferenceException.
                        if let S3Error::Service { status, code, .. } = &e {
                            error!(
                                "StatusCode : {}, ErrorCode : {code}\n{}",
                                status_name(*status),
                                CommandError::s3(e.clone(), &[])
                            );
                        } else {
                            error!("{}", CommandError::from(e));
                        }
                        return Err(CommandError::new(
                            "System.NullReferenceException",
                            "Object reference not set to an instance of an object.",
                        ));
                    }
                }
            } else {
                let response = client
                    .delete_bucket_encryption(&bucket)
                    .await
                    .map_err(|e| CommandError::s3(e, &[]))?;
                if response.status == 204 {
                    info!("DeleteBucketEncryption({bucket}) : Success!!");
                } else {
                    error!("DeleteBucketEncryption({bucket}) : failed");
                }
            }
        }
        Upload => {
            if blank(&ctx.options.file_path) {
                println!("{}", usage::ERROR_FILE_PATH);
                return Ok(0);
            }
            let file_path = ctx.options.file_path.clone().unwrap_or_default();
            info!("Upload Start");
            let sw = Instant::now();
            let path = Path::new(&file_path);
            if !path.is_file() {
                return Err(CommandError::new(
                    "System.ArgumentException",
                    "The file indicated by the FilePath property does not exist!",
                ));
            }
            // TransferUtility: 키가 없으면 파일 이름
            let key = key.unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            client
                .upload(
                    &bucket,
                    &key,
                    Some(path),
                    ctx.options.part_size,
                    10,
                    Option::None,
                    Option::None,
                    Option::None,
                )
                .await
                .map_err(|e| CommandError::s3(e, &[]))?;
            info!("Upload : complete time = {}ms", sw.elapsed().as_millis());
        }
        Download => {
            if blank(&key) {
                println!("{}", usage::ERROR_KEY);
                return Ok(0);
            }
            if blank(&ctx.options.file_path) {
                println!("{}", usage::ERROR_FILE_PATH);
                return Ok(0);
            }
            let file_path = ctx.options.file_path.clone().unwrap_or_default();
            info!("Download Start");
            let sw = Instant::now();
            client
                .download(
                    &bucket,
                    &key.unwrap_or_default(),
                    Path::new(&file_path),
                    version_id.as_deref(),
                )
                .await
                .map_err(|e| CommandError::s3(e, &["NoSuchKey"]))?;
            info!("Download complete time = {}ms", sw.elapsed().as_millis());
        }
        Clear => {
            let thread = ctx.options.thread;
            info!("Clear Start");
            let sw = Instant::now();
            let mut test = ClearTest::new(client);
            test.all_clear(
                ctx.options.prefix.as_deref(),
                ctx.options.max_keys,
                ctx.options.flag,
                if thread > 0 { thread } else { 20 },
            )
            .await?;
            info!("Clear complete time = {}ms", sw.elapsed().as_millis());
        }
        BucketClear => {
            info!("BucketClear Start");
            let sw = Instant::now();
            let mut test = ClearTest::new(client);
            test.bucket_clear(
                &bucket,
                ctx.options.prefix.as_deref(),
                ctx.options.suffix.as_deref(),
                ctx.options.max_keys,
                ctx.options.flag,
            )
            .await;
            info!(
                "{bucket} BucketClear : complete time = {}ms",
                sw.elapsed().as_millis()
            );
        }
        CurrentClear => {
            info!("CurrentClear Start");
            let sw = Instant::now();
            ClearTest::new(client).current_clear(&bucket).await;
            info!(
                "CurrentClear({bucket}) : complete time = {}ms",
                sw.elapsed().as_millis()
            );
        }
        NoncurrentClear => {
            info!("NoncurrentClear Start");
            let sw = Instant::now();
            ClearTest::new(client).noncurrent_clear(&bucket).await;
            info!(
                "NoncurrentClear({bucket}) : complete time = {}ms",
                sw.elapsed().as_millis()
            );
        }
        _ => {
            info!("MarkerClear Start");
            let sw = Instant::now();
            ClearTest::new(client).marker_clear(&bucket).await;
            info!(
                "MarkerClear({bucket}) : complete time = {}ms",
                sw.elapsed().as_millis()
            );
        }
    }
    Ok(0)
}
