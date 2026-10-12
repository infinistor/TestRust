//! TESTCore `Cli/*`: 명령행 옵션 모델(`CommandOptions`)과 Mono.Options 호환 파서.

mod convert;
mod definitions;
mod help;
mod parser;

use testrust_config::EnumBucketTypes;
use testrust_s3::ChecksumAlgorithm;

use crate::menu::MenuList;

pub use definitions::{Action, OPTIONS, OptionDef};
pub use help::write_option_descriptions;
pub use parser::{ParseResult, find_option, option_names, parse};

/// 원본 `CommandOptions`. 문자열 속성은 `null`이 될 수 있어 `Option`으로 둔다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOptions {
    pub help: bool,
    pub worker: bool,
    pub controller: bool,
    pub version: bool,
    pub config_path: Option<String>,
    pub bucket_name: Option<String>,
    pub source: Option<String>,
    pub target: Option<String>,
    pub key: Option<String>,
    pub source_key: Option<String>,
    pub file_path: Option<String>,
    pub path: Option<String>,
    pub tags: Option<String>,
    pub str_acl: Option<String>,
    pub versioning: Option<String>,
    pub version_id: Option<String>,
    pub storage_class: Option<String>,
    /// `ObjectOwnership`(문자열 상수 클래스). 원본은 아무 문자열이나 그대로 받는다.
    pub ownership: Option<String>,
    pub lock_mode: Option<String>,
    pub body: Option<String>,
    pub url: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub encryption_key: Option<String>,
    pub user_name: Option<String>,
    pub bucket_type: EnumBucketTypes,
    pub another: bool,
    pub print: bool,
    pub all: bool,
    pub check: bool,
    pub bypass: Option<bool>,
    pub flag: bool,
    pub random: bool,
    pub multipart: bool,
    pub debug: bool,
    pub checksum: bool,
    pub checksum_type: ChecksumAlgorithm,
    pub md5sum: bool,
    pub id: Option<String>,
    pub bulk: bool,
    pub thread_prefix: Option<String>,
    pub admin: bool,
    pub use_chunk_encoding: bool,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub delimiter: Option<String>,
    /// 원본 속성 이름 `Darker`(`--marker` 값).
    pub darker: Option<String>,
    pub continuation_token: Option<String>,
    pub max_keys: i32,
    pub days: i32,
    pub years: i32,
    pub date: Option<String>,
    pub upload_id: Option<String>,
    pub part_number: i32,
    pub part_size: i64,
    pub start_byte: i64,
    pub end_byte: i64,
    pub file_size: i64,
    pub range_list: Vec<i64>,
    pub start_count: i32,
    pub thread: i32,
    pub count: i32,
    pub times: i32,
    pub read: i32,
    pub write: i32,
    pub delete: i32,
    pub save: Option<String>,
    pub service_type: Option<String>,
    pub address: Option<String>,
    pub port: i32,
    pub target_path: Option<String>,
    pub menu: MenuList,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            help: false,
            worker: false,
            controller: false,
            version: false,
            config_path: Some("config.ini".to_string()),
            bucket_name: None,
            source: None,
            target: None,
            key: None,
            source_key: None,
            file_path: None,
            path: None,
            tags: None,
            str_acl: None,
            versioning: None,
            version_id: None,
            storage_class: None,
            ownership: None,
            lock_mode: None,
            body: None,
            url: None,
            access_key: None,
            secret_key: None,
            encryption_key: None,
            user_name: None,
            bucket_type: EnumBucketTypes::Empty,
            another: false,
            print: true,
            all: false,
            check: false,
            bypass: None,
            flag: false,
            random: false,
            multipart: false,
            debug: false,
            checksum: false,
            checksum_type: ChecksumAlgorithm::None,
            md5sum: false,
            id: None,
            bulk: false,
            thread_prefix: None,
            admin: false,
            use_chunk_encoding: false,
            prefix: None,
            suffix: None,
            delimiter: None,
            darker: None,
            continuation_token: None,
            max_keys: 1000,
            days: -1,
            years: -1,
            date: None,
            upload_id: None,
            part_number: 0,
            part_size: 10_485_760,
            start_byte: -1,
            end_byte: -1,
            file_size: -1,
            range_list: Vec::new(),
            start_count: 0,
            thread: 0,
            count: 0,
            times: 0,
            read: -1,
            write: -1,
            delete: -1,
            save: None,
            service_type: None,
            address: None,
            port: 5555,
            target_path: None,
            menu: MenuList::None,
        }
    }
}

/// 파싱 중 발생한 예외.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Mono.Options `OptionException`. `option_name`은 원본 `OptionName`(묶음 오류는 `null`).
    #[error("{message}")]
    Option {
        message: String,
        option_name: Option<String>,
    },
    /// 옵션 동작에서 난 그 밖의 예외(원본은 잡지 않아 프로세스가 비정상 종료한다).
    #[error("{message}")]
    Unhandled {
        dotnet_type: &'static str,
        message: String,
    },
}

impl ParseError {
    /// .NET 예외 형식 이름.
    pub fn dotnet_type(&self) -> &'static str {
        match self {
            Self::Option { .. } => "Mono.Options.OptionException",
            Self::Unhandled { dotnet_type, .. } => dotnet_type,
        }
    }

    pub(crate) fn option(message: String, option_name: impl Into<Option<String>>) -> Self {
        Self::Option {
            message,
            option_name: option_name.into(),
        }
    }
}
