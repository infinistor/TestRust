//! 원본 `CommandDispatcher` 중 멀티파트 업로드 메뉴.
//!
//! 원본 동작 중 그대로 둔 것
//!
//! - `CreateMultipartUpload`는 버킷만 확인한다. `--key`가 없으면 `Start` 로그 뒤에 SDK가
//!   `ArgumentException`(`Key is a required property ...`)을 던진다.
//! - `CompleteMultipartUpload`는 입력 JSON이 `null`이면 `Start` 로그 뒤에 `NullReferenceException`이다.
//!   `Parts`가 없거나 `null` 요소가 있어도 오류 없이 빈 목록으로 보낸다.
//! - `ListParts`는 `--all`이 없으면 잘림(`IsTruncated`) 여부와 관계없이 한 번만 요청하되, 잘렸다면
//!   `Next Marker` 로그를 남긴다. `--all`이면서 `NextPartNumberMarker`가 없으면 같은 요청을 끝없이 되풀이한다.
//!   응답 상태 코드는 확인하지 않는다.
//! - `ListMultipartUploads`는 업로드가 없으면 아무것도 출력하지 않는다.
//! - `UploadPart`는 파일 존재를 미리 확인하지 않는다. 파일이 없으면 `Start` 로그 뒤에 파일 예외가 난다.
//! - 시각(`Initiated`, `LastModified`)은 SDK가 읽은 UTC 값을 ko-KR 기본 형식으로 출력한다.
//!
//! .NET과 다른 점: 입력 JSON의 `ChecksumMD5`, `ChecksumSHA512`, `ChecksumXXHASH*`는 읽기만 하고 요청에 넣지 않는다
//! (Rust SDK의 `CompletedPart`에 없다).

mod input;

use super::input::blank;
use std::path::{Path, PathBuf};
use std::time::Instant;

use testrust_common::dotnet_http::status_name;
use testrust_common::json::{ReadOptions, deserialize};
use testrust_s3::S3Error;
use testrust_s3::s3_client::PutBody;
use tracing::{error, info};

use super::bucket::format;
use super::output::{pad_right, utf16_len};
use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;
use input::MultiParts;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::AbortMultipartUpload,
    MenuList::CompleteMultipartUpload,
    MenuList::CreateMultipartUpload,
    MenuList::ListMultipartUploads,
    MenuList::ListParts,
    MenuList::UploadPart,
    MenuList::UploadPartCopy,
];

/// `CompleteMultipartUpload` 도움말의 예제(`MultiParts`).
const PARTS_EXAMPLE: &str = r#"{
  "Parts": [
    {
      "ChecksumCRC32": null,
      "ChecksumCRC32C": null,
      "ChecksumCRC64NVME": null,
      "ChecksumMD5": null,
      "ChecksumSHA1": null,
      "ChecksumSHA256": null,
      "ChecksumSHA512": null,
      "ChecksumXXHASH128": null,
      "ChecksumXXHASH3": null,
      "ChecksumXXHASH64": null,
      "ETag": "ETag 1",
      "PartNumber": 1
    },
    {
      "ChecksumCRC32": null,
      "ChecksumCRC32C": null,
      "ChecksumCRC64NVME": null,
      "ChecksumMD5": null,
      "ChecksumSHA1": null,
      "ChecksumSHA256": null,
      "ChecksumSHA512": null,
      "ChecksumXXHASH128": null,
      "ChecksumXXHASH3": null,
      "ChecksumXXHASH64": null,
      "ETag": "ETag 2",
      "PartNumber": 2
    },
    {
      "ChecksumCRC32": null,
      "ChecksumCRC32C": null,
      "ChecksumCRC64NVME": null,
      "ChecksumMD5": null,
      "ChecksumSHA1": null,
      "ChecksumSHA256": null,
      "ChecksumSHA512": null,
      "ChecksumXXHASH128": null,
      "ChecksumXXHASH3": null,
      "ChecksumXXHASH64": null,
      "ETag": "ETag 3",
      "PartNumber": 3
    }
  ]
}"#;

/// 메뉴별 도움말.
fn help_text(menu: MenuList) -> Option<String> {
    use MenuList::*;
    Some(match menu {
        AbortMultipartUpload => [
            usage::main_flag(usage::ABORT_MULTIPART_UPLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::UPLOAD_ID, "string", ""),
        ]
        .concat(),
        CompleteMultipartUpload => [
            usage::main_flag(usage::COMPLETE_MULTIPART_UPLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
            "\nParts\n".to_string(),
            PARTS_EXAMPLE.to_string(),
        ]
        .concat(),
        CreateMultipartUpload => [
            usage::main_flag(usage::CREATE_MULTIPART_UPLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
        ]
        .concat(),
        ListMultipartUploads => [
            usage::main_flag(usage::LIST_MULTIPART_UPLOADS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::optional_value(usage::PREFIX, "string", ""),
            usage::optional_value(usage::DELIMITER, "string", ""),
            usage::optional_value(usage::UPLOAD_ID, "string", " : 업로드 아이디"),
            usage::optional_value(usage::MARKER, "string", " : Next Key Marker"),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        ListParts => [
            usage::main_flag(usage::LIST_PARTS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::UPLOAD_ID, "string", ""),
            usage::optional_value(usage::PART_NUMBER, "string", ""),
            " : Next Part Number".to_string(),
            usage::optional_value(usage::MAX_KEYS, "int", ""),
        ]
        .concat(),
        UploadPart => [
            usage::main_flag(usage::UPLOAD_PART, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::UPLOAD_ID, "string", ""),
            usage::sub_flag(usage::PART_NUMBER, "int", ""),
            usage::sub_flag(usage::FILE, "string", " : 업로드 파일 경로"),
        ]
        .concat(),
        UploadPartCopy => [
            usage::main_flag(usage::UPLOAD_PART_COPY, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::SOURCE, "string", " : 원본 버킷명"),
            usage::sub_flag(usage::SOURCE_KEY, "string", " : 원본 오브젝트명"),
            usage::sub_flag(usage::UPLOAD_ID, "string", ""),
            usage::sub_flag(usage::START_BYTE, "string", " : 시작 바이트(ex> 1K, 10MB)"),
            usage::sub_flag(usage::END_BYTE, "string", " : 종료 바이트(ex> 1K, 10MB)"),
            usage::optional_value(usage::VERSION_ID, "string", ""),
        ]
        .concat(),
        _ => return Option::None,
    })
}

/// `AbortMultipartUpload`가 전용 예외(`NoSuchUploadException`)로 던지는 오류.
fn abort_model(error: S3Error) -> CommandError {
    CommandError::s3(error, &["NoSuchUpload"])
}

/// `UploadPart`의 오류. 파일이 없으면 `FileNotFoundException.ToString()`의 `File name:` 줄이 붙는다.
fn upload_part_error(error: S3Error) -> CommandError {
    if let S3Error::Io {
        dotnet_type: "System.IO.FileNotFoundException",
        message,
    } = &error
    {
        // 메시지는 `Could not find file '<path>'.`
        if let Some(path) = message
            .strip_prefix("Could not find file '")
            .and_then(|m| m.strip_suffix("'."))
        {
            return CommandError::new(
                "System.IO.FileNotFoundException",
                format!("{message}\nFile name: '{path}'"),
            );
        }
    }
    CommandError::from(error)
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
    // 메뉴별 필수 값 확인(원본 `else if` 순서).
    let needs_key = !matches!(menu, CreateMultipartUpload | ListMultipartUploads);
    if needs_key && blank(&o.key) {
        println!("{}", usage::ERROR_KEY);
        return Ok(0);
    }
    if menu == UploadPartCopy {
        if blank(&o.source) {
            println!("{}", usage::ERROR_SOURCE_BUCKET);
            return Ok(0);
        }
        if blank(&o.source_key) {
            println!("{}", usage::ERROR_SOURCE_KEY);
            return Ok(0);
        }
    }
    if matches!(
        menu,
        AbortMultipartUpload | CompleteMultipartUpload | ListParts | UploadPart | UploadPartCopy
    ) && blank(&o.upload_id)
    {
        println!("{}", usage::ERROR_UPLOAD_ID);
        return Ok(0);
    }
    if matches!(menu, UploadPart | UploadPartCopy) && o.part_number < 1 {
        println!("{}", usage::ERROR_PART_NUMBER);
        return Ok(0);
    }
    let bucket_name = o.bucket_name.clone().unwrap_or_default();
    let key = o.key.clone().unwrap_or_default();
    let upload_id = o.upload_id.clone().unwrap_or_default();
    match menu {
        AbortMultipartUpload => {
            info!("AbortMultipartUpload Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .abort_multipart_upload(&bucket_name, &key, &upload_id)
                .await
                .map_err(abort_model)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 204 {
                info!("Abort Multipart Upload complete time = {ms}ms");
            } else {
                error!(
                    "Abort Multipart Upload failed ({})",
                    status_name(response.status)
                );
            }
        }
        CompleteMultipartUpload => {
            if blank(&o.file_path) {
                println!("--file 멀티파트 목록 파일 경로를 입력해야 합니다.");
                return Ok(0);
            }
            let file_path = o.file_path.as_deref().unwrap_or_default();
            if !Path::new(file_path).is_file() {
                println!("{}", usage::ERROR_FILE);
                return Ok(0);
            }
            let text = format::read_all_text(file_path)?;
            let setting = deserialize::<MultiParts>(&text, ReadOptions::default())
                .map_err(format::json_error)?;

            info!("CompleteMultipartUpload Start");
            // `setting.Parts`: 입력이 `null`이면 NullReferenceException.
            let setting = setting.ok_or_else(format::null_reference)?;
            let parts = setting.parts.unwrap_or_default();
            let sw = Instant::now();
            let response = ctx
                .client()
                .complete_multipart_upload(&bucket_name, &key, &upload_id, &parts)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("Complete Multipart Upload complete time = {ms}ms");
            } else {
                error!("Complete Multipart Upload failed");
            }
        }
        CreateMultipartUpload => {
            info!("InitiateMultipartUpload Start");
            let sw = Instant::now();
            // 키가 없으면 SDK가 요청을 만들 때 던지는 예외.
            let Some(key) = o.key.as_deref() else {
                return Err(CommandError::new(
                    "System.ArgumentException",
                    "Key is a required property and must be set before making this call. \
                     (Parameter 'InitiateMultipartUploadRequest.Key')",
                ));
            };
            let response = ctx
                .client()
                .initiate_multipart_upload(&bucket_name, key)
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!(
                    "{key} Upload Id is {}. complete time = {ms}ms",
                    response.output.upload_id().unwrap_or_default()
                );
            } else {
                error!("{key} : Create failed({})", status_name(response.status));
            }
        }
        ListMultipartUploads => {
            info!("ListMultipartUploads Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .list_multipart_uploads(
                    &bucket_name,
                    o.prefix.as_deref(),
                    o.upload_id.as_deref(),
                    o.darker.as_deref(),
                    o.max_keys,
                    o.delimiter.as_deref(),
                )
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                let uploads = response.output.uploads();
                if o.print && !uploads.is_empty() {
                    // 출력전 가장 긴 객체 이름 찾기(`Key`가 `null`이면 NullReferenceException)
                    let mut max_length = 0;
                    for upload in uploads {
                        let key = upload.key().ok_or_else(format::null_reference)?;
                        max_length = max_length.max(utf16_len(key));
                    }
                    println!();
                    for upload in uploads {
                        println!(
                            "{} {} {}",
                            pad_right(upload.key().unwrap_or_default(), max_length),
                            upload
                                .initiated()
                                .map(crate::dispatch::output::ko_kr_time)
                                .unwrap_or_default(),
                            upload.upload_id().unwrap_or_default()
                        );
                    }
                    println!();
                }
                info!(
                    "{bucket_name} List Multipart Uploads({})! complete time = {ms}ms",
                    uploads.len()
                );
            } else {
                error!(
                    "{bucket_name} List Multipart Uploads failed({})",
                    status_name(response.status)
                );
            }
        }
        ListParts => {
            info!("ListParts Start");
            let sw = Instant::now();
            let mut end = false;
            let mut part_number_marker = o.part_number;
            let mut parts = Vec::new();
            while !end {
                let response = ctx
                    .client()
                    .list_parts(
                        &bucket_name,
                        &key,
                        &upload_id,
                        part_number_marker,
                        o.max_keys,
                    )
                    .await
                    .map_err(CommandError::from)?;
                if !o.all {
                    end = true;
                }
                if response.output.is_truncated() == Some(true) {
                    part_number_marker = response
                        .output
                        .next_part_number_marker()
                        .and_then(|m| m.parse::<i32>().ok())
                        .unwrap_or(0);
                    info!("Next Marker: {part_number_marker}");
                } else {
                    end = true;
                }
                parts.extend_from_slice(response.output.parts());
            }
            let ms = sw.elapsed().as_millis();
            // 값 출력
            if o.print {
                println!();
                for item in &parts {
                    println!(
                        "{} {} {} {}",
                        item.part_number()
                            .map(|n| n.to_string())
                            .unwrap_or_default(),
                        item.last_modified()
                            .map(crate::dispatch::output::ko_kr_time)
                            .unwrap_or_default(),
                        item.size().map(|s| s.to_string()).unwrap_or_default(),
                        item.e_tag().unwrap_or_default()
                    );
                }
                println!();
            }
            info!("{} parts. complete time = {ms}ms", parts.len());
        }
        UploadPart => {
            if blank(&o.file_path) {
                println!("{}", usage::ERROR_FILE_PATH);
                return Ok(0);
            }
            let file_path = o.file_path.as_deref().unwrap_or_default();
            info!("UploadPart Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .upload_part(
                    &bucket_name,
                    &key,
                    &upload_id,
                    o.part_number,
                    PutBody::File(PathBuf::from(file_path)),
                    0,
                    -1,
                    true,
                )
                .await
                .map_err(upload_part_error)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("{key} Upload Part! complete time = {ms}ms");
            } else {
                error!("{key} Upload Part failed({})", status_name(response.status));
            }
        }
        UploadPartCopy => {
            if o.start_byte < 0 {
                println!("--start-byte 시작 바이트값을 입력해야 합니다.");
                return Ok(0);
            }
            if o.end_byte < 1 || o.end_byte < o.start_byte {
                println!("--end-byte 종료 바이트값을 입력해야 합니다.");
                return Ok(0);
            }
            info!("UploadPartCopy Start");
            let sw = Instant::now();
            let response = ctx
                .client()
                .copy_part(
                    o.source.as_deref().unwrap_or_default(),
                    o.source_key.as_deref().unwrap_or_default(),
                    &bucket_name,
                    &key,
                    &upload_id,
                    o.part_number,
                    o.start_byte,
                    o.end_byte,
                    o.version_id.as_deref(),
                )
                .await
                .map_err(CommandError::from)?;
            let ms = sw.elapsed().as_millis();
            if response.status == 200 {
                info!("UploadPartCopy : Create! complete time = {ms}ms");
            } else {
                error!("UploadPartCopy() : Create failed");
            }
        }
        _ => unreachable!("help_text가 없는 메뉴는 앞에서 걸러진다"),
    }
    Ok(0)
}
