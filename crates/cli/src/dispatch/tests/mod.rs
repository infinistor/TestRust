//! 원본 `CommandDispatcher` 중 테스트 시나리오 메뉴(`Test/*` 클래스 호출).
//!
//! 도움말과 실행 전 검증(설정 누락, 필수 인자)은 여기서, 시나리오 실행은 묶음별 하위 모듈(`up_down`, `system`,
//! `compare`, `transfer`)에서 한다. 시나리오 본체는 `testrust-scenarios`에 있다.
//! 원본 `MixV2Test`는 `--help`여도 도움말을 출력한 뒤 테스트를 실행한다(원본 버그, 그대로 둔다). 버킷 이름이 빈 채로
//! 실행되는 `--test-mix-v2 --help`(자동 사례 `help/test-mix-v2`)는 .NET처럼 `GET /?acl`을 보낸다(`S3Client::does_s3_bucket_exist`).

mod compare;
mod system;
mod transfer;
mod up_down;

use super::input::blank;
use super::{CommandContext, CommandResult, ERROR_NORMAL};
use crate::menu::MenuList;
use crate::usage;

/// 옮긴 메뉴(도움말·검증 기준).
pub(super) const PORTED: &[MenuList] = &[
    MenuList::RangeReadCopy,
    MenuList::ManualUpload,
    MenuList::AccessIpsTest,
    MenuList::ListObjTest,
    MenuList::UploadTest,
    MenuList::DownloadTest,
    MenuList::Prepare,
    MenuList::PrepareDir,
    MenuList::NewPutTest,
    MenuList::PutTest,
    MenuList::HeadTest,
    MenuList::GetTest,
    MenuList::GetTestV2,
    MenuList::NewGetTest,
    MenuList::GetTestV3,
    MenuList::DeleteTest,
    MenuList::DeleteTestV2,
    MenuList::NewDelTest,
    MenuList::DeleteTestVersion,
    MenuList::DeleteDirectoryTest,
    MenuList::MixTest,
    MenuList::MixV2Test,
    MenuList::NewMixTest,
    MenuList::PutGetTest,
    MenuList::AllTest,
    MenuList::LocalPrepareTest,
    MenuList::LocalPutTest,
    MenuList::LocalGetTest,
    MenuList::LocalGetTestV2,
    MenuList::LocalPutGetTest,
    MenuList::LocalDeleteTest,
    MenuList::LocalMultipartPrepareTest,
    MenuList::LocalMultipartPutTest,
    MenuList::LocalMultipartGetTest,
    MenuList::LocalMultipartGetTestV2,
    MenuList::LocalMultipartPutGetTest,
    MenuList::FullTest,
    MenuList::MultiDeleteTest,
    MenuList::MultipartUploadTest,
    MenuList::MultipartUploadAndDownloadTest,
    MenuList::MultiSystemListTest,
    MenuList::MultiSystemUploadTest,
    MenuList::MultiSystemUpDownTest,
    MenuList::MultiSystemAllTest,
    MenuList::IoTest,
    MenuList::MoverTest,
    MenuList::PutTagTest,
    MenuList::FindTagTest,
    MenuList::CompareTest,
    MenuList::UsedSizeTest,
    MenuList::DuplicateTest,
    MenuList::MultiUploadTest,
    MenuList::DirectoryDownloadTest,
    MenuList::FileListDownloadTest,
    MenuList::LifecycleTest,
    MenuList::RangeReadTest,
    MenuList::AWSTest,
];

/// 메뉴별 도움말. 원본에 `case`가 없는 메뉴는 `None`.
fn help_text(menu: MenuList) -> Option<String> {
    use MenuList::*;
    Some(match menu {
        RangeReadCopy => [
            usage::main_flag(usage::TEST_RANGE_COPY, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::SOURCE, "string", ""),
            usage::sub_flag(usage::SOURCE_KEY, "string", ""),
        ]
        .concat(),
        ManualUpload => [
            usage::main_flag(usage::MANUAL_UPLOAD, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
            usage::optional_value(usage::PART_SIZE, "string", ""),
        ]
        .concat(),
        AccessIpsTest => usage::TEST_USAGE_ACCESS_IPS.to_string(),
        ListObjTest => usage::TEST_USAGE_LIST_OBJECT.to_string(),
        UploadTest => usage::TEST_USAGE_UPLOAD.to_string(),
        DownloadTest => usage::TEST_USAGE_DOWNLOAD.to_string(),
        Prepare => [
            usage::main_flag(usage::TEST_PREPARE, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_CONFIG.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::USAGE_START.to_string(),
            usage::optional(usage::CHECK, " : 파일 내용 체크 여부"),
            usage::optional(usage::NOT_EMPTY, " : 매번 파일의 내용이 다르게 업로드"),
            usage::TEST_USAGE_PREPARE.to_string(),
        ]
        .concat(),
        PrepareDir => [
            usage::main_flag(usage::TEST_PREPARE_DIR, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_CONFIG.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_START.to_string(),
            usage::optional(usage::CHECK, " : 이미 존재하는 폴더 건너뛰기"),
            usage::TEST_USAGE_PREPARE.to_string(),
        ]
        .concat(),
        NewPutTest => [
            usage::main_flag(usage::TEST_NEW_PUT, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_CONFIG.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::USAGE_START.to_string(),
            usage::optional(usage::CHECK, " : 파일 내용 체크 여부"),
            usage::TEST_USAGE_PREPARE.to_string(),
        ]
        .concat(),
        PutTest => [
            usage::main_flag(usage::TEST_PUT, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_TIMES.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::USAGE_START.to_string(),
            usage::TEST_USAGE_PUT.to_string(),
        ]
        .concat(),
        HeadTest => [
            usage::main_flag(usage::TEST_HEAD, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::TEST_USAGE_HEAD.to_string(),
        ]
        .concat(),
        GetTest => [
            usage::main_flag(usage::TEST_GET, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_TIMES.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_START.to_string(),
            usage::TEST_USAGE_GET.to_string(),
        ]
        .concat(),
        GetTestV2 => [
            usage::main_flag(usage::TEST_GET_V2, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_START.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::TEST_USAGE_GET.to_string(),
        ]
        .concat(),
        NewGetTest => [
            usage::main_flag(usage::TEST_NEW_GET, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_START.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::TEST_USAGE_GET.to_string(),
        ]
        .concat(),
        GetTestV3 => [
            usage::main_flag(usage::TEST_GET_V3, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_GET.to_string(),
        ]
        .concat(),
        DeleteTest => [
            usage::main_flag(usage::TEST_DELETE, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
            usage::TEST_USAGE_DELETE.to_string(),
        ]
        .concat(),
        DeleteTestV2 => [
            usage::main_flag(usage::TEST_DELETE_V2, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_START.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::TEST_USAGE_DELETE.to_string(),
        ]
        .concat(),
        NewDelTest => [
            usage::main_flag(usage::TEST_NEW_DEL, ""),
            usage::USAGE_BUCKET_NAME.to_string(),
            usage::USAGE_THREAD_COUNT.to_string(),
            usage::USAGE_SIZE.to_string(),
            usage::USAGE_FILE_COUNT.to_string(),
            usage::USAGE_START.to_string(),
            usage::USAGE_PREFIX.to_string(),
            usage::USAGE_PATH.to_string(),
            usage::TEST_USAGE_DELETE.to_string(),
        ]
        .concat(),
        DeleteTestVersion => [
            usage::main_flag(usage::TEST_DELETE_VERSION, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
            usage::TEST_USAGE_DELETE_VERSION.to_string(),
        ]
        .concat(),
        DeleteDirectoryTest => [
            usage::main_flag(usage::TEST_DELETE_DIRECTORY, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_DELETE_DIRECTORY.to_string(),
        ]
        .concat(),
        MixTest => [
            usage::main_flag(usage::TEST_MIX, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_MIX.to_string(),
        ]
        .concat(),
        MixV2Test => [
            usage::main_flag(usage::TEST_MIX_V2, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_MIX_V2.to_string(),
        ]
        .concat(),
        NewMixTest => [
            usage::main_flag(usage::TEST_NEW_MIX, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_ALL.to_string(),
        ]
        .concat(),
        PutGetTest => [
            usage::main_flag(usage::TEST_PUT_GET, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_PUT_GET.to_string(),
        ]
        .concat(),
        AllTest => [
            usage::main_flag(usage::TEST_ALL, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_ALL.to_string(),
        ]
        .concat(),
        LocalPrepareTest => [
            usage::main_flag(usage::TEST_LOCAL_PREPARE, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::optional_value(usage::CHECK, "string", ""),
            usage::optional_value(usage::START, "string", ""),
            usage::TEST_USAGE_LOCAL_PREPARE.to_string(),
        ]
        .concat(),
        LocalPutTest => [
            usage::main_flag(usage::TEST_LOCAL_PUT, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_PUT.to_string(),
        ]
        .concat(),
        LocalGetTest => [
            usage::main_flag(usage::TEST_LOCAL_GET, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_GET.to_string(),
        ]
        .concat(),
        LocalGetTestV2 => [
            usage::main_flag(usage::TEST_LOCAL_GET_V2, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::optional_value(usage::START, "string", ""),
            usage::TEST_USAGE_LOCAL_GET_V2.to_string(),
        ]
        .concat(),
        LocalPutGetTest => [
            usage::main_flag(usage::TEST_LOCAL_PUT_GET, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_PUT_GET.to_string(),
        ]
        .concat(),
        LocalDeleteTest => [
            usage::main_flag(usage::TEST_LOCAL_DELETE, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_DELETE.to_string(),
        ]
        .concat(),
        LocalMultipartPrepareTest => [
            usage::main_flag(usage::TEST_LOCAL_MULTIPART_PREPARE, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::optional_value(usage::CHECK, "string", ""),
            usage::optional_value(usage::START, "string", ""),
            usage::TEST_USAGE_LOCAL_PREPARE.to_string(),
        ]
        .concat(),
        LocalMultipartPutTest => [
            usage::main_flag(usage::TEST_LOCAL_MULTIPART_PUT, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_PUT.to_string(),
        ]
        .concat(),
        LocalMultipartGetTest => [
            usage::main_flag(usage::TEST_LOCAL_MULTIPART_GET, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_GET.to_string(),
        ]
        .concat(),
        LocalMultipartGetTestV2 => [
            usage::main_flag(usage::TEST_LOCAL_MULTIPART_GET_V2, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::optional_value(usage::START, "string", ""),
            usage::TEST_USAGE_LOCAL_GET_V2.to_string(),
        ]
        .concat(),
        LocalMultipartPutGetTest => [
            usage::main_flag(usage::TEST_LOCAL_MULTIPART_PUT_GET, ""),
            usage::optional_value(usage::TARGET_PATH, "string", ""),
            usage::TEST_USAGE_LOCAL_PUT_GET.to_string(),
        ]
        .concat(),
        FullTest => usage::main_flag(usage::TEST_FULL, "").to_string(),
        MultiDeleteTest => [
            usage::main_flag(usage::TEST_MULTI_DELETE, ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
            usage::TEST_USAGE_MULTI_DELETE.to_string(),
        ]
        .concat(),
        MultipartUploadTest => [
            usage::main_flag(usage::TEST_MULTIPART_UPLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_MULTIPART_UPLOAD.to_string(),
        ]
        .concat(),
        MultipartUploadAndDownloadTest => [
            usage::main_flag(usage::TEST_MULTIPART_UPLOAD_DOWNLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_MULTIPART_UPLOAD_V2.to_string(),
        ]
        .concat(),
        MultiSystemListTest => [
            usage::main_flag(usage::TEST_MULTI_SYSTEM_LIST, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        MultiSystemUploadTest => [
            usage::main_flag(usage::TEST_MULTI_SYSTEM_UPLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::MULTIPART, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        MultiSystemUpDownTest => [
            usage::main_flag(usage::TEST_MULTI_SYSTEM_UP_DOWN, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::MULTIPART, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        MultiSystemAllTest => [
            usage::main_flag(usage::TEST_MULTI_SYSTEM_ALL, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::MULTIPART, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        IoTest => [
            usage::main_flag(usage::TEST_IO, ""),
            usage::main_flag(usage::BUCKET, ""),
            usage::main_flag(usage::SOURCE, " : Upload File Path"),
            usage::main_flag(usage::TARGET, " : Download File Path"),
        ]
        .concat(),
        MoverTest => [
            usage::main_flag(usage::TEST_MOVER, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_MOVER.to_string(),
        ]
        .concat(),
        PutTagTest => [
            usage::main_flag(usage::TEST_PUT_TAG, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_PUT_TAG.to_string(),
        ]
        .concat(),
        FindTagTest => [
            usage::main_flag_value(
                usage::TEST_FIND_TAG,
                "string",
                " : 찾을 태그명 ex> tag1:value1",
            ),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::optional(usage::THREAD_COUNT, ""),
        ]
        .concat(),
        CompareTest => [
            usage::main_flag(usage::TEST_COMPARE, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_COMPARE.to_string(),
        ]
        .concat(),
        UsedSizeTest => [
            usage::main_flag(usage::TEST_USED_SIZE, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_USED_SIZE.to_string(),
        ]
        .concat(),
        DuplicateTest => [
            usage::main_flag(usage::TEST_DUPLICATE, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::TEST_USAGE_DUPLICATE.to_string(),
        ]
        .concat(),
        MultiUploadTest => [
            usage::main_flag(usage::TEST_MULTI_UPLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::sub_flag(usage::THREAD_COUNT, "string", ""),
            usage::optional_value(usage::SIZE, "string", ""),
            usage::optional_value(usage::PART_SIZE, "string", ""),
        ]
        .concat(),
        DirectoryDownloadTest => [
            usage::main_flag(usage::TEST_DIRECTORY_DOWNLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::PREFIX, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
            usage::optional_value(usage::THREAD_COUNT, "string", ""),
        ]
        .concat(),
        FileListDownloadTest => [
            usage::main_flag(usage::TEST_FILE_LIST_DOWNLOAD, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::FILE, "string", ""),
            usage::optional_value(usage::THREAD_COUNT, "string", ""),
        ]
        .concat(),
        LifecycleTest => [
            usage::main_flag(usage::TEST_COMPARE_LIFECYCLE, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
        ]
        .concat(),
        RangeReadTest => [
            usage::main_flag(usage::TEST_RANGE_READ, ""),
            usage::sub_flag(usage::BUCKET, "string", ""),
            usage::sub_flag(usage::KEY, "string", ""),
            usage::optional_value(usage::RANGE_LIST, "string", ""),
            usage::optional_value(usage::THREAD_COUNT, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        AWSTest => [
            usage::main_flag(usage::TEST_AWS, ""),
            usage::optional_value(usage::BUCKET, "string", ""),
            usage::optional_value(usage::COUNT, "string", ""),
        ]
        .concat(),
        _ => return Option::None,
    })
}

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    use MenuList::*;
    let Some(help) = help_text(menu) else {
        // 원본 `switch`에 없는 메뉴(URL, MaxCount 등)는 아무것도 하지 않는다.
        return Ok(0);
    };
    if ctx.options.help {
        println!("{help}");
        // 원본 MixV2Test는 도움말 뒤에도 실행을 이어 간다.
        if menu != MixV2Test {
            return Ok(0);
        }
    }
    let o = &ctx.options;
    match menu {
        Prepare | PrepareDir | NewPutTest | PutTest | GetTest | GetTestV2 | NewGetTest
        | DeleteTestV2 | NewDelTest | NewMixTest | PutGetTest | AllTest => {
            if ctx.config().up_down.is_empty() {
                println!("{}", usage::ERROR_INVALID_CONFIG);
                return Ok(ERROR_NORMAL);
            }
        }
        LocalPrepareTest
        | LocalPutTest
        | LocalGetTest
        | LocalGetTestV2
        | LocalPutGetTest
        | LocalDeleteTest
        | LocalMultipartPrepareTest
        | LocalMultipartPutTest
        | LocalMultipartGetTest
        | LocalMultipartGetTestV2
        | LocalMultipartPutGetTest => {
            if ctx.config().main.target_path.is_empty() {
                println!("{}", usage::ERROR_TARGET_PATH);
                return Ok(ERROR_NORMAL);
            }
        }
        MixV2Test => {
            // 삭제 비율이 생성 비율보다 높을 경우 에러
            // 도움말 실행이면 설정이 없을 수 있다(원본은 NullReferenceException).
            let Some(config) = &ctx.config else {
                return Err(super::input::null_reference());
            };
            let up_down = &config.up_down;
            if up_down.delete_ratio > up_down.write_ratio {
                println!("삭제 비율이 생성 비율보다 높을 수 없습니다.");
                return Ok(ERROR_NORMAL);
            }
        }
        ManualUpload | MultiUploadTest | RangeReadTest => {
            if blank(&o.bucket_name) {
                println!("{}", usage::ERROR_BUCKET);
                return Ok(0);
            }
            if blank(&o.key) {
                println!("{}", usage::ERROR_KEY);
                return Ok(0);
            }
            if menu == ManualUpload && blank(&o.file_path) {
                println!("{}", usage::ERROR_FILE_PATH);
                return Ok(0);
            }
        }
        DirectoryDownloadTest | FileListDownloadTest => {
            if blank(&o.bucket_name) {
                println!("{}", usage::ERROR_BUCKET);
                return Ok(0);
            }
            if menu == FileListDownloadTest && blank(&o.path) {
                println!("{}", usage::ERROR_PATH);
                return Ok(0);
            }
            if blank(&o.file_path) {
                println!("{}", usage::ERROR_FILE_PATH);
                return Ok(0);
            }
        }
        LifecycleTest | AWSTest if blank(&o.bucket_name) => {
            println!("{}", usage::ERROR_BUCKET);
            return Ok(0);
        }
        _ => {}
    }
    match menu {
        LocalPrepareTest
        | LocalPutTest
        | LocalGetTest
        | LocalGetTestV2
        | LocalPutGetTest
        | LocalDeleteTest
        | LocalMultipartPrepareTest
        | LocalMultipartPutTest
        | LocalMultipartGetTest
        | LocalMultipartGetTestV2
        | LocalMultipartPutGetTest
        | MultiSystemListTest
        | MultiSystemUploadTest
        | MultiSystemUpDownTest
        | MultiSystemAllTest
        | AccessIpsTest
        | UsedSizeTest => system::run(ctx, menu).await,
        CompareTest | RangeReadCopy | DuplicateTest | LifecycleTest | MoverTest => {
            compare::run(ctx, menu).await
        }
        ManualUpload
        | MultiUploadTest
        | RangeReadTest
        | FindTagTest
        | DirectoryDownloadTest
        | FileListDownloadTest
        | IoTest => transfer::run(ctx, menu).await,
        _ => up_down::run(ctx, menu).await,
    }
}
