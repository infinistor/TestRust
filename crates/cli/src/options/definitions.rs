//! TESTCore `Cli/CliOptionParser.cs`의 옵션 표. 선언 순서(도움말 순서)와 설명 문구를 원본 그대로 둔다.

use testrust_config::EnumBucketTypes;
use testrust_s3::ChecksumAlgorithm;

use super::convert::long_parse_exception;
use super::{CommandOptions, ParseError};
use crate::menu::MenuList;

/// 옵션 하나. `prototype`은 Mono.Options 형식(`b|bucket=`: 이름을 `|`로 나누고 값이 필요하면 `=`).
pub struct OptionDef {
    pub prototype: &'static str,
    pub description: &'static str,
    pub action: Action,
}

/// 옵션 동작. 원본 람다의 인자 형식에 따라 나뉜다.
#[derive(Clone, Copy)]
pub enum Action {
    /// 값 없는 옵션: `n => commandOptions.Menu = MenuList.X`.
    Menu(MenuList),
    /// 값 없는 옵션: `n => ...`.
    Set(fn(&mut CommandOptions)),
    /// 문자열 값: `n => ...`. 값은 `null`일 수 있다(`--bucket-`).
    Text(fn(&mut CommandOptions, Option<String>)),
    /// 예외를 던질 수 있는 문자열 값.
    TryText(fn(&mut CommandOptions, Option<String>) -> Result<(), ParseError>),
    /// `(int i) => ...`.
    Int(fn(&mut CommandOptions, i32)),
    /// `(bool b) => ...`.
    Bool(fn(&mut CommandOptions, bool)),
}

impl Action {
    /// 값이 필요한 옵션인지(`OptionValueType.Required`).
    pub fn takes_value(self) -> bool {
        !matches!(self, Self::Menu(_) | Self::Set(_))
    }
}

const fn opt(prototype: &'static str, description: &'static str, action: Action) -> OptionDef {
    OptionDef {
        prototype,
        description,
        action,
    }
}

/// 원본 `Utility.SizeToLong`. 실패하면 원본처럼 처리하지 않은 예외가 된다.
fn size_to_long(value: Option<&str>) -> Result<i64, ParseError> {
    let Some(value) = value else { return Ok(0) };
    testrust_config::util::size_to_long(value).map_err(|e| match e {
        testrust_config::UtilError::InvalidNumber(input) => {
            let (dotnet_type, message) = long_parse_exception(&input);
            ParseError::Unhandled {
                dotnet_type,
                message,
            }
        }
        other => ParseError::Unhandled {
            dotnet_type: "System.Exception",
            message: other.to_string(),
        },
    })
}

/// `--range-list`: 쉼표로 나눈 크기를 누적한다. 값이 `null`이면 원본은 `NullReferenceException`.
fn range_list(o: &mut CommandOptions, value: Option<String>) -> Result<(), ParseError> {
    let Some(value) = value else {
        return Err(ParseError::Unhandled {
            dotnet_type: "System.NullReferenceException",
            message: "Object reference not set to an instance of an object.".to_string(),
        });
    };
    for size in value.split(',') {
        o.range_list.push(size_to_long(Some(size))?);
    }
    Ok(())
}

/// `--checksum-type`: 앞뒤 공백을 자르고 대문자로 바꿔 알고리즘을 고른다.
fn checksum_type(o: &mut CommandOptions, value: Option<String>) -> Result<(), ParseError> {
    let name = value.map(|v| v.trim().to_uppercase());
    let algorithm = name
        .as_deref()
        .map_or(ChecksumAlgorithm::None, ChecksumAlgorithm::from_name);
    if algorithm == ChecksumAlgorithm::None && name.as_deref() != Some("NONE") {
        return Err(ParseError::option(
            "지원하지 않는 체크섬 타입입니다. CRC32, CRC32C, CRC64NVME, SHA1, SHA256, None 중 하나를 지정하세요."
                .to_string(),
            "checksum-type".to_string(),
        ));
    }
    o.checksum_type = algorithm;
    Ok(())
}

use Action::{Bool, Int, Menu, Set, Text, TryText};

/// 원본 `OptionSet` 초기화 목록(233개).
#[rustfmt::skip]
pub static OPTIONS: &[OptionDef] = &[
    opt("worker", "분산 테스트 Worker HTTP 서버 실행", Set(|o| o.worker = true)),
    opt("controller", "설정된 Worker에서 테스트 실행 및 CSV/JSON 결과 수집", Set(|o| o.controller = true)),
    opt("?|h|help", "Usage.", Set(|o| o.help = true)),
    opt("v|version", "Show version information.", Set(|o| o.version = true)),
    opt("abort-multipart-upload", "멀티파트업로드 중단 및 삭제", Menu(MenuList::AbortMultipartUpload)),
    opt("complete-multipart-upload", "멀티파트 업로드 종료. 파일 합치기", Menu(MenuList::CompleteMultipartUpload)),
    opt("copy-object", "객체 복사", Menu(MenuList::CopyObject)),
    opt("create-bucket", "버킷 생성", Menu(MenuList::CreateBucket)),
    opt("create-multipart-upload", "멀티파트 업로드 생성", Menu(MenuList::CreateMultipartUpload)),
    opt("delete-bucket", "버킷 삭제", Menu(MenuList::DeleteBucket)),
    opt("delete-bucket-analytics", "버킷의 분석 설정 삭제", Menu(MenuList::DeleteBucketAnalytics)),
    opt("delete-bucket-cors", "CORS 설정 삭제", Menu(MenuList::DeleteBucketCors)),
    opt("delete-bucket-encryption", "버킷의 암호화 설정 삭제", Menu(MenuList::DeleteBucketEncryption)),
    opt("delete-bucket-inventory", "버킷의 인벤토리 설정 삭제", Menu(MenuList::DeleteBucketInventory)),
    opt("delete-bucket-lifecycle", "버킷의 수명주기 설정 삭제", Menu(MenuList::DeleteBucketLifecycle)),
    opt("delete-bucket-metrics", "버킷의 메트릭 설정 삭제", Menu(MenuList::DeleteBucketMetrics)),
    opt("delete-bucket-ownership-controls", "버킷의 소유권 설정 삭제", Menu(MenuList::DeleteBucketOwnershipControls)),
    opt("delete-bucket-policy", "버킷의 정책 삭제", Menu(MenuList::DeleteBucketPolicy)),
    opt("delete-bucket-replication", "버킷의 복제 설정 삭제", Menu(MenuList::DeleteBucketReplication)),
    opt("delete-bucket-tagging", "버킷의 태그 삭제", Menu(MenuList::DeleteBucketTagging)),
    opt("delete-bucket-website", "버킷의 웹사이트 설정 삭제", Menu(MenuList::DeleteBucketWebsite)),
    opt("delete-object", "객체 삭제", Menu(MenuList::DeleteObject)),
    opt("delete-object-tagging", "객체의 태그 삭제", Menu(MenuList::DeleteObjectTagging)),
    opt("delete-objects", "다수의 객체 삭제", Menu(MenuList::DeleteObjects)),
    opt("delete-public-access-block", "버킷의 Public Access Block 설정 삭제", Menu(MenuList::DeletePublicAccessBlock)),
    opt("get-bucket-acl", "버킷의 권한 정보 조회", Menu(MenuList::GetBucketAcl)),
    opt("get-bucket-analytics", "버킷의 분석 설정 조회", Menu(MenuList::GetBucketAnalytics)),
    opt("get-bucket-cors", "버킷의 CORS 설정 조회", Menu(MenuList::GetBucketCors)),
    opt("get-bucket-encryption", "버킷의 암호화 설정 조회", Menu(MenuList::GetBucketEncryption)),
    opt("get-bucket-inventory", "버킷의 인벤토리 설정 조회", Menu(MenuList::GetBucketInventory)),
    opt("get-bucket-lifecycle", "버킷의 수명주기 설정 조회", Menu(MenuList::GetBucketLifecycle)),
    opt("get-bucket-logging", "버킷의 로그 설정 조회", Menu(MenuList::GetBucketLogging)),
    opt("get-bucket-metrics", "버킷의 메트릭 설정 조회", Menu(MenuList::GetBucketMetrics)),
    opt("get-bucket-location", "버킷의 리전 정보 조회", Menu(MenuList::GetBucketLocation)),
    opt("get-bucket-notification", "버킷의 알림 설정 조회", Menu(MenuList::GetBucketNotification)),
    opt("get-bucket-ownership-controls", "버킷의 소유권 설정 조회", Menu(MenuList::GetBucketOwnershipControls)),
    opt("get-bucket-policy", "버킷의 정책 조회", Menu(MenuList::GetBucketPolicy)),
    opt("get-bucket-policy-status", "버킷의 정책 상태 조회", Menu(MenuList::GetBucketPolicyStatus)),
    opt("get-bucket-replication", "버킷의 복제 설정 조회", Menu(MenuList::GetBucketReplication)),
    opt("get-bucket-tagging", "버킷의 태그 조회", Menu(MenuList::GetBucketTagging)),
    opt("get-bucket-versioning", "버킷의 버전설정 조회", Menu(MenuList::GetBucketVersioning)),
    opt("get-bucket-website", "버킷의 웹사이트 설정 조회", Menu(MenuList::GetBucketWebsite)),
    opt("get-object", "객체 다운로드", Menu(MenuList::GetObject)),
    opt("get-object-acl", "객체의 권한 설정 조회", Menu(MenuList::GetObjectAcl)),
    opt("get-object-legal-hold", "객체의 보존(legal-hold) 설정 조회", Menu(MenuList::GetObjectLegalHold)),
    opt("get-object-lock", "객체의 잠금(lock) 설정 조회", Menu(MenuList::GetObjectLock)),
    opt("get-object-retention", "객체의 보유(Retention) 설정 조회", Menu(MenuList::GetObjectRetention)),
    opt("get-object-tagging", "객체의 태그 조회", Menu(MenuList::GetObjectTagging)),
    opt("get-public-access-block", "객체의 Public Access Block 설정 조회", Menu(MenuList::GetPublicAccessBlock)),
    opt("get-presigned-url", "객체의 Presigned URL 생성(GET)", Menu(MenuList::GetPresignedUrl)),
    opt("head-bucket", "버킷의 정보 조회", Menu(MenuList::HeadBucket)),
    opt("head-object", "객체의 정보 조회", Menu(MenuList::HeadObject)),
    opt("restore-object", "객체 복원", Menu(MenuList::RestoreObject)),
    opt("list-bucket-analytics", "버킷의 분석 설정 목록 조회", Menu(MenuList::ListBucketAnalytics)),
    opt("list-bucket-inventory", "버킷 인벤토리 목록 조회", Menu(MenuList::ListBucketInventory)),
    opt("list-bucket-metrics", "버킷 메트릭 목록 조회", Menu(MenuList::ListBucketMetrics)),
    opt("list-buckets", "버킷 목록 조회", Menu(MenuList::ListBuckets)),
    opt("list-directory-buckets", "버킷 목록 조회", Menu(MenuList::ListDirectoryBuckets)),
    opt("list-multipart-uploads", "멀티파트 업로드 목록 조회", Menu(MenuList::ListMultipartUploads)),
    opt("list-object-versions", "객체 버전 목록 조회", Menu(MenuList::ListObjectVersions)),
    opt("list-objects", "객체 목록 조회", Menu(MenuList::ListObjects)),
    opt("list-objects-v2", "객체 목록 조회(V2)", Menu(MenuList::ListObjectsV2)),
    opt("list-parts", "파츠 목록 조회", Menu(MenuList::ListParts)),
    opt("put-bucket-acl", "버킷의 권한 설정", Menu(MenuList::PutBucketAcl)),
    opt("put-bucket-analytics", "버킷의 분석 설정", Menu(MenuList::PutBucketAnalytics)),
    opt("put-bucket-cors", "버킷의 CORS 설정", Menu(MenuList::PutBucketCors)),
    opt("put-bucket-encryption", "버킷의 기본 암호화 설정", Menu(MenuList::PutBucketEncryption)),
    opt("put-bucket-inventory", "버킷의 인벤토리 설정", Menu(MenuList::PutBucketInventory)),
    opt("put-bucket-lifecycle", "버킷내의 객체 수명주기 설정", Menu(MenuList::PutBucketLifecycle)),
    opt("put-bucket-logging", "버킷의 로그 설정", Menu(MenuList::PutBucketLogging)),
    opt("put-bucket-metrics", "버킷의 메트릭 설정", Menu(MenuList::PutBucketMetrics)),
    opt("put-bucket-notification", "버킷의 알람 설정", Menu(MenuList::PutBucketNotification)),
    opt("put-bucket-ownership", "버킷의 소유권 설정", Menu(MenuList::PutBucketOwnershipControls)),
    opt("put-bucket-policy", "버킷의 정책 설정", Menu(MenuList::PutBucketPolicy)),
    opt("put-bucket-replication", "버킷의 복제기능 설정", Menu(MenuList::PutBucketReplication)),
    opt("put-bucket-tagging", "버킷의 태그 설정", Menu(MenuList::PutBucketTagging)),
    opt("put-bucket-versioning", "버킷의 버저닝 설정", Menu(MenuList::PutBucketVersioning)),
    opt("put-bucket-website", "버킷의 웹사이트 설정", Menu(MenuList::PutBucketWebsite)),
    opt("put-object", "객체 업로드", Menu(MenuList::PutObject)),
    opt("put-objects", "폴더 업로드", Menu(MenuList::PutObjects)),
    opt("put-object-acl", "객체의 권한 설정", Menu(MenuList::PutObjectAcl)),
    opt("put-object-legal-hold", "객체의 보존(legal-hold) 설정", Menu(MenuList::PutObjectLegalHold)),
    opt("put-object-lock", "객체의 잠금(lock) 설정", Menu(MenuList::PutObjectLock)),
    opt("put-object-retention", "객체의 보유(Retention) 설정", Menu(MenuList::PutObjectRetention)),
    opt("put-object-tagging", "객체의 태그 설정", Menu(MenuList::PutObjectTagging)),
    opt("put-public-access-block", "객체의 Public Access Block 설정", Menu(MenuList::PutPublicAccessBlock)),
    opt("upload-part", "파츠 업로드", Menu(MenuList::UploadPart)),
    opt("upload-part-copy", "파츠 복제", Menu(MenuList::UploadPartCopy)),
    opt("storage-move", "오브젝트의 스토리지 클래스 변경", Menu(MenuList::StorageMove)),
    opt("set-object-lock", "객체의 Legal Hold 설정(ON)", Menu(MenuList::SetObjectLock)),
    opt("del-object-lock", "객체의 Legal Hold 해제(OFF)", Menu(MenuList::DelObjectLock)),
    opt("upload", "AWS 상위레벨 API를 사용하여 업로드", Menu(MenuList::Upload)),
    opt("download", "AWS 상위레벨 API를 사용하여 다운로드", Menu(MenuList::Download)),
    opt("delete-bucket-tagindex", "버킷의 태그 설정 삭제", Menu(MenuList::DeleteBucketTagIndex)),
    opt("get-bucket-tagindex", "버킷의 태그 설정 조회", Menu(MenuList::GetBucketTagIndex)),
    opt("list-bucket-tagsearch", "버킷에 존재하는 오브젝트의 태그 정보를 바탕으로 목록 조회", Menu(MenuList::ListBucketTagSearch)),
    opt("put-bucket-tagindex", "버킷의 태그 설정 설정", Menu(MenuList::PutBucketTagIndex)),
    opt("pause=", "Value=ServiceType. S3backend 서비스 일시정지", Text(|o, v| { o.menu = MenuList::S3backendPause; o.service_type = v; })),
    opt("resume=", "Value=ServiceType. S3backend 서비스 재개", Text(|o, v| { o.menu = MenuList::S3backendResume; o.service_type = v; })),
    opt("use-chunk-encoding=", "Value=true/false. Chunk Encoding 사용 여부", Bool(|o, v| o.use_chunk_encoding = v)),
    opt("c|config=", "Value=설정파일 경로.", Text(|o, v| o.config_path = v)),
    opt("s|save=", "Value=결과 저장 경로.", Text(|o, v| o.save = v)),
    opt("admin", "관리자 모드 활성화", Set(|o| o.admin = true)),
    opt("b|bucket=", "Value=버킷 이름", Text(|o, v| o.bucket_name = v)),
    opt("access-key=", "value=Access Key", Text(|o, v| o.access_key = v)),
    opt("secret-key=", "value=Secret Key", Text(|o, v| o.secret_key = v)),
    opt("source=", "Value=원본 이름", Text(|o, v| o.source = v)),
    opt("target=", "Value=대상 이름", Text(|o, v| o.target = v)),
    opt("k|key=", "Value=객체 이름", Text(|o, v| o.key = v)),
    opt("source-key=", "Value=원본 객체 이름", Text(|o, v| o.source_key = v)),
    opt("f|file=", "Value=객체의 경로", Text(|o, v| o.file_path = v)),
    opt("p|path=", "Value=객체의 경로", Text(|o, v| o.path = v)),
    opt("upload-id=", "Value=Upload Id.", Text(|o, v| o.upload_id = v)),
    opt("part-size=", "Value=Part Size(long type, ex> 50M)", TryText(|o, v| { o.part_size = size_to_long(v.as_deref())?; Ok(()) })),
    opt("start-byte=", "Value=Start Point(long type, ex> 50M)", TryText(|o, v| { o.start_byte = size_to_long(v.as_deref())?; Ok(()) })),
    opt("end-byte=", "Value=end Point(long type, ex> 50M)", TryText(|o, v| { o.end_byte = size_to_long(v.as_deref())?; Ok(()) })),
    opt("range-list=", "Value=Range List. ex> 8K,2M...", TryText(range_list)),
    opt("version-id=", "Value=Version Id.", Text(|o, v| o.version_id = v)),
    opt("storage-class=", "Value=STANDARD(default)/GLACIER", Text(|o, v| o.storage_class = v)),
    opt("versioning=", "Value=Enabled/Suspended/Off.", Text(|o, v| o.versioning = v)),
    opt("part-number=", "Value=int.", Int(|o, v| o.part_number = v)),
    opt("acl=", "Value=/private/public-read/public-read-write/authenticated-read/aws-exec-read/bucket-owner-read/bucket-owner-full-control/log-delivery-write", Text(|o, v| o.str_acl = v)),
    opt("prefix=", "Value=Prefix.", Text(|o, v| o.prefix = v)),
    opt("suffix=", "Value=Suffix.", Text(|o, v| o.suffix = v)),
    opt("delimiter=", "Value=Delimiter.", Text(|o, v| o.delimiter = v)),
    opt("marker=", "Value=Marker.", Text(|o, v| o.darker = v)),
    opt("continuation-token=", "Value=ContinuationToken.", Text(|o, v| o.continuation_token = v)),
    opt("days=", "Value=int.", Int(|o, v| o.days = v)),
    opt("years=", "Value=int.", Int(|o, v| o.years = v)),
    opt("date=", "Value=Date.", Text(|o, v| o.date = v)),
    opt("body=", "Value=Content.", Text(|o, v| o.body = v)),
    opt("tag=", "Value=Tag.", Text(|o, v| o.tags = v)),
    opt("tag-set=", "Value=Tags file.", Text(|o, v| o.tags = v)),
    opt("id=", "Value=Id.", Text(|o, v| o.id = v)),
    opt("max-keys=", "Value=MaxKeys. 최대 목록 갯수.", Int(|o, v| o.max_keys = v)),
    opt("print=", "Value=true/false. 목록 조회옵션. 결과 출력 여부를 결정(default = true)", Bool(|o, v| o.print = v)),
    opt("url=", "Value=URL. MainUser의 URL 값을 강제로 변경.", Text(|o, v| o.url = v)),
    opt("user=", "Value=User Name. 해당 유저를 MainUser로 변경.", Text(|o, v| o.user_name = v)),
    opt("check", "Check Mode", Set(|o| o.check = true)),
    opt("ownership=", "Value=BucketOwnerEnforced/BucketOwnerPreferred/ObjectWriter", Text(|o, v| o.ownership = v)),
    opt("lock-enable", "버킷에 lock 모드 설정", Set(|o| o.flag = true)),
    opt("lock-mode=", "Value=LockMode. COMPLIANCE/GOVERNANCE. 버킷에 lock 모드 설정", Text(|o, v| o.lock_mode = v)),
    opt("encryption-key=", "Value=EncryptionKey.", Text(|o, v| o.encryption_key = v)),
    opt("set-sse-s3=", "Value=true/false. 버킷에 sse-s3 설정 여부", Bool(|o, v| { o.flag = v; o.menu = MenuList::Encryption; })),
    opt("manual-upload", "create-multipart-upload + upload-part + complete-multipart-upload", Menu(MenuList::ManualUpload)),
    opt("clear=", "Value=true/false. 해당 유저의 모든 객체, True일 경우 버킷도 삭제.", Bool(|o, v| { o.flag = v; o.menu = MenuList::Clear; })),
    opt("start=", "Value= Start Count. Prepare의 시작 번호", Int(|o, v| o.start_count = if v > 0 { v } else { 0 })),
    opt("thread-prefix=", " Value=스레드 접두어", Text(|o, v| o.thread_prefix = v)),
    opt("thread=", "Value= Thread Count.", Int(|o, v| o.thread = if v > 0 { v } else { 1 })),
    opt("count=", "Value= File Count.", Int(|o, v| o.count = if v > 0 { v } else { 1 })),
    opt("size=", "Value=File Size. ex> 1k, 10M...", TryText(|o, v| { o.file_size = size_to_long(v.as_deref())?; Ok(()) })),
    opt("times=", "Value=Run Times. 초 단위로 테스트 수행시간 지정.", Int(|o, v| o.times = v)),
    opt("flag=", "Value=true/false.", Bool(|o, v| o.flag = v)),
    opt("bucket-type=", "Value= int. Bucket Type. [설정없음 = 0, 단일버킷업로드, 스레드당 버킷, 날짜당 폴더, Prefix, 현재시각 기준 생성]", Int(|o, v| o.bucket_type = EnumBucketTypes::from_int(v))),
    opt("service-type=", "Value=ServiceType. Replication, Lifecycle, Logging", Text(|o, v| o.service_type = v)),
    opt("address=", "Value=Ip", Text(|o, v| o.address = v)),
    opt("port=", "Value=Port", Int(|o, v| o.port = v)),
    opt("all", "목록을 불러올때 모든 목록을 불러오도록 설정", Set(|o| o.all = true)),
    opt("bulk", "1000개씩 묶어서 삭제할지 여부(--test-delete, --test-delete-version option)", Set(|o| o.bulk = true)),
    opt("bypass", "강제 삭제 모드", Set(|o| o.bypass = Some(true))),
    opt("quiet", "삭제 실패한 목록만 전달 받는 모드", Set(|o| o.flag = true)),
    opt("multipart", "테스트를 멀티파트로 업로드", Set(|o| o.multipart = true)),
    opt("not-empty", "업로드할 오브젝트의 내용물을 매번 생성하도록 지정.", Set(|o| o.random = true)),
    opt("debug", "디버그용 로그 출력.", Set(|o| o.debug = true)),
    opt("checksum", "체크섬 모드 활성화", Set(|o| o.checksum = true)),
    opt("checksum-type=", "체크섬 타입: CRC32, CRC32C, CRC64NVME, SHA1, SHA256, None", TryText(checksum_type)),
    opt("md5sum", "MD5 체크섬 계산 및 전송", Set(|o| o.md5sum = true)),
    opt("r|read=", "Read=value. 읽기 비율", Int(|o, v| o.read = v)),
    opt("w|write=", "Write=value. 쓰기 비율", Int(|o, v| o.write = v)),
    opt("d|delete=", "Delete=value. 삭제 비율", Int(|o, v| o.delete = v)),
    opt("bucket-clear=", " Value=버킷 이름. 버킷에 존재하는 모든 객체를 삭제. 버킷 보존.", Text(|o, v| { o.bucket_name = v; o.menu = MenuList::BucketClear; })),
    opt("current-clear=", " Value=버킷 이름. 버킷에 존재하는 모든 현재 버전 객체를 삭제. 버킷 보존.", Text(|o, v| { o.bucket_name = v; o.menu = MenuList::CurrentClear; })),
    opt("noncurrent-clear=", " Value=버킷 이름. 버킷에 존재하는 모든 이전 버전 객체를 삭제. 버킷 보존.", Text(|o, v| { o.bucket_name = v; o.menu = MenuList::NoncurrentClear; })),
    opt("marker-clear=", " Value=버킷 이름. 버킷에 존재하는 모든 Delete Marker 삭제. 버킷 보존.", Text(|o, v| { o.bucket_name = v; o.menu = MenuList::MarkerClear; })),
    opt("test-access-ips", "ip 접근제어 테스트", Menu(MenuList::AccessIpsTest)),
    opt("test-replication", "s3 복제 테스트", Menu(MenuList::ReplicationTest)),
    opt("test-list-object", "ListObject 테스트", Menu(MenuList::ListObjTest)),
    opt("test-range-copy", "원본 객체를 Range로 읽어 다운로드 한뒤 멀티파트 업로드", Menu(MenuList::RangeReadCopy)),
    opt("test-upload", "AWS 상위레벨 API를 사용하여 업로드 테스트", Menu(MenuList::UploadTest)),
    opt("test-download", "AWS 상위레벨 API를 사용하여 다운로드 테스트", Menu(MenuList::DownloadTest)),
    opt("test-prepare", "일정 갯수만큼 업로드. 이미 파일이 업로드 되어있다면 업로드 하지않음.", Menu(MenuList::Prepare)),
    opt("test-prepare-dir", "일정 갯수만큼 폴더(디렉터리 마커) 생성. 이미 존재하면 생성하지 않음.", Menu(MenuList::PrepareDir)),
    opt("test-new-put", "Prepare와 동일. 매 PutObject마다 새 S3Client 생성.", Menu(MenuList::NewPutTest)),
    opt("test-put", "PutObject 테스트", Menu(MenuList::PutTest)),
    opt("test-head", "Head 테스트", Menu(MenuList::HeadTest)),
    opt("test-get", "GetObject 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.", Menu(MenuList::GetTest)),
    opt("test-get-v2", "Sequential GetObject 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.", Menu(MenuList::GetTestV2)),
    opt("test-new-get", "test-get-v2와 동일. 매 GetObject마다 새 S3Client 생성.", Menu(MenuList::NewGetTest)),
    opt("test-get-v3", "ListObject => GetObject 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.", Menu(MenuList::GetTestV3)),
    opt("test-get-all", "Sequential GetObject 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.", Menu(MenuList::GetTestAll)),
    opt("test-delete", "DeleteObject 테스트", Menu(MenuList::DeleteTest)),
    opt("test-delete-v2", "Prepare로 업로드한 오브젝트를 순차적으로 삭제하고 모두 삭제하면 종료하는 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.", Menu(MenuList::DeleteTestV2)),
    opt("test-new-del", "test-delete-v2와 동일. 매 DeleteObject마다 새 S3Client 생성.", Menu(MenuList::NewDelTest)),
    opt("test-delete-version", "DeleteObjectVersion 테스트", Menu(MenuList::DeleteTestVersion)),
    opt("test-delete-directory", "ListObject를 이용하여 디렉토리를 삭제하는 테스트", Menu(MenuList::DeleteDirectoryTest)),
    opt("test-mix", "PutObject, GetObject를 설정한 비율로 테스트 설정한 시간만큼 동작. GetObject는 업로드한 파일 목록 내에서 무작위로 실행.", Menu(MenuList::MixTest)),
    opt("test-mix-v2", "Read, Write를 설정한 갯수만큼 스레드를 생성하여 테스트. Prepare로 테스트 환경을 구성하고 진행해야 함. GetObject는 업로드한 파일 목록 내에서 무작위로 실행.", Menu(MenuList::MixV2Test)),
    opt("test-new-mix", "test-all과 동일. 매 Put/Get/Delete마다 새 S3Client 생성.", Menu(MenuList::NewMixTest)),
    opt("test-put-get", "PutObject, GetObject를 순차로 테스트.", Menu(MenuList::PutGetTest)),
    opt("test-all", "PutObject, GetObject, DeleteObject를 순차로 실행하는 테스트.", Menu(MenuList::AllTest)),
    opt("test-full", "모든 테스트를 순차로 실행하는 테스트.", Menu(MenuList::FullTest)),
    opt("test-io", "특정 폴더의 내용을 모두 업로드한뒤 다운로드하여 md5sum으로 비교하는 테스트.", Menu(MenuList::IoTest)),
    opt("test-mover", "ifsMover용 테스트.", Menu(MenuList::MoverTest)),
    opt("test-put-tag", "PutObject시 Tag를 포함하여 업로드", Menu(MenuList::PutTagTest)),
    opt("test-compare=", "Value=true(Alt User)/false(Main User Only). 원본과 대상 버킷의 모든 객체가 동일한지 비교.(default = true)", Bool(|o, v| { o.another = v; o.menu = MenuList::CompareTest; })),
    opt("test-find-tag=", "test-put-tag에서 업로드한 Tag를 조회하는 테스트.", Text(|o, v| { o.tags = v; o.menu = MenuList::FindTagTest; })),
    opt("test-multi-delete", "단일 버킷, DeleteObject만 하는 테스트", Menu(MenuList::MultiDeleteTest)),
    opt("test-used-size", "버킷의 UsedSize가 일치하는지 확인하는 테스트", Menu(MenuList::UsedSizeTest)),
    opt("test-duplicate", "중복 업로드 테스트", Menu(MenuList::DuplicateTest)),
    opt("test-multi-upload", "동시 업로드 테스트", Menu(MenuList::MultiUploadTest)),
    opt("test-directory-download", "폴더 다운로드 테스트", Menu(MenuList::DirectoryDownloadTest)),
    opt("test-file-list=", "value=File List Path. 파일목록을 읽어와서 멀티 다운로드 테스트", Text(|o, v| { o.menu = MenuList::FileListDownloadTest; o.path = v; })),
    opt("test-range-read", "단일 오브젝트를 Range 단위로 순차 읽기", Menu(MenuList::RangeReadTest)),
    opt("test-multipart-upload", "멀티파트업로드 테스트", Menu(MenuList::MultipartUploadTest)),
    opt("test-multipart-upload-v2", "멀티파트업로드 및 다운로드 테스트", Menu(MenuList::MultipartUploadAndDownloadTest)),
    opt("test-multi-system-list", "다중 시스템 업로드 테스트", Menu(MenuList::MultiSystemListTest)),
    opt("test-multi-system-upload", "다중 시스템 업로드 테스트", Menu(MenuList::MultiSystemUploadTest)),
    opt("test-multi-system-up-down", "다중 시스템 업로드, 다운로드 테스트", Menu(MenuList::MultiSystemUpDownTest)),
    opt("test-multi-system-all", "다중 시스템 업로드, 다운로드, 삭제 테스트", Menu(MenuList::MultiSystemAllTest)),
    opt("test-compare-lifecycle", "만료기한 확인", Menu(MenuList::LifecycleTest)),
    opt("test-aws", "AWS Multi test", Menu(MenuList::AWSTest)),
    opt("target-path=", "로컬 테스트 타겟 경로", Text(|o, v| o.target_path = v)),
    opt("test-local-prepare", "로컬 파일시스템 Prepare 테스트", Menu(MenuList::LocalPrepareTest)),
    opt("test-local-put", "로컬 파일시스템 PutOnly 테스트", Menu(MenuList::LocalPutTest)),
    opt("test-local-get", "로컬 파일시스템 랜덤 GetOnly 테스트", Menu(MenuList::LocalGetTest)),
    opt("test-local-get-v2", "로컬 파일시스템 순차 GetOnly 테스트", Menu(MenuList::LocalGetTestV2)),
    opt("test-local-put-get", "로컬 파일시스템 PutGet 테스트", Menu(MenuList::LocalPutGetTest)),
    opt("test-local-delete", "로컬 파일시스템 Delete 테스트", Menu(MenuList::LocalDeleteTest)),
    opt("test-local-multipart-prepare", "로컬 파일시스템 멀티파트 Prepare 테스트", Menu(MenuList::LocalMultipartPrepareTest)),
    opt("test-local-multipart-put", "로컬 파일시스템 멀티파트 PutOnly 테스트", Menu(MenuList::LocalMultipartPutTest)),
    opt("test-local-multipart-get", "로컬 파일시스템 멀티파트 랜덤 GetOnly 테스트", Menu(MenuList::LocalMultipartGetTest)),
    opt("test-local-multipart-get-v2", "로컬 파일시스템 멀티파트 순차 GetOnly 테스트", Menu(MenuList::LocalMultipartGetTestV2)),
    opt("test-local-multipart-put-get", "로컬 파일시스템 멀티파트 PutGet 테스트", Menu(MenuList::LocalMultipartPutGetTest)),
];
