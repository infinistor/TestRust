//! 원본 `Util/Utility.cs` 중 시나리오가 쓰는 것(다른 크레이트에 없는 것만).
//!
//! 이미 옮긴 것: `CreateRandomFile`·`GetETag(path)`·`GetMD5` → `testrust_clients::file_util`,
//! `RandomTextLong` → `testrust_config::util::random_text_long`, `GetFileSizeUint` → `testrust_model::units`,
//! `GetFileList`·`SaveFile` → [`crate::files`].
//!
//! 원본 `S3Client`는 `GetAwaiter().GetResult()`로 기다려 예외를 그대로 던진다. 그래서 원본 곳곳의
//! `catch (AggregateException e)`는 실행되지 않고 `catch (Exception e) { log.Error(e); }`가 `형식: 메시지`를 남긴다.

use std::path::Path;

use md5::{Digest, Md5};
use rand::Rng;
use testrust_s3::S3Client;
use tracing::error;

use crate::ScenarioError;
use crate::files::file_list;

/// 원본 `TEXT`.
const TEXT: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// 원본 `GetDummyFileName(index, root = null)`.
pub fn dummy_file_name(index: i32, root: Option<&str>) -> String {
    match root {
        None => format!("test/FILE_{index:03}"),
        Some(root) => format!("{root}/FILE_{index:03}"),
    }
}

/// 원본 `SanitizeFileName`: `Path.GetInvalidFileNameChars()`(Windows 기준: 제어 문자와 `"<>|:*?\/`)를 `_`로 바꾼다.
/// Linux .NET은 `/`와 `\0`만 바꾸지만 기준 출력이 Windows라 Windows 집합을 쓴다.
pub fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if (c as u32) < 32 || "\"<>|:*?\\/".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// 원본 `RandomText(length)`: 소문자·숫자.
pub fn random_text(length: usize) -> String {
    let mut rng = rand::rng();
    (0..length)
        .map(|_| TEXT[rng.random_range(0..TEXT.len())] as char)
        .collect()
}

/// 원본 `GetMD5HexFromString(content)`: UTF-8 바이트 MD5의 소문자 16진수.
pub fn md5_hex_from_string(content: &str) -> String {
    Md5::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 원본 `Utility.CreateBucket(client, bucketName)`: 없으면 만든다. 실패는 로그만 남긴다.
pub async fn create_bucket(client: &S3Client, bucket_name: &str) {
    if client.does_s3_bucket_exist(bucket_name).await {
        return;
    }
    match client.put_bucket(bucket_name, None, None, None).await {
        Ok(response) if response.status == 200 => {}
        Ok(_) => error!("CreateBucket({bucket_name}) : Create failed"),
        Err(e) => error!("{}", ScenarioError::from(e)),
    }
}

/// 원본 `GetNewBucket(client, prefix, length = 10)`.
pub async fn get_new_bucket(client: &S3Client, prefix: &str, length: usize) -> String {
    let bucket_name = format!("{prefix}{}", random_text(length));
    create_bucket(client, &bucket_name).await;
    bucket_name
}

/// 원본 `CompareDirMD5(source, target)`: 두 디렉터리의 파일 목록(전체 경로)을 정렬해 차례로 MD5를 비교한다.
/// 원본 `List<string>.Sort()`는 문화권 비교지만 여기서는 서수 비교다.
pub fn compare_dir_md5(source: &str, target: &str) -> Result<bool, ScenarioError> {
    let mut source_list = file_list(source)?;
    let mut target_list = file_list(target)?;
    if source_list.len() != target_list.len() {
        return Ok(false);
    }
    source_list.sort();
    target_list.sort();
    for (s, t) in source_list.iter().zip(&target_list) {
        let source_md5 = crate::files::file_md5_base64(s)?;
        let target_md5 = crate::files::file_md5_base64(t)?;
        if source_md5 != target_md5 {
            error!("MD5 Compare Failed. ({s} : {source_md5}, {t} : {target_md5})");
            return Ok(false);
        }
    }
    Ok(true)
}

/// 원본 `GetETag(fileName)`: 파일 MD5의 소문자 16진수. 파일이 없으면 `FileNotFoundException`.
pub fn file_etag(path: &str) -> Result<String, ScenarioError> {
    let full = crate::input::full_path_of(path);
    testrust_clients::file_util::file_etag(Path::new(&full))
        .map_err(|e| crate::input::io_error(&full, &e))
}

/// 원본 `Path.DirectorySeparatorChar`.
const SEPARATOR: char = if cfg!(windows) { '\\' } else { '/' };

fn is_directory_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// 원본 `Path.HasExtension`: 마지막 경로 요소에서 끝이 아닌 자리에 `.`이 있으면 `true`.
pub fn has_extension(path: &str) -> bool {
    let chars: Vec<char> = path.chars().collect();
    for (i, c) in chars.iter().enumerate().rev() {
        if *c == '.' {
            return i != chars.len() - 1;
        }
        if is_directory_separator(*c) || (cfg!(windows) && *c == ':') {
            break;
        }
    }
    false
}

/// 원본 `Path.Combine(first, second)`(`second`는 상대 경로 파일 이름).
pub fn path_combine(first: &str, second: &str) -> String {
    if first.is_empty() {
        return second.to_string();
    }
    let last = first.chars().last().unwrap_or(SEPARATOR);
    if is_directory_separator(last) || (cfg!(windows) && last == ':') {
        format!("{first}{second}")
    } else {
        format!("{first}{SEPARATOR}{second}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_file_names() {
        assert_eq!(dummy_file_name(7, None), "test/FILE_007");
        assert_eq!(dummy_file_name(1234, Some("/tmp/a")), "/tmp/a/FILE_1234");
    }

    #[test]
    fn sanitizes_windows_invalid_chars() {
        assert_eq!(
            sanitize_file_name("Write/Read/Delete(5:3:2)"),
            "Write_Read_Delete(5_3_2)"
        );
        assert_eq!(
            sanitize_file_name("a*b?c\"d<e>f|g\\h\ti"),
            "a_b_c_d_e_f_g_h_i"
        );
        assert_eq!(sanitize_file_name("Local Write"), "Local Write");
    }

    #[test]
    fn md5_hex() {
        assert_eq!(md5_hex_from_string(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            md5_hex_from_string("abc"),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert!(random_text(10).bytes().all(|b| TEXT.contains(&b)));
    }
}
