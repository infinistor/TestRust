//! TESTCore `Util/ConfigBootstrapper.cs`: 설정 파일을 읽고 명령행 값으로 덮어쓴다.

use testrust_common::to_dotnet_json;
use testrust_config::{Config, EnumBucketTypes};
use tracing::info;

use crate::options::CommandOptions;

/// 원본 `ConfigBootstrapper.Load`. 실패하면 `None`(원본은 `(null, false)`).
/// 버킷 이름을 주지 않았으면 설정의 버킷 이름을 `options`에 채운다.
pub fn load(options: &mut CommandOptions) -> Option<Config> {
    let mut config = Config::load(
        options.config_path.as_deref().unwrap_or(""),
        options.user_name.as_deref(),
    )
    .ok()?;

    if options.admin {
        config.main.set_is_admin(true);
    }
    match &options.bucket_name {
        Some(bucket_name) => config.main.set_bucket_name(bucket_name.clone()),
        None => options.bucket_name = Some(config.main.bucket_name.clone()),
    }
    if let Some(thread_prefix) = &options.thread_prefix {
        config.main.set_thread_prefix(thread_prefix.clone());
    }
    if let Some(prefix) = &options.prefix {
        config.main.set_object_prefix(prefix.clone());
    }
    if options.file_size >= 0 {
        config.main.set_file_size(options.file_size);
    }
    if let Some(path) = &options.path {
        config.main.set_file_path(path.clone());
    }
    if let Some(target_path) = &options.target_path {
        config.main.set_target_path(target_path.clone());
    }
    if options.bucket_type != EnumBucketTypes::Empty {
        config.up_down.set_bucket_type(options.bucket_type);
    }
    if options.thread > 0 {
        config.up_down.set_thread_count(options.thread);
    }
    if options.count > 0 {
        config.up_down.set_file_count(options.count);
    }
    if options.times > 0 {
        config.up_down.set_times(options.times);
    }
    if options.read >= 0 {
        config.up_down.set_read(options.read);
    }
    if options.write >= 0 {
        config.up_down.set_write(options.write);
    }
    if options.delete >= 0 {
        config.up_down.set_delete(options.delete);
    }
    if options.md5sum {
        config.up_down.set_etag_check(true);
    }
    if let Some(save) = non_blank(&options.save) {
        config.up_down.set_save(save);
    }
    if options.use_chunk_encoding {
        config.up_down.set_use_chunk_encoding(true);
    }
    if let Some(url) = non_blank(&options.url) {
        config.main_user.set_url(url);
    }
    if let Some(access_key) = non_blank(&options.access_key) {
        config.main_user.set_access_key(access_key);
    }
    if let Some(secret_key) = non_blank(&options.secret_key) {
        config.main_user.set_secret_key(secret_key);
    }
    if options.debug {
        info!("{}", to_dotnet_json(&config.main_user));
        if options.another {
            info!("{}", to_dotnet_json(&config.alt_user));
        }
    }
    Some(config)
}

/// `!string.IsNullOrWhiteSpace(value)`인 값.
fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.trim().is_empty())
}
