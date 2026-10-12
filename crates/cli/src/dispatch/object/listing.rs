//! `ListObjects`, `ListObjectsV2`, `ListObjectVersions`와 목록 출력(`Data/S3/ObjectData.cs`).

use std::time::Instant;

use aws_sdk_s3::types::Object;
use chrono::Local;
use rust_decimal::Decimal;
use testrust_s3::s3_client::ListVersions;
use tracing::info;

use super::{S3Result, bucket_name};
use crate::dispatch::output::{LINE, pad_right, utf16_len};
use crate::dispatch::{CommandContext, CommandResult};

/// 원본 `ObjectData`. 수정 시각은 .NET SDK가 UTC `DateTime`으로 돌려주므로 UTC 그대로 쓴다.
struct ObjectData {
    name: String,
    modified: String,
    size: i64,
    version_id: String,
    delete_marker: bool,
    latest: bool,
}

impl ObjectData {
    fn new(
        name: &str,
        modified: Option<&aws_sdk_s3::primitives::DateTime>,
        size: Option<i64>,
    ) -> Self {
        Self {
            name: name.to_string(),
            modified: modified_text(modified),
            size: size.unwrap_or(0),
            version_id: String::new(),
            delete_marker: false,
            latest: false,
        }
    }

    /// 원본 `SizeToString`(`Utility.GetFileSizeUint(Size)`).
    fn size_to_string(&self) -> String {
        testrust_model::units::file_size_unit(Decimal::from(self.size), false, false)
    }
}

/// `DateTime.ToString("yyyy-MM-dd HH:mm:ss", InvariantInfo)`. 값이 없으면 빈 문자열.
fn modified_text(time: Option<&aws_sdk_s3::primitives::DateTime>) -> String {
    time.map(crate::dispatch::output::invariant_time)
        .unwrap_or_default()
}

/// `x.Key.EndsWith(suffix, StringComparison.OrdinalIgnoreCase)`.
fn ends_with_ignore_case(key: &str, suffix: &str) -> bool {
    let upper = |c: char| {
        let mut mapped = c.to_uppercase();
        match (mapped.next(), mapped.next()) {
            (Some(single), None) => single,
            _ => c,
        }
    };
    let key: Vec<char> = key.chars().map(upper).collect();
    let suffix: Vec<char> = suffix.chars().map(upper).collect();
    key.ends_with(&suffix)
}

/// 원본 `[{DateTime.Now:yyyy-MM-dd HH:mm:ss}]`.
fn now_text() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 이름·폴더 열 너비 갱신(`item.Length > maxObjectLength`, UTF-16 길이).
fn widen(max: &mut usize, text: &str) {
    *max = (*max).max(utf16_len(text));
}

pub(super) async fn list_objects(ctx: &CommandContext, v2: bool) -> CommandResult {
    let name = if v2 { "ListObjectsV2" } else { "ListObjects" };
    info!("{name} Start");
    let o = &ctx.options;
    let bucket = bucket_name(ctx);
    let started = Instant::now();
    let mut marker = o.darker.clone();
    let mut end = false;
    let mut max_object_length = 0usize;
    let mut total_count = 0i64;
    let mut total_size = 0i64;
    let mut folders: Vec<String> = Vec::new();
    let mut objects: Vec<ObjectData> = Vec::new();
    let mut listing_count = 0;
    while !end {
        let (contents, prefixes, truncated, next) = if v2 {
            let response = ctx
                .client()
                .list_objects_v2(
                    bucket,
                    o.prefix.as_deref(),
                    marker.as_deref(),
                    o.max_keys,
                    o.delimiter.as_deref(),
                    None,
                )
                .await
                .modeled(&["NoSuchBucket"])?
                .output;
            (
                response.contents().to_vec(),
                prefix_list(response.common_prefixes()),
                response.is_truncated().unwrap_or(false),
                response.next_continuation_token().map(str::to_string),
            )
        } else {
            let response = ctx
                .client()
                .list_objects(
                    bucket,
                    o.prefix.as_deref(),
                    marker.as_deref(),
                    o.max_keys,
                    o.delimiter.as_deref(),
                )
                .await
                .modeled(&["NoSuchBucket"])?
                .output;
            (
                response.contents().to_vec(),
                prefix_list(response.common_prefixes()),
                response.is_truncated().unwrap_or(false),
                response.next_marker().map(str::to_string),
            )
        };
        listing_count += 1;

        let has_objects = !contents.is_empty();
        let has_prefixes = !prefixes.is_empty();
        // 둘 다 없으면 종료
        if !has_objects && !has_prefixes {
            break;
        }

        if has_objects {
            let contents: Vec<Object> = match &o.suffix {
                Some(suffix) => contents
                    .into_iter()
                    .filter(|x| ends_with_ignore_case(x.key().unwrap_or_default(), suffix))
                    .collect(),
                None => contents,
            };
            total_count += i64::try_from(contents.len()).unwrap_or(i64::MAX);
            total_size += contents.iter().map(|x| x.size().unwrap_or(0)).sum::<i64>();
            if o.print {
                for item in &contents {
                    let data = ObjectData::new(
                        item.key().unwrap_or_default(),
                        item.last_modified(),
                        item.size(),
                    );
                    if !blank_text(&data.name) {
                        widen(&mut max_object_length, &data.name);
                    }
                    objects.push(data);
                }
            }
        }
        if has_prefixes && o.print {
            for item in prefixes {
                widen(&mut max_object_length, &item);
                folders.push(item);
            }
        }
        if !o.all {
            end = true;
        }

        if truncated {
            if v2 {
                // 원본은 새 토큰이 아니라 직전 값(`marker`)을 출력한다.
                println!(
                    "[{}]Next Continuation Token : {}",
                    now_text(),
                    marker.as_deref().unwrap_or("")
                );
            } else {
                println!(
                    "[{}]Next Key Marker : {}",
                    now_text(),
                    next.as_deref().unwrap_or("")
                );
            }
            marker = next;
        } else {
            end = true;
        }
    }
    let elapsed = started.elapsed().as_millis();
    // 값 출력
    if o.print {
        println!("{LINE}");
        for item in &folders {
            println!("{}  Folder", pad_right(item, max_object_length));
        }
        for item in &objects {
            if v2 {
                println!(
                    "{}  {}  {}  {}",
                    pad_right(&item.name, max_object_length),
                    item.version_id,
                    item.modified,
                    item.size_to_string()
                );
            } else {
                println!(
                    "{}  {}  {}",
                    pad_right(&item.name, max_object_length),
                    item.modified,
                    item.size_to_string()
                );
            }
        }
        println!("{LINE}");
        println!("{} Folders, {} Objects.", folders.len(), objects.len());
    }
    if v2 {
        info!(
            "{total_count} files. {total_size} Byte. ListingCount Count = {listing_count}. complete time = {elapsed}ms"
        );
    } else {
        info!(
            "{total_count} files. {total_size} Byte. ListingCount Count = {listing_count} . complete time = {elapsed}ms"
        );
    }
    Ok(0)
}

fn prefix_list(prefixes: &[aws_sdk_s3::types::CommonPrefix]) -> Vec<String> {
    prefixes
        .iter()
        .map(|p| p.prefix().unwrap_or_default().to_string())
        .collect()
}

/// `string.IsNullOrWhiteSpace`(비어 있거나 공백뿐).
fn blank_text(text: &str) -> bool {
    text.trim().is_empty()
}

/// .NET `S3ObjectVersion`: 버전과 삭제 마커가 한 목록에 문서 순서로 들어 있다.
struct VersionItem<'a> {
    key: &'a str,
    version_id: Option<&'a str>,
    modified: Option<&'a aws_sdk_s3::primitives::DateTime>,
    size: Option<i64>,
    delete_marker: bool,
    latest: bool,
}

/// 응답의 버전과 삭제 마커를 문서 순서로(`S3Client::list_versions`가 기록한 순서).
fn version_items(response: &ListVersions) -> Vec<VersionItem<'_>> {
    response
        .entries()
        .unwrap_or_default()
        .iter()
        .map(|entry| VersionItem {
            key: entry.key().unwrap_or_default(),
            version_id: entry.version_id(),
            modified: entry.last_modified(),
            size: entry.size(),
            delete_marker: entry.is_delete_marker(),
            latest: entry.is_latest().unwrap_or(false),
        })
        .collect()
}

pub(super) async fn list_object_versions(ctx: &CommandContext) -> CommandResult {
    info!("ListObjectVersions Start");
    let o = &ctx.options;
    let bucket = bucket_name(ctx);
    let started = Instant::now();
    let mut marker = o.darker.clone();
    let mut version_id = o.version_id.clone();
    let mut end = false;
    let mut max_object_length = 0usize;
    let mut total_count = 0i64;
    let mut total_size = 0i64;
    let mut listing_count = 0;
    let mut objects: Vec<ObjectData> = Vec::new();
    while !end {
        let response = ctx
            .client()
            .list_versions(
                bucket,
                o.prefix.as_deref(),
                marker.as_deref(),
                version_id.as_deref(),
                o.max_keys,
                o.delimiter.as_deref(),
            )
            .await
            .plain()?
            .output;
        listing_count += 1;

        let items = version_items(&response);
        if !items.is_empty() {
            if !o.all {
                end = true;
            }

            let items: Vec<&VersionItem<'_>> = items
                .iter()
                .filter(|x| {
                    o.suffix
                        .as_deref()
                        .is_none_or(|suffix| ends_with_ignore_case(x.key, suffix))
                })
                .collect();
            total_count += i64::try_from(items.len()).unwrap_or(i64::MAX);
            total_size += items.iter().map(|x| x.size.unwrap_or(0)).sum::<i64>();

            if response.is_truncated() == Some(true) {
                marker = response.next_key_marker().map(str::to_string);
                version_id = response.next_version_id_marker().map(str::to_string);
                println!(
                    "Next Key Marker : {}, Next Version Id Marker : {}",
                    response.next_key_marker().unwrap_or(""),
                    response.next_version_id_marker().unwrap_or("")
                );
            } else {
                end = true;
            }

            if o.print {
                // 위치 정렬을 위한 값 찾기
                for item in items {
                    let mut data = ObjectData::new(item.key, item.modified, item.size);
                    data.version_id = item.version_id.unwrap_or_default().to_string();
                    data.delete_marker = item.delete_marker;
                    data.latest = item.latest;
                    widen(&mut max_object_length, &data.name);
                    objects.push(data);
                }
            }
        } else {
            end = true;
        }
    }
    let elapsed = started.elapsed().as_millis();

    if o.print {
        println!("{LINE}");
        // 원본의 `folderList`는 채워지지 않는다.
        for item in &objects {
            println!(
                "{} {} {} {} {} {}",
                pad_right(&item.name, max_object_length),
                item.version_id,
                item.modified,
                item.size_to_string(),
                if item.delete_marker { " True" } else { "False" },
                if item.latest { "Latest" } else { "" }
            );
        }
        println!("{LINE}");
        println!("0 Folders, {} Objects.", objects.len());
    }

    info!(
        "{total_count} files. {total_size} Byte. ListingCount Count = {listing_count}. complete time = {elapsed}ms"
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use aws_sdk_s3::primitives::DateTime;

    use super::*;

    #[test]
    fn suffix_ignores_case() {
        assert!(ends_with_ignore_case("a.TXT", ".txt"));
        assert!(ends_with_ignore_case("a.txt", ""));
        assert!(!ends_with_ignore_case("a.txt", "x.txt"));
        assert!(ends_with_ignore_case("한글.Txt", ".tXT"));
    }

    #[test]
    fn modified_is_utc_text() {
        let time = DateTime::from_secs(1_772_600_767);
        assert_eq!(modified_text(Some(&time)), "2026-03-04 05:06:07");
        assert_eq!(modified_text(None), "");
    }
}
