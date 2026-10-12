//! TESTCore `Commands/CommandDispatcher.cs`: 파싱된 `MenuList`에 해당하는 명령을 실행한다.
//!
//! 원본은 메서드 하나의 `switch`지만 기능 묶음별 모듈로 나눴다. 각 모듈은 `run(ctx, menu)`과
//! 옮긴 메뉴 목록 `PORTED`를 둔다. 메뉴 → 모듈 배정은 [`execute`]의 `match`가 정한다.
//!
//! 원본 지역 변수(`bucketName`, `key` 등)는 `ctx.options`의 같은 이름 필드로 쓴다.
//! 응답 상태가 기대와 다르면 원본처럼 `_log.Error`만 남기고 종료 코드는 0이다. 예외(S3 오류 등)는
//! `CommandError`로 돌려주고, 최상위(`app.rs`)가 `ERROR` 로그를 남긴 뒤 -1로 끝낸다.

mod acl;
mod backend;
mod bucket;
mod bucket_config;
mod bucket_rules;
pub use testrust_scenarios::input;
mod ksan;
mod multipart;
mod object;
pub mod output;
mod tests;
mod util;

use testrust_config::Config;
use testrust_s3::S3Client;
use tokio_util::sync::CancellationToken;

use crate::menu::MenuList;
use crate::options::CommandOptions;

/// 원본 `ERROR_NORMAL`.
pub const ERROR_NORMAL: i32 = -1;

/// 원본 `CommandExecutionContext`.
pub struct CommandContext {
    /// 명령행 값이 반영된 설정. 도움말만 출력할 때는 설정 파일이 없어도 되므로 `None`일 수 있다.
    pub config: Option<Config>,
    pub options: CommandOptions,
    /// S3 클라이언트. 도움말 실행에서는 `None`.
    pub client: Option<S3Client>,
    /// 프로세스 전체 취소 토큰. 시나리오가 Ctrl+C 처리기를 등록하면(`scenarios::shutdown`) 이 토큰이 취소된다.
    pub cancel: CancellationToken,
}

impl CommandContext {
    /// 설정(도움말이 아닌 실행에서는 항상 있다).
    pub fn config(&self) -> &Config {
        self.config.as_ref().expect("설정이 로드되지 않았다")
    }

    pub fn config_mut(&mut self) -> &mut Config {
        self.config.as_mut().expect("설정이 로드되지 않았다")
    }

    /// S3 클라이언트(도움말이 아닌 실행에서는 항상 있다).
    pub fn client(&self) -> &S3Client {
        self.client.as_ref().expect("S3 클라이언트가 없다")
    }

    /// 원본 `var acl = string.IsNullOrWhiteSpace(strACL) ? null : S3CannedACL.FindValue(strACL);`
    pub fn acl(&self) -> Option<&str> {
        self.options
            .str_acl
            .as_deref()
            .filter(|acl| !acl.trim().is_empty())
    }
}

/// 명령 실행 중 난 예외. 원본 최상위의 `catch (Exception e) { _log.Error(e); return ERROR_NORMAL; }`로 간다.
pub use testrust_scenarios::ScenarioError as CommandError;

pub type CommandResult = Result<i32, CommandError>;

/// 아직 옮기지 않은 메뉴.
fn not_ported(menu: MenuList) -> CommandResult {
    println!("아직 이식되지 않은 명령입니다: {menu:?}");
    Ok(ERROR_NORMAL)
}

/// 옮긴 메뉴인지(실행 비교 테스트가 이 값으로 사례를 고른다).
pub fn is_ported(menu: MenuList) -> bool {
    menu == MenuList::None
        || [
            acl::PORTED,
            bucket::PORTED,
            multipart::PORTED,
            bucket_config::PORTED,
            bucket_rules::PORTED,
            object::PORTED,
            ksan::PORTED,
            backend::PORTED,
            util::PORTED,
            tests::PORTED,
        ]
        .iter()
        .any(|ported| ported.contains(&menu))
}

/// 원본 `CommandDispatcher.Execute`.
pub async fn execute(ctx: &mut CommandContext) -> CommandResult {
    use MenuList::*;
    let menu = ctx.options.menu;
    match menu {
        None => {
            print!("{}", crate::options::write_option_descriptions());
            Ok(0)
        }
        CreateBucket
        | DeleteBucket
        | HeadBucket
        | ListBuckets
        | ListDirectoryBuckets
        | GetBucketLocation
        | GetBucketVersioning
        | PutBucketVersioning
        | GetBucketOwnershipControls
        | PutBucketOwnershipControls
        | DeleteBucketOwnershipControls => bucket::run(ctx, menu).await,
        GetBucketAcl | PutBucketAcl | GetObjectAcl | PutObjectAcl => acl::run(ctx, menu).await,
        AbortMultipartUpload
        | CompleteMultipartUpload
        | CreateMultipartUpload
        | ListMultipartUploads
        | ListParts
        | UploadPart
        | UploadPartCopy => multipart::run(ctx, menu).await,
        GetBucketAnalytics
        | PutBucketAnalytics
        | DeleteBucketAnalytics
        | ListBucketAnalytics
        | GetBucketCors
        | PutBucketCors
        | DeleteBucketCors
        | GetBucketEncryption
        | PutBucketEncryption
        | DeleteBucketEncryption
        | GetBucketInventory
        | PutBucketInventory
        | DeleteBucketInventory
        | ListBucketInventory
        | GetBucketMetrics
        | PutBucketMetrics
        | DeleteBucketMetrics
        | ListBucketMetrics
        | GetBucketLogging
        | PutBucketLogging
        | GetBucketNotification
        | PutBucketNotification
        | GetBucketWebsite
        | PutBucketWebsite
        | DeleteBucketWebsite => bucket_config::run(ctx, menu).await,
        GetPublicAccessBlock
        | PutPublicAccessBlock
        | DeletePublicAccessBlock
        | GetBucketPolicy
        | PutBucketPolicy
        | DeleteBucketPolicy
        | GetBucketPolicyStatus
        | GetBucketTagging
        | PutBucketTagging
        | DeleteBucketTagging
        | GetBucketLifecycle
        | PutBucketLifecycle
        | DeleteBucketLifecycle
        | GetBucketReplication
        | PutBucketReplication
        | DeleteBucketReplication => bucket_rules::run(ctx, menu).await,
        CopyObject | DeleteObject | DeleteObjects | DeleteObjectTagging | GetObject
        | GetObjectLegalHold | GetObjectLock | GetObjectRetention | GetObjectTagging
        | GetPresignedUrl | HeadObject | RestoreObject | ListObjects | ListObjectsV2
        | ListObjectVersions | PutObject | PutObjects | PutObjectLegalHold | PutObjectLock
        | PutObjectRetention | PutObjectTagging | StorageMove => object::run(ctx, menu).await,
        DeleteBucketTagIndex | GetBucketTagIndex | ListBucketTagSearch | PutBucketTagIndex => {
            ksan::run(ctx, menu).await
        }
        S3backendPause | S3backendResume => backend::run(ctx, menu).await,
        SetObjectLock | DelObjectLock | Encryption | Upload | Download | Clear | BucketClear
        | CurrentClear | NoncurrentClear | MarkerClear => util::run(ctx, menu).await,
        _ => tests::run(ctx, menu).await,
    }
}
