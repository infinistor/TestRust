//! 원본 `CommandDispatcher` 중 KSAN 확장 API(태그 인덱스, 태그 검색).
//!
//! 원본과 같게 맞춘 동작
//!
//! - 필수 값 검증이 없다. 버킷 이름이 없으면 그대로 요청하거나 `KsanClient`가 던지는 예외로 끝난다.
//! - `DeleteBucketTagIndex`·`GetBucketTagIndex`는 예외를 잡아 `ERROR`로 기록하고 종료 코드 0으로 끝난다.
//!   `ListBucketTagSearch`·`PutBucketTagIndex`는 예외를 잡지 않아 최상위가 -1로 끝낸다.
//! - `ListBucketTagSearch --debug`는 응답을 JSON으로 먼저 출력한다. 목록 출력의 수정 시각은 `yyyy-mm-dd HH:MM:ss`로
//!   분과 월이 뒤바뀐다. 객체 이름이 없으면(`Key == null`) `NullReferenceException`이다.
//!
//! 원본 버그(그대로 둔다)
//!
//! - `DeleteBucketTagIndex`는 서명 리전이 `null`이라 요청을 보내기 전에 항상
//!   `System.ArgumentNullException: Value cannot be null. (Parameter 's')`로 실패한다.
//! - `KsanException`은 응답 본문을 두 번 읽어 오류 내용과 관계없이 `Stream was not readable.`이다.
//! - `ListBucketTagSearch`의 날짜 형식 `yyyy-mm-dd HH:MM:ss`(분 `mm`과 월 `MM`이 뒤바뀜).

use std::time::Instant;

use testrust_common::to_dotnet_json;
use testrust_s3::ksan::{KsanClient, KsanError};
use tracing::{error, info};

use super::output::{LINE, pad_right, utf16_len};
use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::DeleteBucketTagIndex,
    MenuList::GetBucketTagIndex,
    MenuList::ListBucketTagSearch,
    MenuList::PutBucketTagIndex,
];

fn help_text(menu: MenuList) -> String {
    match menu {
        MenuList::DeleteBucketTagIndex => [
            usage::main_flag(usage::DELETE_BUCKET_TAG_INDEX, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        MenuList::GetBucketTagIndex => [
            usage::main_flag(usage::GET_BUCKET_TAG_INDEX, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        MenuList::ListBucketTagSearch => [
            usage::main_flag(usage::LIST_BUCKET_TAG_SEARCH, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(
                usage::TAGS,
                "string",
                " : 태그 설정 정보(ex> tag1:value1,tag2:value2)",
            ),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        _ => [
            usage::main_flag(usage::PUT_BUCKET_TAG_INDEX, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
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
    let bucket = ctx.options.bucket_name.clone().unwrap_or_default();
    let debug = ctx.options.debug;
    match menu {
        DeleteBucketTagIndex => {
            let result: Result<u128, KsanError> = async {
                info!("DeleteBucketTagIndex Start");
                let sw = Instant::now();
                let client = KsanClient::from_user(&ctx.config().main_user, debug)?;
                client.delete_bucket_tag_index(&bucket).await?;
                Ok(sw.elapsed().as_millis())
            }
            .await;
            match result {
                Ok(millis) => info!("Success! complete time = {millis}ms"),
                Err(e) => error!("{}", CommandError::from(e)),
            }
        }
        GetBucketTagIndex => {
            let result: Result<(String, u128), KsanError> = async {
                info!("GetBucketTagIndex Start");
                let sw = Instant::now();
                let client = KsanClient::from_user(&ctx.config().main_user, debug)?;
                let response = client.get_bucket_tag_index(&bucket).await?;
                Ok((response.status, sw.elapsed().as_millis()))
            }
            .await;
            match result {
                Ok((status, millis)) => info!(
                    "Bucket({bucket}) TagIndex Config is {status}. complete time = {millis}ms"
                ),
                Err(e) => error!("{}", CommandError::from(e)),
            }
        }
        ListBucketTagSearch => {
            info!("ListBucketTagSearch Start");
            let sw = Instant::now();
            let client = KsanClient::from_user(&ctx.config().main_user, debug)?;
            let tags = ctx.options.tags.clone().unwrap_or_default();
            let response = client
                .list_bucket_tag_search(&bucket, &tags, ctx.options.max_keys)
                .await?;
            let millis = sw.elapsed().as_millis();

            if debug {
                println!("{}", to_dotnet_json(&response));
            }
            if ctx.options.print {
                let mut max_object_length = 0;
                let mut rows = Vec::new();
                for data in &response.contents {
                    // `data.Key.Length`: 이름이 없으면 NullReferenceException
                    let key = data.key.as_deref().ok_or_else(|| {
                        CommandError::new(
                            "System.NullReferenceException",
                            "Object reference not set to an instance of an object.",
                        )
                    })?;
                    max_object_length = max_object_length.max(utf16_len(key));
                    rows.push((
                        key,
                        data.last_modified.format("yyyy-mm-dd HH:MM:ss"),
                        // `Utility.GetFileSizeUint(size)`
                        testrust_model::units::file_size_unit(
                            i64::from(data.size).into(),
                            false,
                            false,
                        ),
                    ));
                }
                println!("{LINE}");
                for (name, modified, size) in &rows {
                    // 원본 `ObjectData.VersionId`는 빈 문자열이다.
                    println!(
                        "{}    {modified}  {size}",
                        pad_right(name, max_object_length)
                    );
                }
                println!("{LINE}");
            }
            info!(
                "{} files. complete time = {millis}ms",
                response.contents.len()
            );
        }
        _ => {
            info!("PutBucketTagIndex Start");
            let sw = Instant::now();
            let client = KsanClient::from_user(&ctx.config().main_user, debug)?;
            client.put_bucket_tag_index(&bucket).await?;
            info!("Success! complete time = {}ms", sw.elapsed().as_millis());
        }
    }
    Ok(0)
}
