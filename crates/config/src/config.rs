//! TESTCore `Data/Config/Config.cs` 이식: INI 파일에서 섹션별 설정 객체를 만든다.

use std::path::Path;

use serde::Serialize;

use crate::enum_bucket_types::EnumBucketTypes;
use crate::external::{AccessIpsConfig, JenkinsConfig, MoverConfig, PortalConfig};
use crate::ini::{IniError, IniFile};
use crate::sections::{
    CompareConfig, DbConfig, DuplicateConfig, ListObjConfig, MainConfig, MultiSystemConfig,
    MultiSystemView, UpDownConfig, UsedSizeConfig,
};
use crate::user_data::UserData;
use crate::util::{UtilError, try_parse_bool, try_parse_i32};

/// 섹션 이름, 키 이름, 사용법 문자열(원본 `Config`의 `const string`들).
pub mod keys {
    // Global
    pub const STR_FILE_COUNT: &str = "FileCount";
    pub const USAGE_FILE_COUNT: &str = "FileCount = [int] : 스레드 당 생성할 파일 개수\n";
    pub const STR_THREAD_COUNT: &str = "ThreadCount";
    pub const USAGE_THREAD_COUNT: &str = "ThreadCount = [int] : 쓰레드 개수\n";
    pub const STR_SOURCE_BUCKET: &str = "SourceBucket";
    pub const USAGE_SOURCE_BUCKET: &str = "SourceBucket = [string] : 원본 버킷명\n";
    pub const STR_TARGET_BUCKET: &str = "TargetBucket";
    pub const USAGE_TARGET_BUCKET: &str = "TargetBucket = [string] : 대상 버킷명\n";
    pub const STR_URL: &str = "URL";
    pub const USAGE_URL: &str = "URL = [string] : URL\n";
    pub const STR_HOST: &str = "Host";
    pub const USAGE_HOST: &str = "Host = [string] : 호스트\n";
    pub const STR_PORT: &str = "Port";
    pub const USAGE_PORT: &str = "Port = [int] : 포트\n";
    pub const STR_USER: &str = "User";
    pub const USAGE_USER: &str = "User = [string] : 사용자명\n";
    pub const STR_PASSWORD: &str = "Password";
    pub const USAGE_PASSWORD: &str = "Password = [string] : 비밀번호\n";
    pub const STR_FILE_SIZE: &str = "FileSize";
    pub const USAGE_FILE_SIZE: &str = "FileSize = [string] : 생성할 파일 크기(ex> 128K, 10M, 1G)\n";
    pub const STR_PART_SIZE: &str = "PartSize";
    pub const USAGE_PART_SIZE: &str =
        "PartSize = [string] : 멀티파트 업로드 시 파트 크기(ex> 128K, 10M, 1G)\n";
    pub const STR_BUCKET_PREFIX: &str = "BucketPrefix";
    pub const USAGE_BUCKET_PREFIX: &str = "BucketPrefix = [string] : 버킷 접두어\n";
    pub const STR_BUCKET_TYPE: &str = "BucketType";
    // Default
    pub const STR_DEFAULT_DATA: &str = "Default";
    pub const STR_DEFAULT_BUCKET_NAME: &str = "BucketName";
    pub const USAGE_DEFAULT_BUCKET_NAME: &str = "BucketName = [string] : 버킷 이름\n";
    pub const STR_DEFAULT_THREAD_PREFIX: &str = "ThreadPrefix";
    pub const USAGE_DEFAULT_THREAD_PREFIX: &str =
        "ThreadPrefix = [string] : 생성할 스레드 접두어\n";
    pub const STR_DEFAULT_OBJECT_PREFIX: &str = "ObjectPrefix";
    pub const USAGE_DEFAULT_OBJECT_PREFIX: &str =
        "ObjectPrefix = [string] : 생성할 오브젝트 접두어\n";
    pub const STR_DEFAULT_FILE_PATH: &str = "FilePath";
    pub const STR_DEFAULT_RETRY: &str = "Retry";
    pub const USAGE_DEFAULT_RETRY: &str = "Retry = [int] : 재시도 횟수\n";
    pub const STR_DEFAULT_TARGET_PATH: &str = "TargetPath";
    pub const USAGE_DEFAULT_TARGET_PATH: &str = "TargetPath = [string] : 타겟 경로\n";
    pub const STR_DEFAULT_IS_ADMIN: &str = "IsAdmin";
    pub const USAGE_DEFAULT_IS_ADMIN: &str = "IsAdmin = [bool] : 관리자 모드 활성화 여부\n";
    // ListObj
    pub const STR_TEST_LIST_OBJECT: &str = "ListObj";
    pub const STR_LIST_OBJ_PREFIX: &str = "Prefix";
    pub const STR_LIST_OBJ_TIMES: &str = "Times";
    pub const STR_LIST_OBJ_PREPARE: &str = "Prepare";
    // UpDown
    pub const STR_UP_DOWN_DATA: &str = "UpDown";
    pub const STR_UP_DOWN_READ_RATIO: &str = "ReadRatio";
    pub const STR_UP_DOWN_WRITE_RATIO: &str = "WriteRatio";
    pub const STR_UP_DOWN_DELETE_RATIO: &str = "DeleteRatio";
    pub const STR_UP_DOWN_TIMES: &str = "Times";
    pub const USAGE_UP_DOWN_TIMES: &str = "Times = [int] : 테스트 할 시간(sec)\n";
    pub const USAGE_UP_DOWN_BUCKET_TYPE: &str = "BucketType[int] : 버킷 생성 방식(0:설정없음, 1:단일버킷 업로드, 2: 쓰레드당 버킷 1개로 업로드, 3:시간당 하나의 폴더, 4:단일 버킷. 오브젝트 prefix에 {Random:10} 형식의 변수 추가 가능)\n";
    pub const STR_DIVISION_COUNT: &str = "DivisionCount";
    pub const USAGE_DIVISION_COUNT: &str = "DivisionCount[int] : 오브젝트의 폴더 분리 분기\n";
    pub const STR_ETAG_CHECK: &str = "ETagCheck";
    pub const USAGE_ETAG_CHECK: &str = "ETagCheck = [bool] : ETag 비교 여부\n";
    pub const STR_USE_CHUNK_ENCODING: &str = "UseChunkEncoding";
    pub const USAGE_USE_CHUNK_ENCODING: &str =
        "UseChunkEncoding = [bool] : Chunk Encoding 사용 여부\n";
    // Compare
    pub const STR_COMPARE_CHECKSUM_CHECK: &str = "ChecksumCheck";
    pub const STR_TEST_COMPARE: &str = "Compare";
    pub const STR_COMPARE_DELETE_MARKER: &str = "DeleteMarker";
    pub const STR_COMPARE_VERSION_CHECK: &str = "VersionCheck";
    pub const STR_COMPARE_METADATA_CHECK: &str = "MetadataCheck";
    pub const STR_COMPARE_REPLICATION_CHECK: &str = "ReplicationCheck";
    pub const STR_COMPARE_TAG_CHECK: &str = "TagCheck";
    // Mover
    pub const STR_MOVER_DATA: &str = "Mover";
    pub const STR_MOVER_MAX_SIZE: &str = "MaxFileSize";
    pub const STR_MOVER_MAX_SIZE_USAGE: &str = "MaxFileSize = [int] : 이동할 파일의 최대 크기\n";
    // UsedSize
    pub const STR_USED_SIZE_DATA: &str = "UsedSize";
    // Duplicate
    pub const STR_DUPLICATE_DATA: &str = "Duplicate";
    pub const STR_DUPLICATE_OBJECT_PATH: &str = "ObjectPath";
    pub const USAGE_DUPLICATE_OBJECT_PATH: &str = "ObjectPath = [string] : 오브젝트 경로\n";
    pub const STR_DUPLICATE_LOOP_COUNT: &str = "LoopCount";
    pub const USAGE_DUPLICATE_LOOP_COUNT: &str = "LoopCount = [int] : 반복 횟수\n";
    pub const STR_DUPLICATE_OBJECT_COUNT: &str = "ObjectCount";
    pub const USAGE_DUPLICATE_OBJECT_COUNT: &str = "ObjectCount = [int] : 오브젝트 생성 개수\n";
    // AccessIps
    pub const STR_ACCESS_IPS: &str = "AccessIps";
    pub const STR_ACCESS_IPS_ALL_FAILED: &str = "AllFailed";
    pub const USAGE_ALL_FAILED: &str = "AllFailed = [bool] : 모든 테스트가 실패해야 하는지 여부\n";
    pub const STR_ACCESS_IPS_TEST_LIST_PATH: &str = "TestListPath";
    pub const USAGE_TEST_LIST_PATH: &str = "TestListPath = [string] : 테스트 리스트 파일 경로\n";
    pub const STR_ACCESS_IPS_IPS_URLS: &str = "S3URLs";
    pub const USAGE_S3_URLS: &str = "S3URLs = [string] : S3 URL\n";
    pub const STR_ACCESS_IPS_VOLUME_NAME: &str = "Volume";
    pub const USAGE_VOLUME_NAME: &str = "Volume = [string] : 볼륨 이름\n";
    pub const STR_ACCESS_IPS_MAIN_USER_NAME: &str = "MainUser";
    pub const USAGE_MAIN_USER_NAME: &str = "MainUser = [string] : Main User Name\n";
    pub const STR_ACCESS_IPS_SUB_USER_NAME: &str = "SubUser";
    pub const USAGE_SUB_USER_NAME: &str = "SubUser = [string] : Sub User Name\n";
    // DB
    pub const STR_DB: &str = "DB";
    pub const STR_DB_DATABASE: &str = "Database";
    pub const USAGE_DB_DATABASE: &str = "Database = [string] : Database\n";
    // User
    pub const STR_MAIN_USER: &str = "Main User";
    pub const STR_ALT_USER: &str = "Alt User";
    pub const STR_THIRD_USER: &str = "Third User";
    pub const STR_ACCESS_KEY: &str = "AccessKey";
    pub const USAGE_ACCESS_KEY: &str = "AccessKey = [string] : AccessKey\n";
    pub const STR_SECRET_KEY: &str = "SecretKey";
    pub const USAGE_SECRET_KEY: &str = "SecretKey = [string] : SecretKey\n";
    pub const STR_REGION_NAME: &str = "RegionName";
    pub const USAGE_REGION_NAME: &str = "RegionName = [string] : RegionName\n";
    // Portal
    pub const STR_PORTAL: &str = "Portal";
    pub const STR_API_KEY: &str = "ApiKey";
    // Jenkins
    pub const STR_JENKINS: &str = "Jenkins";
    pub const STR_JENKINS_TEST_NAME: &str = "JenkinsTestName";
    pub const USAGE_JENKINS_TEST_NAME: &str = "JenkinsTestName = [string] : Jenkins Test Name\n";
    pub const STR_JENKINS_PORT: &str = "JenkinsPort";
    pub const USAGE_JENKINS_PORT: &str = "JenkinsPort = [int] : Jenkins Port\n";
    // MultiSystemUpload
    pub const STR_MULTI_SYSTEM_UPLOAD: &str = "MultiSystem";
    pub const STR_API_GW_URL: &str = "MultiGatewayURL";
    pub const STR_PROXY_URL: &str = "OldSystemURL";
    pub const STR_GW_URL: &str = "NewSystemURL";
}

use keys::*;

/// 설정 로드 오류.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// 설정 파일이 없다(`FileNotFoundException`).
    #[error("Config file not found : {0}")]
    NotFound(String),
    /// 그 밖의 파일 읽기 오류.
    #[error(transparent)]
    Ini(IniError),
    /// 값 변환 오류(크기 문자열 등).
    #[error(transparent)]
    Util(#[from] UtilError),
}

/// 설정 파일 전체. JSON 속성 이름은 원본 `Config`의 공개 속성과 같다(`Ini` 필드는 직렬화 대상이 아니다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub main: MainConfig,
    pub list_obj: ListObjConfig,
    pub up_down: UpDownConfig,
    pub mover: MoverConfig,
    pub compare: CompareConfig,
    pub used_size: UsedSizeConfig,
    pub duplicate: DuplicateConfig,
    pub access_ips: AccessIpsConfig,
    pub db: DbConfig,
    pub portal: PortalConfig,
    pub jenkins: JenkinsConfig,
    /// 사용자 정보는 `main_user`를 따른다([`MultiSystemConfig`] 참고).
    multi_system_upload: MultiSystemConfig,
    pub main_user: UserData,
    pub alt_user: UserData,
}

/// `Config`의 JSON 모양. `MultiSystemUpload`는 `main_user`를 반영해 만든다.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ConfigJson<'a> {
    main: &'a MainConfig,
    list_obj: &'a ListObjConfig,
    up_down: &'a UpDownConfig,
    mover: &'a MoverConfig,
    compare: &'a CompareConfig,
    used_size: &'a UsedSizeConfig,
    duplicate: &'a DuplicateConfig,
    access_ips: &'a AccessIpsConfig,
    #[serde(rename = "DB")]
    db: &'a DbConfig,
    portal: &'a PortalConfig,
    jenkins: &'a JenkinsConfig,
    multi_system_upload: MultiSystemView<'a>,
    main_user: &'a UserData,
    alt_user: &'a UserData,
}

impl Serialize for Config {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ConfigJson {
            main: &self.main,
            list_obj: &self.list_obj,
            up_down: &self.up_down,
            mover: &self.mover,
            compare: &self.compare,
            used_size: &self.used_size,
            duplicate: &self.duplicate,
            access_ips: &self.access_ips,
            db: &self.db,
            portal: &self.portal,
            jenkins: &self.jenkins,
            multi_system_upload: self.multi_system_view(),
            main_user: &self.main_user,
            alt_user: &self.alt_user,
        }
        .serialize(serializer)
    }
}

impl Config {
    /// `GetConfig(fileName, mainUserName)`: 설정 파일을 읽어 섹션별 설정 객체를 채운다.
    ///
    /// 실패하면 원본처럼 로그를 남긴다(파일이 없으면 `Config file not found : {fileName}`, 그 외는 오류 내용).
    /// `main_user_name`이 `None`이면 `Main User` 섹션을 쓴다(빈 문자열은 그 이름의 섹션을 쓴다).
    pub fn load(path: impl AsRef<Path>, main_user_name: Option<&str>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let result = IniFile::load(path)
            .map_err(|e| match e {
                IniError::NotFound(_) => ConfigError::NotFound(path.display().to_string()),
                other => ConfigError::Ini(other),
            })
            .and_then(|ini| Self::from_ini(&ini, main_user_name));
        if let Err(e) = &result {
            tracing::error!("{e}");
        }
        result
    }

    /// 파싱된 INI에서 설정을 만든다. 섹션 읽기 순서와 실패 지점은 원본 `GetConfig`와 같다.
    pub fn from_ini(ini: &IniFile, main_user_name: Option<&str>) -> Result<Self, ConfigError> {
        let r = Reader { ini };
        let main_user = r.user(main_user_name.unwrap_or(STR_MAIN_USER));
        let alt_user = r.user(STR_ALT_USER);

        let main = r.default_data()?;
        let list_obj = r.list_obj_data();
        let compare = r.compare_data();
        let up_down = r.up_down_data();
        let mover = r.mover_data()?;
        let used_size = r.used_size_data()?;
        let duplicate = r.duplicate_data();
        let access_ips = r.access_ips_data();
        let db = r.db_config();
        let portal = r.portal_config();
        let jenkins = r.jenkins_config();
        let multi_system_upload = r.multi_system_upload_config();

        Ok(Self {
            main,
            list_obj,
            up_down,
            mover,
            compare,
            used_size,
            duplicate,
            access_ips,
            db,
            portal,
            jenkins,
            multi_system_upload,
            main_user,
            alt_user,
        })
    }

    /// `MultiSystemUpload`. 사용자 정보는 `main_user`를 따른다.
    pub fn multi_system_upload(&self) -> &MultiSystemConfig {
        &self.multi_system_upload
    }

    /// `MultiSystemUpload`와 `main_user`를 묶은 보기(직렬화나 `MultiGateway` 등 접근용).
    pub fn multi_system_view(&self) -> MultiSystemView<'_> {
        self.multi_system_upload.view(&self.main_user)
    }

    /// `Config.ToString()`: 들여쓰기가 적용된 JSON.
    pub fn to_json_string(&self) -> String {
        testrust_common::to_dotnet_json(self)
    }
}

/// `ReadKeyToString`/`ReadKeyToInt`/`ReadKeyToBoolean`과 `Read*Data` 메서드들.
struct Reader<'a> {
    ini: &'a IniFile,
}

impl Reader<'_> {
    /// 앞뒤 공백을 제거한 문자열. 없는 키는 빈 문자열.
    fn string(&self, section: &str, key: &str) -> String {
        self.ini.text(section, key).trim().to_string()
    }

    /// `int.TryParse`, 실패하면 -1.
    fn int(&self, section: &str, key: &str) -> i32 {
        try_parse_i32(self.ini.text(section, key)).unwrap_or(-1)
    }

    /// `bool.TryParse`, 실패하면 false.
    fn boolean(&self, section: &str, key: &str) -> bool {
        try_parse_bool(self.ini.text(section, key)).unwrap_or(false)
    }

    fn default_data(&self) -> Result<MainConfig, ConfigError> {
        let s = STR_DEFAULT_DATA;
        Ok(MainConfig::new(
            &self.string(s, STR_DEFAULT_BUCKET_NAME),
            &self.string(s, STR_DEFAULT_THREAD_PREFIX),
            &self.string(s, STR_DEFAULT_OBJECT_PREFIX),
            &self.string(s, STR_DEFAULT_FILE_PATH),
            &self.string(s, STR_FILE_SIZE),
            &self.string(s, STR_PART_SIZE),
            self.int(s, STR_DEFAULT_RETRY),
            &self.string(s, STR_DEFAULT_TARGET_PATH),
            self.boolean(s, STR_DEFAULT_IS_ADMIN),
        )?)
    }

    fn list_obj_data(&self) -> ListObjConfig {
        let s = STR_TEST_LIST_OBJECT;
        ListObjConfig::new(
            &self.string(s, STR_LIST_OBJ_PREFIX),
            self.boolean(s, STR_LIST_OBJ_PREPARE),
            self.int(s, STR_FILE_COUNT),
            self.int(s, STR_LIST_OBJ_TIMES),
            self.int(s, STR_THREAD_COUNT),
        )
    }

    fn up_down_data(&self) -> UpDownConfig {
        let s = STR_UP_DOWN_DATA;
        UpDownConfig::new(
            self.int(s, STR_UP_DOWN_READ_RATIO),
            self.int(s, STR_UP_DOWN_WRITE_RATIO),
            self.int(s, STR_UP_DOWN_DELETE_RATIO),
            self.int(s, STR_THREAD_COUNT),
            self.int(s, STR_FILE_COUNT),
            self.int(s, STR_DIVISION_COUNT),
            self.int(s, STR_UP_DOWN_TIMES),
            EnumBucketTypes(self.int(s, STR_BUCKET_TYPE)),
            self.boolean(s, STR_ETAG_CHECK),
            self.boolean(s, STR_USE_CHUNK_ENCODING),
        )
    }

    fn mover_data(&self) -> Result<MoverConfig, ConfigError> {
        let s = STR_MOVER_DATA;
        Ok(MoverConfig::new(
            &self.string(s, STR_URL),
            &self.string(s, STR_USER),
            &self.string(s, STR_SOURCE_BUCKET),
            &self.string(s, STR_TARGET_BUCKET),
            self.int(s, STR_FILE_COUNT),
            &self.string(s, STR_MOVER_MAX_SIZE),
        )?)
    }

    fn compare_data(&self) -> CompareConfig {
        let s = STR_TEST_COMPARE;
        CompareConfig::new(
            &self.string(s, STR_SOURCE_BUCKET),
            &self.string(s, STR_TARGET_BUCKET),
            self.boolean(s, STR_COMPARE_DELETE_MARKER),
            self.boolean(s, STR_ETAG_CHECK),
            self.boolean(s, STR_COMPARE_CHECKSUM_CHECK),
            self.boolean(s, STR_COMPARE_VERSION_CHECK),
            self.boolean(s, STR_COMPARE_METADATA_CHECK),
            self.boolean(s, STR_COMPARE_REPLICATION_CHECK),
            self.boolean(s, STR_COMPARE_TAG_CHECK),
        )
    }

    fn used_size_data(&self) -> Result<UsedSizeConfig, ConfigError> {
        let s = STR_USED_SIZE_DATA;
        Ok(UsedSizeConfig::new(
            &self.string(s, STR_BUCKET_PREFIX),
            &self.string(s, STR_FILE_SIZE),
            &self.string(s, STR_PART_SIZE),
        )?)
    }

    fn duplicate_data(&self) -> DuplicateConfig {
        let s = STR_DUPLICATE_DATA;
        DuplicateConfig::new(
            &self.string(s, STR_DUPLICATE_OBJECT_PATH),
            self.int(s, STR_DUPLICATE_LOOP_COUNT),
            self.int(s, STR_DUPLICATE_OBJECT_COUNT),
        )
    }

    fn access_ips_data(&self) -> AccessIpsConfig {
        let s = STR_ACCESS_IPS;
        AccessIpsConfig::from_csv(
            &self.string(s, STR_BUCKET_PREFIX),
            self.boolean(s, STR_ACCESS_IPS_ALL_FAILED),
            &self.string(s, STR_ACCESS_IPS_TEST_LIST_PATH),
            &self.string(s, STR_JENKINS_TEST_NAME),
            &self.string(s, STR_ACCESS_IPS_IPS_URLS),
            &self.string(s, STR_ACCESS_IPS_VOLUME_NAME),
            &self.string(s, STR_ACCESS_IPS_MAIN_USER_NAME),
            &self.string(s, STR_ACCESS_IPS_SUB_USER_NAME),
            &self.string(s, STR_PASSWORD),
        )
    }

    fn db_config(&self) -> DbConfig {
        let s = STR_DB;
        DbConfig::new(
            &self.string(s, STR_HOST),
            self.int(s, STR_PORT),
            &self.string(s, STR_USER),
            &self.string(s, STR_PASSWORD),
            &self.string(s, STR_DB_DATABASE),
        )
    }

    /// 원본 `ReadJenkinsConfig`는 `Port` 키를 `jenkinsPort`에, `JenkinsPort` 키를 `dbPort`에 넣는다
    /// (키 이름과 반대로 보이지만 원본 그대로 옮긴다).
    fn jenkins_config(&self) -> JenkinsConfig {
        let s = STR_JENKINS;
        JenkinsConfig::new(
            &self.string(s, STR_HOST),
            self.int(s, STR_PORT),
            self.int(s, STR_JENKINS_PORT),
            &self.string(s, STR_USER),
            &self.string(s, STR_PASSWORD),
            &self.string(s, STR_DB_DATABASE),
        )
    }

    fn portal_config(&self) -> PortalConfig {
        let s = STR_PORTAL;
        PortalConfig::new(&self.string(s, STR_URL), &self.string(s, STR_API_KEY))
    }

    fn multi_system_upload_config(&self) -> MultiSystemConfig {
        let s = STR_MULTI_SYSTEM_UPLOAD;
        MultiSystemConfig::new(
            EnumBucketTypes(self.int(s, STR_BUCKET_TYPE)),
            self.int(s, STR_THREAD_COUNT),
            self.int(s, STR_FILE_COUNT),
            &self.string(s, STR_API_GW_URL),
            &self.string(s, STR_PROXY_URL),
            &self.string(s, STR_GW_URL),
        )
    }

    fn user(&self, section: &str) -> UserData {
        UserData::new(
            self.string(section, STR_URL),
            self.string(section, STR_REGION_NAME),
            self.string(section, STR_ACCESS_KEY),
            self.string(section, STR_SECRET_KEY),
        )
    }
}
