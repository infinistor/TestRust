//! 원본 `CommandDispatcher` 중 버킷 Public Access Block·정책·태그·수명주기·복제 설정.
//!
//! 원본과 같게 맞춘 동작
//!
//! - Put 메뉴는 `--file`을 읽어 `JsonSerializer.Deserialize<T>`(System.Text.Json 규칙, `JsonException` 메시지 포함)로
//!   해석한다. 해석·`--target` 치환 중 난 예외는 `... Start` 로그 전에, `null` 설정(`setting`이 `null`)에서 난
//!   `NullReferenceException`은 로그 뒤에 나온다.
//! - 오류 응답은 이 모듈의 모든 연산에서 `Amazon.S3.AmazonS3Exception`이다(`NoSuchBucket` 등을 별도 예외로 모델링하지 않는다).
//! - `GetBucketLifecycle`·`GetBucketPolicy`·`GetBucketReplication`은 404 응답이 예외가 아니라 `NotFound` 상태의
//!   응답이라 `failed(NotFound)` 로그만 남기고 종료 코드는 0이다(그 밖의 상태는 예외).
//! - `GetBucketLifecycle`은 SDK 응답이 아니라 `MyLifecycleConfiguration` DTO를 출력하므로(`dump` 없이) 글자 단위로 같다.
//!
//! 원본 버그·특이점(그대로 둔다)
//!
//! - `PutBucketReplication` 도움말 예제는 SDK `ReplicationConfiguration`을 직렬화한 것(`Destination.BucketArn`)이지만
//!   실제 입력은 `MyReplicationConfiguration`(`Destination.Bucket`)이라 예제를 그대로 파일로 쓰면 대상 버킷이 비어 있다.
//! - `PutBucketPolicy` 도움말은 `Dictionary.ToString()`이라 형식 이름만 출력한다.
//! - `--target` 치환은 `Rules`·`Destination`·`Bucket`이 없으면 `NullReferenceException`이다.
//! - 수명주기 `Filter`는 `Prefix`만 옮기고(`LifecyclePrefixPredicate`), `Prefix`가 없어도 빈 `<Prefix>`를 보낸다.
//!   복제 `Filter`는 읽기만 하고 요청에 넣지 않는다.
//!
//! .NET과 다른 점(SDK 모델이 필수 값을 요구해 같은 요청을 만들 수 없는 경우)
//!
//! - 필수 값이 빠진 입력(복제 `Role`·`Destination.Bucket`, 태그의 `Key`·`Value`)은 표식(`UNSET`)으로 모델을 만들고
//!   S3 클라이언트가 서명 전에 그 요소를 지워 .NET처럼 생략한다. 수명주기 `Status`가 없으면 .NET처럼 `Disabled`,
//!   복제 규칙 `Status`가 없으면 .NET처럼 `ArgumentNullException`이다.
//!   `TagSet`이 없는 입력은 .NET이 `<Tagging/>`을 보내지만 여기서는 빈 `<TagSet/>`이 붙는다.
//! - PublicAccessBlock 파일이 JSON `null`이면 .NET은 본문 없는 요청을 보내지만 여기서는 빈 설정을 보낸다.
//! - 수명주기 `Expiration.Date`의 소수 초는 .NET이 밀리초 3자리(`.500`)로, SDK가 최소 자릿수(`.5`)로 쓴다.

mod dto;

use super::input::blank;
use std::path::Path;
use std::time::Instant;

use aws_sdk_s3::types::BucketLifecycleConfiguration;
use testrust_common::dotnet_http::status_name;
use testrust_common::json::{FromJson, ReadOptions, deserialize};
use testrust_common::to_dotnet_json;
use testrust_s3::{S3Error, S3Response};
use tracing::{error, info};

use self::dto::{
    MyLifecycleConfiguration, MyReplicationConfiguration, PublicAccessBlockInput, TaggingInput,
};
use super::output::print_json;
use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::GetPublicAccessBlock,
    MenuList::PutPublicAccessBlock,
    MenuList::DeletePublicAccessBlock,
    MenuList::GetBucketPolicy,
    MenuList::PutBucketPolicy,
    MenuList::DeleteBucketPolicy,
    MenuList::GetBucketPolicyStatus,
    MenuList::GetBucketTagging,
    MenuList::PutBucketTagging,
    MenuList::DeleteBucketTagging,
    MenuList::GetBucketLifecycle,
    MenuList::PutBucketLifecycle,
    MenuList::DeleteBucketLifecycle,
    MenuList::GetBucketReplication,
    MenuList::PutBucketReplication,
    MenuList::DeleteBucketReplication,
];

/// 원본 도움말의 `LifeCycle` 예제(.NET `JsonSerializer.Serialize` 출력).
const LIFECYCLE_EXAMPLE: &str = "{\n  \"Rules\": [\n    {\n      \"Id\": \"rule1\",\n      \"Status\": \"Enabled\",\n      \"Expiration\": {\n        \"Days\": 1,\n        \"Date\": null,\n        \"ExpiredObjectDeleteMarker\": null\n      },\n      \"NoncurrentVersionExpiration\": {\n        \"NoncurrentDays\": 1,\n        \"NewerNoncurrentVersions\": null\n      },\n      \"Filter\": null,\n      \"AbortIncompleteMultipartUpload\": null\n    },\n    {\n      \"Id\": \"rule2\",\n      \"Status\": \"Enabled\",\n      \"Expiration\": {\n        \"Days\": null,\n        \"Date\": null,\n        \"ExpiredObjectDeleteMarker\": true\n      },\n      \"NoncurrentVersionExpiration\": null,\n      \"Filter\": null,\n      \"AbortIncompleteMultipartUpload\": {\n        \"DaysAfterInitiation\": 1\n      }\n    }\n  ]\n}";

/// 원본 도움말의 `Replication` 예제(.NET `JsonSerializer.Serialize` 출력).
const REPLICATION_EXAMPLE: &str = "{\n  \"Role\": \"arn:aws:iam::635518764071:role/awsreplicationtest\",\n  \"Rules\": [\n    {\n      \"DeleteMarkerReplication\": {\n        \"Status\": {\n          \"Value\": \"Disabled\"\n        }\n      },\n      \"Destination\": {\n        \"AccessControlTranslation\": null,\n        \"AccountId\": null,\n        \"BucketArn\": \"arn:aws:s3:::TargetBucketName\",\n        \"EncryptionConfiguration\": null,\n        \"Metrics\": null,\n        \"ReplicationTime\": null,\n        \"StorageClass\": null\n      },\n      \"ExistingObjectReplication\": null,\n      \"Filter\": null,\n      \"Id\": \"Rule1\",\n      \"Prefix\": null,\n      \"Priority\": 1,\n      \"SourceSelectionCriteria\": null,\n      \"Status\": {\n        \"Value\": \"Enabled\"\n      }\n    }\n  ]\n}";

/// 원본 도움말의 `Tagging` 예제(.NET `JsonSerializer.Serialize` 출력).
const TAGGING_EXAMPLE: &str = "{\n  \"TagSet\": [\n    {\n      \"Key\": \"0\",\n      \"Value\": \"0\"\n    },\n    {\n      \"Key\": \"1\",\n      \"Value\": \"1\"\n    },\n    {\n      \"Key\": \"2\",\n      \"Value\": \"2\"\n    },\n    {\n      \"Key\": \"3\",\n      \"Value\": \"3\"\n    },\n    {\n      \"Key\": \"4\",\n      \"Value\": \"4\"\n    }\n  ]\n}";

/// 원본 도움말의 `PublicAccessBlock` 예제(.NET `JsonSerializer.Serialize` 출력).
const PUBLIC_ACCESS_BLOCK_EXAMPLE: &str = "{\n  \"BlockPublicAcls\": true,\n  \"BlockPublicPolicy\": true,\n  \"IgnorePublicAcls\": true,\n  \"RestrictPublicBuckets\": false\n}";

/// 메뉴별 도움말.
fn help_text(menu: MenuList) -> String {
    use MenuList::*;
    let bucket_only = |flag: &str| {
        [
            usage::main_flag(flag, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat()
    };
    let with_file = |flag: &str, summary: &str, title: &str, example: &str| {
        [
            usage::main_flag(flag, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::FILE, "string", summary),
            format!("\n{title}\n"),
            example.to_string(),
        ]
        .concat()
    };
    match menu {
        GetPublicAccessBlock => bucket_only(usage::GET_PUBLIC_ACCESS_BLOCK),
        DeletePublicAccessBlock => bucket_only(usage::DELETE_PUBLIC_ACCESS_BLOCK),
        GetBucketPolicy => bucket_only(usage::GET_BUCKET_POLICY),
        DeleteBucketPolicy => bucket_only(usage::DELETE_BUCKET_POLICY),
        GetBucketPolicyStatus => bucket_only(usage::GET_BUCKET_POLICY_STATUS),
        GetBucketTagging => bucket_only(usage::GET_BUCKET_TAGGING),
        DeleteBucketTagging => bucket_only(usage::DELETE_BUCKET_TAGGING),
        GetBucketLifecycle => bucket_only(usage::GET_BUCKET_LIFECYCLE),
        DeleteBucketLifecycle => bucket_only(usage::DELETE_BUCKET_LIFECYCLE),
        GetBucketReplication => bucket_only(usage::GET_BUCKET_REPLICATION),
        DeleteBucketReplication => bucket_only(usage::DELETE_BUCKET_REPLICATION),
        PutPublicAccessBlock => with_file(
            usage::PUT_PUBLIC_ACCESS_BLOCK,
            " : PublicAccessBlock 설정 파일 경로",
            "PublicAccessBlock",
            PUBLIC_ACCESS_BLOCK_EXAMPLE,
        ),
        PutBucketPolicy => with_file(
            usage::PUT_BUCKET_POLICY,
            " : 정책 파일 경로",
            "Policy",
            // 원본은 `Dictionary.ToString()`이라 형식 이름이 나온다.
            "System.Collections.Generic.Dictionary`2[System.String,System.Object]",
        ),
        PutBucketTagging => with_file(
            usage::PUT_BUCKET_TAGGING,
            " : 태깅 설정 파일 경로",
            "Tagging",
            TAGGING_EXAMPLE,
        ),
        PutBucketLifecycle => with_file(
            usage::PUT_BUCKET_LIFECYCLE,
            " : 라이프사이클 설정 파일 경로",
            "LifeCycle",
            LIFECYCLE_EXAMPLE,
        ),
        PutBucketReplication => with_file(
            usage::PUT_BUCKET_REPLICATION,
            " : 복제 설정 파일 경로",
            "Replication",
            REPLICATION_EXAMPLE,
        ),
        _ => String::new(),
    }
}

/// Get 응답: .NET은 2xx(200이 아닌 `NoContent` 등)를 상태만 다른 정상 응답으로 돌려주고, `not_found_ok`인 연산
/// (수명주기, 정책, 복제)은 404도 그렇다. SDK가 이를 오류로 돌려주면 상태만 남긴다.
fn tolerate<T>(
    result: Result<S3Response<T>, S3Error>,
    not_found_ok: bool,
) -> Result<(u16, Option<T>), CommandError> {
    match result {
        Ok(response) => Ok((response.status, Some(response.output))),
        Err(S3Error::Service { status, .. })
            if (200..300).contains(&status) || (not_found_ok && status == 404) =>
        {
            Ok((status, Option::None))
        }
        Err(error) => Err(CommandError::from(error)),
    }
}

use super::input::null_reference;

use super::input::read_all_text;

/// `JsonSerializer.Deserialize<T>(text)`. JSON `null`이면 `None`.
fn read_json<T: FromJson>(text: &str) -> Result<Option<T>, CommandError> {
    deserialize::<T>(text, ReadOptions::default())
        .map_err(|e| CommandError::new("System.Text.Json.JsonException", e.0))
}

/// 응답 상태가 기대와 같으면 완료 로그, 아니면 실패 로그.
fn report(ok: bool, bucket: &str, ok_text: &str, fail_text: &str, status: u16, millis: u128) {
    if ok {
        info!("{bucket} {ok_text}! complete time = {millis}ms");
    } else {
        error!("{bucket} {fail_text}({})", status_name(status));
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
    if blank(&ctx.options.bucket_name) {
        println!("{}", usage::ERROR_BUCKET);
        return Ok(0);
    }
    let bucket = ctx.options.bucket_name.clone().unwrap_or_default();
    let print = ctx.options.print;

    // Put 메뉴: 설정 파일 검증
    let setting = if matches!(
        menu,
        PutPublicAccessBlock
            | PutBucketPolicy
            | PutBucketTagging
            | PutBucketLifecycle
            | PutBucketReplication
    ) {
        if blank(&ctx.options.file_path) {
            println!("{}", usage::ERROR_CONFIG_PATH);
            return Ok(0);
        }
        let file_path = ctx.options.file_path.clone().unwrap_or_default();
        if !Path::new(&file_path).is_file() {
            println!("{}", usage::ERROR_FILE);
            return Ok(0);
        }
        Some(read_all_text(&file_path)?)
    } else {
        Option::None
    };
    let setting = setting.unwrap_or_default();
    let client = ctx.client();

    match menu {
        GetPublicAccessBlock => {
            info!("GetPublicAccessBlock Start");
            let sw = Instant::now();
            let (status, output) = tolerate(client.get_public_access_block(&bucket).await, false)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 && print {
                print_json(
                    &output
                        .as_ref()
                        .and_then(|o| o.public_access_block_configuration()),
                );
            }
            report(
                status == 200,
                &bucket,
                "Get Public Access Block",
                "Get Public Access Block failed",
                status,
                millis,
            );
        }
        PutPublicAccessBlock => {
            let config = read_json::<PublicAccessBlockInput>(&setting)?;
            info!("PutPublicAccessBlock Start");
            let sw = Instant::now();
            let response = client
                .put_public_access_block(&bucket, config.unwrap_or_default().to_sdk())
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 200,
                &bucket,
                "Put Public Access Block",
                "Put Public Access Block failed",
                response.status,
                millis,
            );
        }
        DeletePublicAccessBlock => {
            info!("DeletePublicAccessBlock Start");
            let sw = Instant::now();
            let response = client
                .delete_public_access_block(&bucket)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            // 원본 문구는 성공은 `AccessBlock`, 실패는 `Bucket AccessBlock`이다.
            report(
                response.status == 204,
                &bucket,
                "Delete bucket AccessBlock",
                "Delete Bucket AccessBlock failed",
                response.status,
                millis,
            );
        }
        GetBucketPolicy => {
            info!("GetBucketPolicy Start");
            let sw = Instant::now();
            let (status, output) = tolerate(client.get_bucket_policy(&bucket).await, true)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 && print {
                println!(
                    "{}",
                    output.as_ref().and_then(|o| o.policy()).unwrap_or_default()
                );
            }
            report(
                status == 200,
                &bucket,
                "Get Bucket Policy",
                "Get Bucket Policy failed",
                status,
                millis,
            );
        }
        PutBucketPolicy => {
            info!("PutBucketPolicy Start");
            let sw = Instant::now();
            let response = client
                .put_bucket_policy(&bucket, &setting)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 200,
                &bucket,
                "Put Bucket Policy",
                "Put Bucket Policy failed",
                response.status,
                millis,
            );
        }
        DeleteBucketPolicy => {
            info!("DeleteBucketPolicy Start");
            let sw = Instant::now();
            let response = client
                .delete_bucket_policy(&bucket)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 204,
                &bucket,
                "Delete Bucket Policy",
                "Delete Bucket Policy failed",
                response.status,
                millis,
            );
        }
        GetBucketPolicyStatus => {
            info!("GetBucketPolicyStatus Start");
            let sw = Instant::now();
            let (status, output) = tolerate(client.get_bucket_policy_status(&bucket).await, false)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 && print {
                print_json(&output.as_ref().and_then(|o| o.policy_status()));
            }
            report(
                status == 200,
                &bucket,
                "Get Bucket Policy Status",
                "Get Bucket Policy Status failed",
                status,
                millis,
            );
        }
        GetBucketTagging => {
            info!("GetBucketTagging Start");
            let sw = Instant::now();
            let (status, output) = tolerate(client.get_bucket_tagging(&bucket).await, false)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 && print {
                print_json(output.as_ref().map_or(&[][..], |o| o.tag_set()));
            }
            report(
                status == 200,
                &bucket,
                "Get Bucket Tagging",
                "Get Bucket Tagging failed",
                status,
                millis,
            );
        }
        PutBucketTagging => {
            let tagging = read_json::<TaggingInput>(&setting)?;
            info!("PutBucketTagging Start");
            let sw = Instant::now();
            let tag_set = tagging.ok_or_else(null_reference)?.tag_set()?;
            let response = client
                .put_bucket_tagging(&bucket, tag_set)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 200,
                &bucket,
                "Put Bucket Tagging",
                "Put Bucket Tagging failed",
                response.status,
                millis,
            );
        }
        DeleteBucketTagging => {
            info!("DeleteBucketTagging Start");
            let sw = Instant::now();
            let response = client
                .delete_bucket_tagging(&bucket)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 204,
                &bucket,
                "Delete Bucket Tagging",
                "Delete Bucket Tagging failed",
                response.status,
                millis,
            );
        }
        GetBucketLifecycle => {
            info!("GetBucketLifecycleConfiguration Start");
            let sw = Instant::now();
            let (status, output) =
                tolerate(client.get_lifecycle_configuration(&bucket).await, true)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 {
                let rules = output.as_ref().map(|o| o.rules()).unwrap_or_default();
                let configuration = MyLifecycleConfiguration::from_sdk(rules);
                if print {
                    println!("{}", to_dotnet_json(&configuration));
                }
            }
            report(
                status == 200,
                &bucket,
                "Get Bucket Lifecycle",
                "Get Bucket Lifecycle failed",
                status,
                millis,
            );
        }
        PutBucketLifecycle => {
            let configuration = read_json::<MyLifecycleConfiguration>(&setting)?;
            info!("PutLifecycleConfiguration Start");
            let sw = Instant::now();
            let rules = configuration.ok_or_else(null_reference)?.to_sdk()?;
            let lifecycle = BucketLifecycleConfiguration::builder()
                .set_rules(Some(rules))
                .build()
                .map_err(|e| {
                    CommandError::new("Amazon.Runtime.AmazonClientException", e.to_string())
                })?;
            let response = client
                .put_lifecycle_configuration(&bucket, lifecycle)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 200,
                &bucket,
                "Put Bucket Lifecycle",
                "Put Bucket Lifecycle failed",
                response.status,
                millis,
            );
        }
        DeleteBucketLifecycle => {
            info!("DeleteBucketLifecycle Start");
            let sw = Instant::now();
            let response = client
                .delete_lifecycle(&bucket)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 204,
                &bucket,
                "Delete Bucket Lifecycle",
                "Delete Bucket Lifecycle failed",
                response.status,
                millis,
            );
        }
        GetBucketReplication => {
            info!("GetBucketReplication Start");
            let sw = Instant::now();
            let (status, output) = tolerate(client.get_bucket_replication(&bucket).await, true)?;
            let millis = sw.elapsed().as_millis();
            if status == 200 && print {
                print_json(&output.as_ref().and_then(|o| o.replication_configuration()));
            }
            report(
                status == 200,
                &bucket,
                "Get Bucket Replication",
                "Get Bucket Replication failed",
                status,
                millis,
            );
        }
        PutBucketReplication => {
            let mut configuration = read_json::<MyReplicationConfiguration>(&setting)?;
            if let Some(target) = ctx.options.target.as_deref() {
                // 원본: `setting.Rules[i].Destination.Bucket.Replace("TargetBucketName", target)`
                let rules = configuration
                    .as_mut()
                    .and_then(|c| c.rules.as_mut())
                    .ok_or_else(null_reference)?;
                for rule in rules.iter_mut() {
                    let bucket_name = rule
                        .as_mut()
                        .and_then(|r| r.destination.as_mut())
                        .and_then(|d| d.bucket.as_mut())
                        .ok_or_else(null_reference)?;
                    *bucket_name = bucket_name.replace("TargetBucketName", target);
                }
            }
            info!("PutBucketReplication Start");
            let sw = Instant::now();
            let replication = configuration.ok_or_else(null_reference)?.to_sdk()?;
            let response = client
                .put_bucket_replication(&bucket, replication, Option::None)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 200,
                &bucket,
                "Put Bucket Replication",
                "Put Bucket Replication failed",
                response.status,
                millis,
            );
        }
        DeleteBucketReplication => {
            info!("DeleteBucketReplication Start");
            let sw = Instant::now();
            let response = client
                .delete_bucket_replication(&bucket)
                .await
                .map_err(CommandError::from)?;
            let millis = sw.elapsed().as_millis();
            report(
                response.status == 204,
                &bucket,
                "Delete Bucket Replication",
                "Delete Bucket Replication failed",
                response.status,
                millis,
            );
        }
        _ => return not_ported(menu),
    }
    Ok(0)
}
