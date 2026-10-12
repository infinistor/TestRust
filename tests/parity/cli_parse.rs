//! `CliOptionParser.Parse` 비교: `baseline/cli/parse.json`(오라클 `cli-parse`)과 같은 옵션 값·Extra·예외를 내는지 본다.
//! 오라클은 기본값과 다른 속성만 기록하므로 여기서도 기본값과 다른 항목만 비교한다.

#![recursion_limit = "256"]

use std::path::Path;

use serde_json::{Map, Value, json};
use testrust_cli::options::{CommandOptions, ParseError, parse};

fn baseline(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parity/baseline/cli")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// 오라클 `DumpOptions`와 같은 이름·형식.
fn dump(o: &CommandOptions) -> Map<String, Value> {
    let fields = json!({
        "Help": o.help, "Worker": o.worker, "Controller": o.controller, "Version": o.version,
        "ConfigPath": o.config_path, "BucketName": o.bucket_name, "Source": o.source, "Target": o.target,
        "Key": o.key, "SourceKey": o.source_key, "FilePath": o.file_path, "Path": o.path, "Tags": o.tags,
        "StrACL": o.str_acl, "Versioning": o.versioning, "VersionId": o.version_id, "StorageClass": o.storage_class,
        "Ownership": o.ownership, "LockMode": o.lock_mode, "Body": o.body, "Url": o.url,
        "AccessKey": o.access_key, "SecretKey": o.secret_key, "EncryptionKey": o.encryption_key, "UserName": o.user_name,
        "BucketType": o.bucket_type.0, "Another": o.another, "Print": o.print, "ALL": o.all, "Check": o.check,
        "Bypass": o.bypass, "Flag": o.flag, "Random": o.random, "Multipart": o.multipart, "Debug": o.debug,
        "Checksum": o.checksum, "ChecksumType": o.checksum_type.name(), "Md5sum": o.md5sum, "Id": o.id,
        "Bulk": o.bulk, "ThreadPrefix": o.thread_prefix, "Admin": o.admin, "UseChunkEncoding": o.use_chunk_encoding,
        "Prefix": o.prefix, "Suffix": o.suffix, "Delimiter": o.delimiter, "Darker": o.darker,
        "ContinuationToken": o.continuation_token, "MaxKeys": o.max_keys, "Days": o.days, "Years": o.years,
        "Date": o.date, "UploadId": o.upload_id, "PartNumber": o.part_number, "PartSize": o.part_size,
        "StartByte": o.start_byte, "EndByte": o.end_byte, "FileSize": o.file_size, "RangeList": o.range_list,
        "StartCount": o.start_count, "Thread": o.thread, "Count": o.count, "Times": o.times, "Read": o.read,
        "Write": o.write, "Delete": o.delete, "Save": o.save, "ServiceType": o.service_type, "Address": o.address,
        "Port": o.port, "TargetPath": o.target_path, "Menu": format!("{:?}", o.menu),
    });
    let Value::Object(map) = fields else {
        unreachable!()
    };
    map
}

fn changed(o: &CommandOptions) -> Value {
    let defaults = dump(&CommandOptions::default());
    Value::Object(
        dump(o)
            .into_iter()
            .filter(|(k, v)| defaults[k] != *v)
            .collect(),
    )
}

#[test]
fn parse_matches_dotnet() {
    let cases: Vec<Value> = serde_json::from_str(&baseline("parse.json")).unwrap();
    assert!(cases.len() > 200);
    let mut failures = Vec::new();
    for case in &cases {
        let args: Vec<String> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_string())
            .collect();
        let actual = match parse(&args) {
            Ok(result) => json!({
                "options": changed(&result.options),
                "extra": result.extra,
                "error": null,
            }),
            Err(error) => {
                let option_name = match &error {
                    ParseError::Option { option_name, .. } => option_name.clone(),
                    ParseError::Unhandled { .. } => None,
                };
                json!({
                    "options": null,
                    "extra": null,
                    "error": { "type": error.dotnet_type(), "message": error.to_string(), "optionName": option_name },
                })
            }
        };
        let expected =
            json!({ "options": case["options"], "extra": case["extra"], "error": case["error"] });
        if actual != expected {
            failures.push(format!(
                "{args:?}\n  expected {expected}\n  actual   {actual}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 건 불일치:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
