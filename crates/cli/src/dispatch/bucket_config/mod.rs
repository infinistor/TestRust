//! 원본 `CommandDispatcher` 중 버킷 분석·CORS·암호화·인벤토리·메트릭·로깅·알림·웹사이트 설정.
//!
//! 입력 설정 파일(`--file`)은 `JsonSerializer.Deserialize<T>(File.ReadAllText(path))`로 읽는다. 분석·인벤토리·메트릭·
//! 알림은 `Data/S3/My*` DTO로, CORS·암호화·로깅·웹사이트는 AWSSDK 모델 그대로 읽는다(하위 모듈).
//!
//! 원본 동작 그대로 둔 것(자세한 것은 각 하위 모듈)
//! - `GetBucketInventory`의 도움말은 `--get-bucket-encryption`을 이름으로 쓴다.
//! - 분석·인벤토리·메트릭에서 `Id`가 비어 있으면 SDK가 요청 전에 `AmazonS3Exception`을 던진다.
//! - 분석·메트릭 필터의 `Tag`가 둘 이하(`Prefix` 없음)이면 첫 태그만 쓴다.
//! - `ListBucketInventory`는 응답에 구성이 없으면(목록이 `null`) 결과 개수를 읽다가 `NullReferenceException`이다.
//! - 알림 저장·조회 로그의 시작 줄은 `PutBucketNotificationConfiguration Start`,
//!   `GetBucketNotificationConfiguration Start`이다.
//!
//! 어긋나는 점: SDK 모델을 직접 읽는 CORS·암호화·웹사이트에서 파일 내용이 JSON `null`이면 원본은 본문 없는 요청을
//! 보내지만 여기서는 빈 설정 본문을 보낸다(SDK가 본문 없는 요청을 만들 수 없다).

mod analytics;
mod cors;
mod encryption;
mod filter;
mod help_examples;
mod inventory;
mod jsonutil;
mod logging;
mod metrics;
mod notification;
mod website;

use super::input::blank;
use std::time::Instant;

use aws_sdk_s3::error::BuildError;
use aws_sdk_s3::types::{BucketLoggingStatus, CorsConfiguration, WebsiteConfiguration};
use testrust_common::dotnet_http::status_name;
use testrust_common::json::FromJson;
use testrust_s3::{S3Error, S3Response};
use tracing::{error, info};

use super::output::print_json;
use super::{CommandContext, CommandError, CommandResult};
use crate::menu::MenuList;
use crate::usage;
use analytics::MyAnalyticsConfiguration;
use cors::CorsConfigurationInput;
use encryption::SseConfigurationInput;
use inventory::MyInventoryConfiguration;
use jsonutil::parse;
use logging::LoggingConfigInput;
use metrics::MyMetricsConfiguration;
use notification::MyNotificationConfiguration;
use website::WebsiteConfigurationInput;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::GetBucketAnalytics,
    MenuList::PutBucketAnalytics,
    MenuList::DeleteBucketAnalytics,
    MenuList::ListBucketAnalytics,
    MenuList::GetBucketCors,
    MenuList::PutBucketCors,
    MenuList::DeleteBucketCors,
    MenuList::GetBucketEncryption,
    MenuList::PutBucketEncryption,
    MenuList::DeleteBucketEncryption,
    MenuList::GetBucketInventory,
    MenuList::PutBucketInventory,
    MenuList::DeleteBucketInventory,
    MenuList::ListBucketInventory,
    MenuList::GetBucketMetrics,
    MenuList::PutBucketMetrics,
    MenuList::DeleteBucketMetrics,
    MenuList::ListBucketMetrics,
    MenuList::GetBucketLogging,
    MenuList::PutBucketLogging,
    MenuList::GetBucketNotification,
    MenuList::PutBucketNotification,
    MenuList::GetBucketWebsite,
    MenuList::PutBucketWebsite,
    MenuList::DeleteBucketWebsite,
];

use crate::dispatch::input::null_reference;

/// SDK 빌더의 `build()`. 필수 값은 항상 채우므로(없는 값은 `UNSET` 표식) 실패하지 않는다.
fn built<T>(result: Result<T, BuildError>) -> T {
    result.expect("필수 값을 모두 채웠다")
}

/// S3 호출 결과의 오류를 명령 오류로 바꾼다. 이 메뉴들의 SDK 연산에는 별도 예외 형식이 모델링돼 있지 않아
/// 서버 오류는 오류 코드와 상관없이 모두 `AmazonS3Exception`이다(`NoSuchBucket`도 마찬가지).
fn api<T>(result: Result<S3Response<T>, S3Error>) -> Result<S3Response<T>, CommandError> {
    result.map_err(CommandError::from)
}

/// 상태 404를 예외 대신 응답으로 돌려주는 조회. 분석·암호화·인벤토리·메트릭·로깅·웹사이트 조회는 .NET SDK가
/// 404에서 예외를 던지지 않고 `HttpStatusCode.NotFound` 응답을 돌려주므로 `None`으로 알린다.
fn api_or_not_found<T>(
    result: Result<S3Response<T>, S3Error>,
) -> Result<Option<S3Response<T>>, CommandError> {
    match result {
        Err(S3Error::Service { status: 404, .. }) => Ok(None),
        other => api(other).map(Some),
    }
}

/// 설정 객체의 아이디 속성 검사. .NET SDK는 값이 `null`이거나 비어 있으면 요청을 만들기 전에
/// `AmazonS3Exception`을 던진다.
fn required_id(id: Option<&str>, property: &str) -> Result<String, CommandError> {
    match id {
        Some(id) if !id.is_empty() => Ok(id.to_string()),
        _ => Err(CommandError::new(
            "Amazon.S3.AmazonS3Exception",
            format!("Request object does not have required field {property} set"),
        )),
    }
}

/// 메뉴별 도움말. 이 모듈의 메뉴가 아니면 `None`.
fn help_text(menu: MenuList) -> Option<String> {
    use MenuList::*;
    use help_examples::*;
    let put = |main: &str, id: bool, summary: &str, title: &str, example: &[&str]| {
        [
            usage::main_flag(main, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            if id {
                usage::sub_flag(usage::ID, "string", "")
            } else {
                String::new()
            },
            usage::sub_flag(usage::FILE, "string", summary),
            format!("\n{title}\n{}", example.join("\n")),
        ]
        .concat()
    };
    Some(match menu {
        DeleteBucketAnalytics => [
            usage::main_flag(usage::DELETE_BUCKET_ANALYTICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", ""),
        ]
        .concat(),
        DeleteBucketCors => [
            usage::main_flag(usage::DELETE_BUCKET_CORS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        DeleteBucketEncryption => [
            usage::main_flag(usage::DELETE_BUCKET_ENCRYPTION, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        DeleteBucketInventory => [
            usage::main_flag(usage::DELETE_BUCKET_INVENTORY, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", ""),
        ]
        .concat(),
        DeleteBucketMetrics => [
            usage::main_flag(usage::DELETE_BUCKET_METRICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", ""),
        ]
        .concat(),
        DeleteBucketWebsite => [
            usage::main_flag(usage::DELETE_BUCKET_WEBSITE, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        GetBucketAnalytics => [
            usage::main_flag(usage::GET_BUCKET_ANALYTICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", " : 분석 구성 식별자"),
        ]
        .concat(),
        GetBucketCors => [
            usage::main_flag(usage::GET_BUCKET_CORS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        GetBucketEncryption => [
            usage::main_flag(usage::GET_BUCKET_ENCRYPTION, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        // 원본 버그: 이름이 `--get-bucket-encryption`이다.
        GetBucketInventory => [
            usage::main_flag(usage::GET_BUCKET_ENCRYPTION, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", " : 인벤토리 구성 식별자"),
        ]
        .concat(),
        GetBucketMetrics => [
            usage::main_flag(usage::GET_BUCKET_METRICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::ID, "string", " : 메트릭 구성 식별자"),
        ]
        .concat(),
        GetBucketLogging => [
            usage::main_flag(usage::GET_BUCKET_LOGGING, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        GetBucketNotification => [
            usage::main_flag(usage::GET_BUCKET_NOTIFICATION, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        GetBucketWebsite => [
            usage::main_flag(usage::GET_BUCKET_WEBSITE, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        ListBucketAnalytics => [
            usage::main_flag(usage::LIST_BUCKET_ANALYTICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        ListBucketInventory => [
            usage::main_flag(usage::LIST_BUCKET_INVENTORY, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        ListBucketMetrics => [
            usage::main_flag(usage::LIST_BUCKET_METRICS, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        PutBucketAnalytics => put(
            usage::PUT_BUCKET_ANALYTICS,
            false,
            " : 분석 설정 파일 경로",
            "AnalyticsConfig",
            PUT_BUCKET_ANALYTICS_EXAMPLE,
        ),
        PutBucketCors => put(
            usage::PUT_BUCKET_CORS,
            false,
            " : CORS 설정 파일 경로",
            "CORSConfig",
            PUT_BUCKET_CORS_EXAMPLE,
        ),
        PutBucketEncryption => put(
            usage::PUT_BUCKET_ENCRYPTION,
            false,
            " : 암호화 설정 파일 경로",
            "SSEConfig",
            PUT_BUCKET_ENCRYPTION_EXAMPLE,
        ),
        PutBucketInventory => put(
            usage::PUT_BUCKET_INVENTORY,
            true,
            " : 인벤토리 설정 파일 경로",
            "InventoryConfig",
            PUT_BUCKET_INVENTORY_EXAMPLE,
        ),
        PutBucketLogging => put(
            usage::PUT_BUCKET_LOGGING,
            false,
            " : 로깅 설정 파일 경로",
            "Logging",
            PUT_BUCKET_LOGGING_EXAMPLE,
        ),
        PutBucketMetrics => put(
            usage::PUT_BUCKET_METRICS,
            false,
            " : 메트릭스 설정 파일 경로",
            "MetricsConfig",
            PUT_BUCKET_METRICS_EXAMPLE,
        ),
        PutBucketNotification => put(
            usage::PUT_BUCKET_NOTIFICATION,
            false,
            " : 알림 설정 파일 경로",
            "Notification",
            PUT_BUCKET_NOTIFICATION_EXAMPLE,
        ),
        PutBucketWebsite => put(
            usage::PUT_BUCKET_WEBSITE,
            false,
            " : 웹사이트 설정 파일 경로",
            "Website",
            PUT_BUCKET_WEBSITE_EXAMPLE,
        ),
        _ => return Option::None,
    })
}

/// 아이디(`--id`)가 필요한 메뉴.
fn needs_id(menu: MenuList) -> bool {
    use MenuList::*;
    matches!(
        menu,
        GetBucketAnalytics
            | DeleteBucketAnalytics
            | GetBucketInventory
            | DeleteBucketInventory
            | GetBucketMetrics
            | DeleteBucketMetrics
    )
}

/// 설정 파일(`--file`)을 읽는 메뉴.
fn needs_file(menu: MenuList) -> bool {
    use MenuList::*;
    matches!(
        menu,
        PutBucketAnalytics
            | PutBucketCors
            | PutBucketEncryption
            | PutBucketInventory
            | PutBucketLogging
            | PutBucketMetrics
            | PutBucketNotification
            | PutBucketWebsite
    )
}

use super::input::read_all_text;

/// `--file`의 JSON을 `T`로 읽는다(`JsonSerializer.Deserialize<T>(File.ReadAllText(filePath))`). `null`이면 `None`.
fn load<T: FromJson>(path: &str) -> Result<Option<T>, CommandError> {
    parse(&read_all_text(path)?)
}

/// `Console.WriteLine(JsonSerializer.Serialize(list))`. SDK 컬렉션은 항목이 없으면 `null`이다.
fn print_list<T: std::fmt::Debug>(items: &[T]) {
    if items.is_empty() {
        println!("null");
    } else {
        print_json(items);
    }
}

/// 성공 로그(`{bucket} {label}! complete time = {ms}ms`).
fn log_complete(bucket: &str, label: &str, started: Instant) {
    info!(
        "{bucket} {label}! complete time = {}ms",
        started.elapsed().as_millis()
    );
}

/// 실패 로그(`{bucket} {label} failed({status})`).
fn log_failed(bucket: &str, label: &str, status: u16) {
    error!("{bucket} {label} failed({})", status_name(status));
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let Some(help) = help_text(menu) else {
        return Ok(0);
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
    if needs_id(menu) && blank(&o.id) {
        println!("{}", usage::ERROR_ID);
        return Ok(0);
    }
    if needs_file(menu) {
        if blank(&o.file_path) {
            println!("{}", usage::ERROR_CONFIG_PATH);
            return Ok(0);
        }
        if !std::path::Path::new(o.file_path.as_deref().unwrap_or_default()).is_file() {
            println!("{}", usage::ERROR_FILE);
            return Ok(0);
        }
    }
    let bucket = o.bucket_name.clone().unwrap_or_default();
    let bucket = bucket.as_str();
    let id = o.id.clone().unwrap_or_default();
    let id = id.as_str();
    let file = o.file_path.clone().unwrap_or_default();
    let file = file.as_str();
    let print = o.print;
    let client = ctx.client();

    match menu {
        // ---------------------------------------------------------------- Delete
        DeleteBucketAnalytics
        | DeleteBucketInventory
        | DeleteBucketMetrics
        | DeleteBucketCors
        | DeleteBucketEncryption
        | DeleteBucketWebsite => {
            let (start, label) = match menu {
                DeleteBucketAnalytics => ("DeleteBucketAnalytics Start", "Delete Bucket Analytics"),
                DeleteBucketInventory => ("DeleteBucketInventory Start", "Delete Bucket Inventory"),
                DeleteBucketMetrics => ("DeleteBucketMetrics Start", "Delete Bucket Metrics"),
                DeleteBucketCors => ("DeleteBucketCors Start", "Delete Bucket CORS"),
                DeleteBucketEncryption => {
                    ("DeleteBucketEncryption Start", "Delete Bucket Encryption")
                }
                _ => ("DeleteBucketWebsite Start", "Delete Bucket Website"),
            };
            info!("{start}");
            let started = Instant::now();
            let status = match menu {
                DeleteBucketAnalytics => {
                    api(client.delete_bucket_analytics(bucket, id).await)?.status
                }
                DeleteBucketInventory => {
                    api(client.delete_bucket_inventory(bucket, id).await)?.status
                }
                DeleteBucketMetrics => api(client.delete_bucket_metrics(bucket, id).await)?.status,
                DeleteBucketCors => api(client.delete_cors(bucket).await)?.status,
                DeleteBucketEncryption => {
                    api(client.delete_bucket_encryption(bucket).await)?.status
                }
                _ => api(client.delete_bucket_website(bucket).await)?.status,
            };
            if status == 204 {
                log_complete(bucket, label, started);
            } else {
                log_failed(bucket, label, status);
            }
        }
        // ---------------------------------------------------------------- Get
        GetBucketAnalytics => {
            info!("GetBucketAnalytics Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_analytics(bucket, id).await)?
            else {
                log_failed(bucket, "Get Bucket Analytics", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    print_json(&response.output.analytics_configuration());
                }
                log_complete(bucket, "Get Bucket Analytics", started);
            } else {
                log_failed(bucket, "Get Bucket Analytics", response.status);
            }
        }
        GetBucketCors => {
            info!("GetBucketCors Start");
            let started = Instant::now();
            let response = api(client.get_cors(bucket).await)?;
            if response.status == 200 {
                if print {
                    let rules = response.output.cors_rules().to_vec();
                    print_json(
                        &CorsConfiguration::builder()
                            .set_cors_rules(Some(rules))
                            .build()
                            .ok(),
                    );
                }
                log_complete(bucket, "Get Bucket CORS", started);
            } else {
                log_failed(bucket, "Get Bucket CORS", response.status);
            }
        }
        GetBucketEncryption => {
            info!("GetBucketEncryption Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_encryption(bucket).await)?
            else {
                log_failed(bucket, "Get Bucket Encryption", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    print_json(&response.output.server_side_encryption_configuration());
                }
                log_complete(bucket, "Get Bucket Encryption", started);
            } else {
                log_failed(bucket, "Get Bucket Encryption", response.status);
            }
        }
        GetBucketInventory => {
            info!("GetBucketInventory Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_inventory(bucket, id).await)?
            else {
                log_failed(bucket, "Get Bucket Inventory", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    print_json(&response.output.inventory_configuration());
                }
                log_complete(bucket, "Get Bucket Inventory", started);
            } else {
                log_failed(bucket, "Get Bucket Inventory", response.status);
            }
        }
        GetBucketMetrics => {
            info!("GetBucketMetrics Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_metrics(bucket, id).await)?
            else {
                log_failed(bucket, "Get Bucket Metrics", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    print_json(&response.output.metrics_configuration());
                }
                log_complete(bucket, "Get Bucket Metrics", started);
            } else {
                log_failed(bucket, "Get Bucket Metrics", response.status);
            }
        }
        GetBucketLogging => {
            info!("GetBucketLogging Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_logging(bucket).await)? else {
                log_failed(bucket, "Get Bucket Logging", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    let status = BucketLoggingStatus::builder()
                        .set_logging_enabled(response.output.logging_enabled().cloned())
                        .build();
                    print_json(&status);
                }
                log_complete(bucket, "Get Bucket Logging", started);
            } else {
                log_failed(bucket, "Get Bucket Logging", response.status);
            }
        }
        GetBucketNotification => {
            info!("GetBucketNotificationConfiguration Start");
            let started = Instant::now();
            let response = api(client.get_bucket_notification(bucket).await)?;
            if response.status == 200 {
                if print {
                    print_list(response.output.lambda_function_configurations());
                    print_list(response.output.queue_configurations());
                    print_list(response.output.topic_configurations());
                }
                log_complete(bucket, "Get Bucket Notification", started);
            } else {
                log_failed(bucket, "Get Bucket Notification", response.status);
            }
        }
        GetBucketWebsite => {
            info!("GetBucketWebsite Start");
            let started = Instant::now();
            let Some(response) = api_or_not_found(client.get_bucket_website(bucket).await)? else {
                log_failed(bucket, "Get Bucket Website", 404);
                return Ok(0);
            };
            if response.status == 200 {
                if print {
                    let output = &response.output;
                    let configuration = WebsiteConfiguration::builder()
                        .set_error_document(output.error_document().cloned())
                        .set_index_document(output.index_document().cloned())
                        .set_redirect_all_requests_to(output.redirect_all_requests_to().cloned())
                        .set_routing_rules(Some(output.routing_rules().to_vec()))
                        .build();
                    print_json(&configuration);
                }
                log_complete(bucket, "Get Bucket Website", started);
            } else {
                log_failed(bucket, "Get Bucket Website", response.status);
            }
        }
        // ---------------------------------------------------------------- List
        ListBucketAnalytics => {
            info!("ListBucketAnalytics Start");
            let started = Instant::now();
            let response = api(client.list_bucket_analytics(bucket).await)?;
            if response.status == 200 {
                if print {
                    print_list(response.output.analytics_configuration_list());
                }
                log_complete(bucket, "List Bucket Analytics", started);
            } else {
                log_failed(bucket, "List Bucket Analytics", response.status);
            }
        }
        ListBucketInventory => {
            info!("ListBucketInventory Start");
            let started = Instant::now();
            let response = api(client.list_bucket_inventory(bucket).await)?;
            if response.status == 200 {
                let list = response.output.inventory_configuration_list();
                if print {
                    print_list(list);
                }
                // 응답에 구성이 없으면 목록이 `null`이라 `.Count`가 `NullReferenceException`이다.
                if list.is_empty() {
                    return Err(null_reference());
                }
                info!(
                    "List Bucket Inventory({})! complete time = {}ms",
                    list.len(),
                    started.elapsed().as_millis()
                );
            } else {
                error!(
                    "List Bucket Inventory failed({})",
                    status_name(response.status)
                );
            }
        }
        ListBucketMetrics => {
            info!("ListBucketMetrics Start");
            let started = Instant::now();
            let response = api(client.list_bucket_metrics(bucket).await)?;
            if response.status == 200 {
                if print {
                    print_list(response.output.metrics_configuration_list());
                }
                log_complete(bucket, "List Bucket Metrics", started);
            } else {
                log_failed(bucket, "List Bucket Metrics", response.status);
            }
        }
        // ---------------------------------------------------------------- Put
        PutBucketAnalytics => {
            let setting = load::<MyAnalyticsConfiguration>(file)?;
            info!("PutBucketAnalytics Start");
            let started = Instant::now();
            let configuration = setting
                .ok_or_else(null_reference)?
                .analytics_configuration()?;
            let status = api(client.put_bucket_analytics(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Analytics", started);
            } else {
                log_failed(bucket, "Put Bucket Analytics", status);
            }
        }
        PutBucketCors => {
            let setting = load::<CorsConfigurationInput>(file)?;
            info!("PutBucketCors Start");
            let started = Instant::now();
            let configuration = setting.unwrap_or_default().to_sdk();
            let status = api(client.put_cors(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket CORS", started);
            } else {
                log_failed(bucket, "Put Bucket CORS", status);
            }
        }
        PutBucketEncryption => {
            let setting = load::<SseConfigurationInput>(file)?;
            info!("PutBucketEncryption Start");
            let started = Instant::now();
            let configuration = setting.unwrap_or_default().to_sdk()?;
            let status = api(client.put_bucket_encryption(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Encryption", started);
            } else {
                log_failed(bucket, "Put Bucket Encryption", status);
            }
        }
        PutBucketInventory => {
            let setting = load::<MyInventoryConfiguration>(file)?;
            info!("PutBucketInventory Start");
            let started = Instant::now();
            let configuration = setting
                .ok_or_else(null_reference)?
                .inventory_configuration()?;
            let status = api(client.put_bucket_inventory(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Inventory", started);
            } else {
                log_failed(bucket, "Put Bucket Inventory", status);
            }
        }
        PutBucketLogging => {
            let setting = load::<LoggingConfigInput>(file)?;
            info!("PutBucketLogging Start");
            let started = Instant::now();
            let configuration = setting.unwrap_or_default().to_sdk()?;
            let status = api(client.put_bucket_logging(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Logging", started);
            } else {
                log_failed(bucket, "Put Bucket Logging", status);
            }
        }
        PutBucketMetrics => {
            let setting = load::<MyMetricsConfiguration>(file)?;
            info!("PutBucketMetrics Start");
            let started = Instant::now();
            let configuration = setting
                .ok_or_else(null_reference)?
                .metrics_configuration()?;
            let status = api(client.put_bucket_metrics(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Metrics", started);
            } else {
                log_failed(bucket, "Put Bucket Metrics", status);
            }
        }
        PutBucketNotification => {
            let setting = load::<MyNotificationConfiguration>(file)?;
            info!("PutBucketNotificationConfiguration Start");
            let started = Instant::now();
            let setting = setting.ok_or_else(null_reference)?;
            let topics = setting.topic_configurations()?;
            let queues = setting.queue_configurations()?;
            let lambdas = setting.lambda_function_configurations()?;
            let status = api(client
                .put_bucket_notification(bucket, Some(topics), Some(queues), Some(lambdas))
                .await)?
            .status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Notification", started);
            } else {
                log_failed(bucket, "Put Bucket Notification", status);
            }
        }
        PutBucketWebsite => {
            let setting = load::<WebsiteConfigurationInput>(file)?;
            info!("PutBucketWebsite Start");
            let started = Instant::now();
            let configuration = setting.unwrap_or_default().to_sdk();
            let status = api(client.put_bucket_website(bucket, configuration).await)?.status;
            if status == 200 {
                log_complete(bucket, "Put Bucket Website", started);
            } else {
                log_failed(bucket, "Put Bucket Website", status);
            }
        }
        _ => {}
    }
    Ok(0)
}
