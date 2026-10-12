//! 원본 `CommandDispatcher` 중 객체 업로드·다운로드·복사·삭제·조회·목록과 객체 설정.
//!
//! 파일 구성: `help`(도움말), `read`(조회·다운로드), `listing`(목록과 `ObjectData` 출력), `write`(업로드·삭제·복사·
//! `StorageMove`), `settings`(객체 잠금·보존·태그 설정), `input`(입력 JSON DTO), `files`(파일 도우미).
//!
//! 원본 동작 중 그대로 둔 것(버그 포함):
//!
//! - `GetObject`는 `Utility.SaveFile`이 `Directory.CreateDirectory(Path.GetDirectoryName(path))`를 부르므로 디렉터리가
//!   없는 상대 경로(`--file out.txt`)는 `ArgumentException`을 로그로 남기고 파일을 저장하지 않는다. 그래도 성공 로그를 낸다.
//!   `--checksum`에서 저장에 실패하고 응답에 체크섬이 있으면 `FileNotFoundException`으로 끝난다.
//! - `GetObject --size`는 본문을 한 번만 읽고(`Read(buffer, 0, size)`) 디렉터리를 만들지 않는다(`File.Create`).
//! - `ListObjectsV2`는 다음 페이지를 `ContinuationToken`이 아니라 `StartAfter`로 요청하고, 로그에는 새 토큰이 아닌
//!   직전 `marker`를 출력한다. `--continuation-token`은 쓰지 않는다. `ListObjectVersions`는 `CommonPrefixes`를 무시한다.
//! - `DeleteObjectTagging`, `PutObjectLegalHold`는 `--version-id`를 요청에 넣지 않는다.
//! - `PutObjectLock`은 `--lock-mode`가 `Compliance`가 아니면 모두 `Governance`(도움말은 기본값이 Compliance라고 한다)이고,
//!   days·years를 둘 다 주거나 하나도 안 줘도 안내만 출력하고 요청을 보낸다. `PutObjectRetention`은 반대로
//!   `Governance`가 아니면 `Compliance`다.
//! - `DeleteObjects`·`PutObjectLock`의 로그는 `--key`(보통 비어 있음)를 출력한다. `GetObjectLock`도 같다.
//! - `PutObject`에서 `--file`이 없는 파일이면 안내만 하고 본문 없이 업로드한다(`--md5sum`이면 `FileNotFoundException`).
//!   키가 없으면 요청을 만들 때 `ArgumentException`이다. 태그에 `=`가 없으면 `IndexOutOfRangeException`.
//! - `PutObjects`의 키는 파일의 전체 경로에서 `:`만 `/`로 바꾼 값이다.
//! - `GetPresignedUrl`·`PutObjectRetention`은 `DateTime.Now`/`DateTime.TryParse`를 쓰므로 현지 시간대에 따라 달라진다.
//!
//! .NET과 다른 점:
//!
//! - 목록의 수정 시각은 UTC 그대로 출력한다(.NET SDK가 UTC `DateTime`을 돌려준다).
//! - `ListObjectVersions`의 `Version`과 `DeleteMarker`는 SDK 응답에서 문서 순서를 잃어 키·수정 시각 순으로 합친다.
//! - 입력 JSON이 SDK 형식에 맞지 않는 경우(`KeyVersion.Key`, `Tag.Key/Value`, `TagSet` 누락, JSON `null`)는 원본이
//!   요소를 빼고 보내지만 여기서는 빈 문자열·빈 목록으로 채우거나 `AmazonClientException`이 된다.
//! - `DateTime.TryParse`는 ISO 8601과 `yyyy/MM/dd` 계열만 받는다.
//! - `HeadObject --print`의 `Headers` 덤프는 사람이 보는 용도라 속성 구성이 .NET과 다르다.

mod help;
mod input;
mod listing;
mod read;
mod settings;
mod write;

use super::input::blank;
use testrust_s3::S3Error;

use super::{CommandContext, CommandError, CommandResult};
use crate::menu::MenuList;
use crate::usage;

/// S3 호출 결과의 오류를 `CommandError`로 바꾼다. .NET SDK는 연산마다 오류 응답 해석기가 아는 코드(`modeled`)만
/// `AmazonS3Exception`의 하위 형식(`NoSuchKeyException` 등)으로 던지고, 나머지는 `AmazonS3Exception`이다.
/// `S3Error::dotnet_type`은 연산을 구분하지 않으므로 여기서 연산별로 좁힌다.
trait S3Result<T> {
    /// 하위 형식으로 해석하는 오류 코드가 없는 연산.
    fn plain(self) -> Result<T, CommandError>;
    /// `codes`만 하위 형식으로 해석하는 연산.
    fn modeled(self, codes: &[&str]) -> Result<T, CommandError>;
}

impl<T> S3Result<T> for Result<T, S3Error> {
    fn plain(self) -> Result<T, CommandError> {
        self.modeled(&[])
    }

    fn modeled(self, codes: &[&str]) -> Result<T, CommandError> {
        self.map_err(|error| CommandError::s3(error, codes))
    }
}

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[
    MenuList::CopyObject,
    MenuList::DeleteObject,
    MenuList::DeleteObjects,
    MenuList::DeleteObjectTagging,
    MenuList::GetObject,
    MenuList::GetObjectLegalHold,
    MenuList::GetObjectLock,
    MenuList::GetObjectRetention,
    MenuList::GetObjectTagging,
    MenuList::GetPresignedUrl,
    MenuList::HeadObject,
    MenuList::RestoreObject,
    MenuList::ListObjects,
    MenuList::ListObjectsV2,
    MenuList::ListObjectVersions,
    MenuList::PutObject,
    MenuList::PutObjects,
    MenuList::PutObjectLegalHold,
    MenuList::PutObjectLock,
    MenuList::PutObjectRetention,
    MenuList::PutObjectTagging,
    MenuList::StorageMove,
];

/// `--bucket`(검증을 통과했으므로 값이 있다).
fn bucket_name(ctx: &CommandContext) -> &str {
    ctx.options.bucket_name.as_deref().unwrap_or_default()
}

/// `--key`(없으면 빈 문자열: 원본의 문자열 보간은 `null`을 빈 문자열로 쓴다).
fn key_name(ctx: &CommandContext) -> &str {
    ctx.options.key.as_deref().unwrap_or_default()
}

/// 실행 전 필수 값 검증(원본의 `else if (string.IsNullOrWhiteSpace(x)) Console.WriteLine(...)` 사슬).
#[derive(Clone, Copy)]
enum Check {
    Bucket,
    Key,
    SourceKey,
    /// `--file`(`ERROR_FILE_PATH`)
    FilePath,
    /// `--file`(`ERROR_CONFIG_PATH`)
    ConfigPath,
    /// `File.Exists(--file)`(`ERROR_FILE`)
    FileExists,
    Date,
}

/// 첫 번째로 실패한 검사의 문구를 출력하고 `true`를 돌려준다.
fn rejected(ctx: &CommandContext, checks: &[Check]) -> bool {
    let o = &ctx.options;
    for check in checks {
        let message = match check {
            Check::Bucket if blank(&o.bucket_name) => usage::ERROR_BUCKET,
            Check::Key if blank(&o.key) => usage::ERROR_KEY,
            Check::SourceKey if blank(&o.source_key) => usage::ERROR_SOURCE_KEY,
            Check::FilePath if blank(&o.file_path) => usage::ERROR_FILE_PATH,
            Check::ConfigPath if blank(&o.file_path) => usage::ERROR_CONFIG_PATH,
            Check::FileExists
                if !testrust_scenarios::files::file_exists(
                    o.file_path.as_deref().unwrap_or_default(),
                ) =>
            {
                usage::ERROR_FILE
            }
            Check::Date if blank(&o.date) => "--date 보관 만료 날짜를 입력해야 합니다.",
            _ => continue,
        };
        println!("{message}");
        return true;
    }
    false
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use Check::*;
    use MenuList::*;
    if ctx.options.help {
        println!("{}", help::text(menu));
        return Ok(0);
    }
    let ctx = &*ctx;
    let checks: &[Check] = match menu {
        CopyObject => &[Bucket, Key, SourceKey],
        DeleteObject | DeleteObjectTagging | GetObject | GetObjectLegalHold
        | GetObjectRetention | GetObjectTagging | GetPresignedUrl | HeadObject | RestoreObject
        | StorageMove => &[Bucket, Key],
        DeleteObjects | PutObjects => &[Bucket, FilePath],
        GetObjectLock | ListObjects | ListObjectsV2 | ListObjectVersions | PutObject
        | PutObjectLock => &[Bucket],
        PutObjectLegalHold | PutObjectTagging => &[Bucket, Key, ConfigPath, FileExists],
        PutObjectRetention => &[Bucket, Key, Date],
        _ => unreachable!("object 모듈의 메뉴가 아니다: {menu:?}"),
    };
    if rejected(ctx, checks) {
        return Ok(0);
    }
    match menu {
        CopyObject => write::copy_object(ctx).await,
        DeleteObject => write::delete_object(ctx).await,
        DeleteObjects => write::delete_objects(ctx).await,
        DeleteObjectTagging => write::delete_object_tagging(ctx).await,
        GetObject => read::get_object(ctx).await,
        GetObjectLegalHold => read::get_object_legal_hold(ctx).await,
        GetObjectLock => read::get_object_lock(ctx).await,
        GetObjectRetention => read::get_object_retention(ctx).await,
        GetObjectTagging => read::get_object_tagging(ctx).await,
        GetPresignedUrl => read::get_presigned_url(ctx).await,
        HeadObject => read::head_object(ctx).await,
        RestoreObject => write::restore_object(ctx).await,
        ListObjects => listing::list_objects(ctx, false).await,
        ListObjectsV2 => listing::list_objects(ctx, true).await,
        ListObjectVersions => listing::list_object_versions(ctx).await,
        PutObject => write::put_object(ctx).await,
        PutObjects => write::put_objects(ctx).await,
        PutObjectLegalHold => settings::put_object_legal_hold(ctx).await,
        PutObjectLock => settings::put_object_lock(ctx).await,
        PutObjectRetention => settings::put_object_retention(ctx).await,
        PutObjectTagging => settings::put_object_tagging(ctx).await,
        StorageMove => write::storage_move(ctx).await,
        _ => unreachable!("object 모듈의 메뉴가 아니다: {menu:?}"),
    }
}
