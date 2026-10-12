//! `Test/AccessIpsTest.cs`: Portal로 볼륨·사용자를 준비한 뒤 접근 허용 IP 설정에 따라 S3 접근(ListBuckets·ListObjects)이
//! 허용·차단되는지 확인하고 정리한다. 테스트 목록 파일 파서(`AccessIpsConfig.GetTestList`·`AccessIpsTestData.Parse`)와
//! `AccessIpsBucketList`도 여기에 둔다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - Portal 호출 실패는 예외로 던져져(`PortalXxxException`) 정리(사용자·볼륨 삭제)를 건너뛰고 최상위가 로그를 남긴다.
//! - 접근 확인(`AccessCheck`·`AccessBucketCheck`)은 예외를 모두 삼켜 `false`로 본다. 응답 상태는 보지 않는다.
//! - TESTCore c83e35f에서 `DeleteAccessIp(volume, user, ip)`를 `DeleteAccessIp(volume, user)`로 고친 판이다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - `TestBucket`은 하위 버킷 4개를 모두 `mainClients[0]`으로 만들지만, 지울 때 Sub 버킷은 `subClients[0]`으로 지운다.
//! - 버킷 이름은 `GetNewBucket(client, BucketPrefix)`(무작위 10자)다. 그래서 요청 경로가 실행마다 다르다.
//! - `S3URL`이 비어 있고 테스트 목록이 있으면 `TestBucket`의 `mainClients[0]`에서 `ArgumentOutOfRangeException`이 난다.
//! - 테스트 목록의 주소는 앞뒤 공백을 자르지 않는다(`Split(',')`한 첫 조각 그대로). `bool.Parse`만 공백을 무시한다.

use testrust_clients::portal::{EnumVolumeStatus, PortalError, PortalManager};
use testrust_config::{AccessIpsConfig, PortalConfig, UserData};
use testrust_s3::S3Client;
use tracing::{error, info};

use testrust_common::dotnet_format::bool_text as dotnet_bool;

use crate::ScenarioError;
use crate::input::{null_reference, read_all_text};
use crate::util::get_new_bucket;

/// 원본 `DEFAULT_VOLUME_SIZE`.
const DEFAULT_VOLUME_SIZE: u64 = 1_000_000_000;

/// 원본 `AccessIpsTestData`: 접근 허용 IP 하나와 기대 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessIpsTestData {
    pub address: String,
    pub pass: bool,
}

/// `bool.Parse(value)`: 앞뒤 공백을 무시하고 `True`/`False`를 대소문자 없이 읽는다.
fn parse_bool(value: &str) -> Result<bool, ScenarioError> {
    let trimmed = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if trimmed.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if trimmed.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(ScenarioError::new(
            "System.FormatException",
            format!("String '{value}' was not recognized as a valid Boolean."),
        ))
    }
}

impl AccessIpsTestData {
    /// 원본 `Parse(line)`: `주소,성공여부`.
    pub fn parse(line: &str) -> Result<Self, ScenarioError> {
        let split: Vec<&str> = line.split(',').collect();
        if split.len() != 2 {
            return Err(PortalError::InvalidAccessIpsTestData(line.to_string()).into());
        }
        Ok(Self {
            address: split[0].to_string(),
            pass: parse_bool(split[1])?,
        })
    }
}

/// `File.ReadAllLines`: `\r\n`·`\n`·`\r`로 줄을 나누고, 끝의 빈 조각은 버린다.
fn read_all_lines(path: &str) -> Result<Vec<String>, ScenarioError> {
    let text = read_all_text(path)?;
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut line));
            }
            '\n' => lines.push(std::mem::take(&mut line)),
            _ => line.push(c),
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    Ok(lines)
}

/// 원본 `AccessIpsConfig.GetTestList()`: 빈 줄과 `#`으로 시작하는 줄은 건너뛴다.
pub fn get_test_list(config: &AccessIpsConfig) -> Result<Vec<AccessIpsTestData>, ScenarioError> {
    let mut test_list = Vec::new();
    for line in read_all_lines(&config.test_list_path)? {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        test_list.push(AccessIpsTestData::parse(&line)?);
    }
    Ok(test_list)
}

/// 원본 `AccessIpsBucketList`.
struct AccessIpsBucketList {
    main_a: String,
    main_b: String,
    sub_a: String,
    sub_b: String,
}

/// 원본 `AccessIpsTest`.
pub struct AccessIpsTest {
    access_ips: AccessIpsConfig,
    portal: PortalManager,
    main_clients: Vec<S3Client>,
    sub_clients: Vec<S3Client>,
}

/// 원본 `AccessCheck`: `ListBuckets()`가 예외 없이 끝나는지.
async fn access_check(client: &S3Client) -> bool {
    client.list_buckets(None, 10000, None).await.is_ok()
}

/// 원본 `AccessBucketCheck`: `ListObjects(bucketName)`가 예외 없이 끝나는지.
async fn access_bucket_check(client: &S3Client, bucket_name: &str) -> bool {
    client
        .list_objects(bucket_name, None, None, 1000, None)
        .await
        .is_ok()
}

impl AccessIpsTest {
    /// 원본 `new AccessIpsTest(portal, accessIps)`. API 키 형식이 틀리면 `FormatException`이다.
    pub fn new(portal: &PortalConfig, access_ips: &AccessIpsConfig) -> Result<Self, ScenarioError> {
        let portal = PortalManager::new(portal.clone()).map_err(PortalError::from)?;
        Ok(Self {
            access_ips: access_ips.clone(),
            portal,
            main_clients: Vec::new(),
            sub_clients: Vec::new(),
        })
    }

    /// Portal에 접속해 볼륨·유저를 준비한 뒤 IP별 접근 제어(User/Bucket) 테스트를 수행하고 리소스를 정리한다.
    pub async fn start(&mut self) -> Result<(), ScenarioError> {
        info!("AccessIpsTest Start");

        // Portal 접속 확인
        if !self.portal.health_check().await? {
            error!("PortalManager HealthCheck Fail");
            return Ok(());
        }
        info!("PortalManager HealthCheck Success");

        // Read Test List
        let test_list = get_test_list(&self.access_ips)?;

        // 초기화
        // 0. AccessIps 초기화
        let ips = self.access_ips.clone();
        self.portal
            .delete_access_ip(&ips.volume_name, &ips.main_user_name, None)
            .await?;
        self.portal
            .delete_access_ip(&ips.volume_name, &ips.sub_user_name, None)
            .await?;

        // 볼륨 정보 가져오기
        let volume = self.portal.get_volume(&ips.volume_name).await?;
        match volume {
            // 1. Volume이 없을 경우 생성
            None => {
                self.portal
                    .create_volume(&ips.volume_name, DEFAULT_VOLUME_SIZE, &ips.password)
                    .await?;
                info!("Volume {} create", ips.volume_name);

                // Volume 시작
                self.portal.start_volume(&ips.volume_name).await?;
                info!("Volume {} start", ips.volume_name);
            }
            // 2. volume 가 시작되지 않았다면 시작
            Some(volume) if volume.status != EnumVolumeStatus::Online => {
                self.portal.start_volume(&ips.volume_name).await?;
                info!("Volume {} start", ips.volume_name);
            }
            Some(_) => {}
        }

        // 3. Main Credential 가져오기
        let main = self
            .get_user_credential(&ips.volume_name, &ips.main_user_name, &ips.password)
            .await?;

        // 4. Sub Credential 가져오기
        let sub = self
            .get_user_credential(&ips.volume_name, &ips.sub_user_name, &ips.password)
            .await?;

        // S3 Client 생성
        for url in &ips.s3_url {
            let main = main.as_ref().ok_or_else(null_reference)?;
            self.main_clients.push(S3Client::new(
                url,
                &main.access_key,
                &main.secret_key,
                false,
                2,
            ));
            let sub = sub.as_ref().ok_or_else(null_reference)?;
            self.sub_clients.push(S3Client::new(
                url,
                &sub.access_key,
                &sub.secret_key,
                false,
                2,
            ));
        }

        // 5. AccessIps Test
        for test in &test_list {
            self.test(&test.address, !ips.all_failed && test.pass)
                .await?;
        }
        for test in &test_list {
            self.test_bucket(&test.address, !ips.all_failed && test.pass)
                .await?;
        }

        info!("AccessIpsTest End. Clearing up...");

        // 6. 유저 삭제
        self.portal.delete_user(&ips.main_user_name).await?;
        self.portal.delete_user(&ips.sub_user_name).await?;

        // 7. 볼륨 삭제
        self.portal.stop_volume(&ips.volume_name).await?;
        self.portal.delete_volume(&ips.volume_name).await?;

        info!("AccessIpsTest Clear End");
        Ok(())
    }

    // ---- User Test ----

    /// Main/Sub/전체 유저 조합으로 IP 접근 제어 테스트를 실행하고 결과가 예측과 일치하는지 확인한다(원본 `Test`).
    async fn test(&self, ip: &str, prediction: bool) -> Result<(), ScenarioError> {
        if self.test_main_user_only(ip, prediction).await?
            && self.test_sub_user_only(ip, prediction).await?
            && self.test_all_user(ip, prediction).await?
        {
            info!(
                "User Access Ips Test Success : {ip}, {}",
                dotnet_bool(prediction)
            );
        } else {
            error!(
                "User Access Ips Test Fail : {ip}, {}",
                dotnet_bool(prediction)
            );
        }
        Ok(())
    }

    /// 한 사용자의 접근 IP 설정(`bucket`이 없으면 전체 버킷).
    async fn put_access_ip(
        &self,
        user: &str,
        ip: &str,
        bucket: Option<&str>,
    ) -> Result<(), ScenarioError> {
        Ok(self
            .portal
            .put_access_ip(&self.access_ips.volume_name, user, ip, bucket)
            .await?)
    }

    /// 한 사용자의 접근 IP 해제.
    async fn delete_access_ip(&self, user: &str) -> Result<(), ScenarioError> {
        Ok(self
            .portal
            .delete_access_ip(&self.access_ips.volume_name, user, None)
            .await?)
    }

    /// MainUser에만 접근 IP를 설정했을 때 ListBuckets 결과가 예측과 일치하는지 확인한다.
    async fn test_main_user_only(&self, ip: &str, prediction: bool) -> Result<bool, ScenarioError> {
        let test_name = "TestMainUserOnly";
        let main = self.access_ips.main_user_name.as_str();

        // 버킷 목록 조회 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        // MainUser Access Ip 설정
        self.put_access_ip(main, ip, None).await?;
        // MainUser Access Ip 설정 확인 테스트
        if !self
            .access_list_buckets_test(test_name, prediction, true)
            .await
        {
            return Ok(false);
        }
        // MainUser Access Ip 해제
        self.delete_access_ip(main).await?;
        // Access Ip 설정 해제 확인 테스트
        Ok(self.access_list_buckets_test(test_name, true, true).await)
    }

    /// SubUser에만 접근 IP를 설정했을 때 ListBuckets 결과가 예측과 일치하는지 확인한다.
    async fn test_sub_user_only(&self, ip: &str, prediction: bool) -> Result<bool, ScenarioError> {
        let test_name = "TestSubUserOnly";
        let sub = self.access_ips.sub_user_name.as_str();

        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        // SubUser Access Ip 설정
        self.put_access_ip(sub, ip, None).await?;
        // SubUser Access Ip 설정 확인 테스트
        if !self
            .access_list_buckets_test(test_name, true, prediction)
            .await
        {
            return Ok(false);
        }
        // SubUser Access Ip 해제
        self.delete_access_ip(sub).await?;
        Ok(self.access_list_buckets_test(test_name, true, true).await)
    }

    /// Main/Sub 유저 모두에 접근 IP를 설정했을 때 ListBuckets 결과가 예측과 일치하는지 확인한다.
    async fn test_all_user(&self, ip: &str, prediction: bool) -> Result<bool, ScenarioError> {
        let test_name = "TestAllUser";
        let main = self.access_ips.main_user_name.as_str();
        let sub = self.access_ips.sub_user_name.as_str();

        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        // MainUser·SubUser Access Ip 설정
        self.put_access_ip(main, ip, None).await?;
        self.put_access_ip(sub, ip, None).await?;
        // Access Ip 설정 확인 테스트
        if !self
            .access_list_buckets_test(test_name, prediction, prediction)
            .await
        {
            return Ok(false);
        }
        // MainUser·SubUser Access Ip 해제
        self.delete_access_ip(main).await?;
        self.delete_access_ip(sub).await?;
        // Access Ip 설정 해제 확인 테스트
        Ok(self.access_list_buckets_test(test_name, true, true).await)
    }

    // ---- Bucket Test ----

    /// 버킷 단위 IP 접근 제어를 위한 테스트용 버킷 4개를 생성하여 Main/Sub/전체 버킷 접근 제어 테스트를 실행하고
    /// 정리한다(원본 `TestBucket`).
    async fn test_bucket(&self, ip: &str, prediction: bool) -> Result<(), ScenarioError> {
        // 원본 그대로 네 버킷 모두 mainClients[0]으로 만든다.
        let first = self.main_clients.first().ok_or_else(index_out_of_range)?;
        let prefix = &self.access_ips.bucket_prefix;
        let buckets = AccessIpsBucketList {
            main_a: get_new_bucket(first, prefix, 10).await,
            main_b: get_new_bucket(first, prefix, 10).await,
            sub_a: get_new_bucket(first, prefix, 10).await,
            sub_b: get_new_bucket(first, prefix, 10).await,
        };

        if self.test_main_bucket_only(ip, &buckets, prediction).await?
            && self.test_sub_bucket_only(ip, &buckets, prediction).await?
            && self.test_all_bucket(ip, &buckets, prediction).await?
        {
            info!(
                "Bucket Access Ips Test Success : {ip}, {}",
                dotnet_bool(prediction)
            );
        } else {
            error!(
                "Bucket Access Ips Test Fail : {ip}, {}",
                dotnet_bool(prediction)
            );
        }

        first.delete_bucket(&buckets.main_a).await?;
        first.delete_bucket(&buckets.main_b).await?;
        // Sub 버킷은 subClients[0]으로 지운다.
        let sub_first = self.sub_clients.first().ok_or_else(index_out_of_range)?;
        sub_first.delete_bucket(&buckets.sub_a).await?;
        sub_first.delete_bucket(&buckets.sub_b).await?;
        Ok(())
    }

    /// MainUser 버킷에만 접근 IP를 설정했을 때 ListBuckets/ListObjects 결과가 예측과 일치하는지 확인한다.
    async fn test_main_bucket_only(
        &self,
        ip: &str,
        buckets: &AccessIpsBucketList,
        prediction: bool,
    ) -> Result<bool, ScenarioError> {
        let test_name = "TestMainBucketOnly";
        let main = self.access_ips.main_user_name.as_str();

        // 정상적인 버킷 생성 확인
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, true, true)
            .await
        {
            return Ok(false);
        }
        // MainUser Access Ip 설정
        self.put_access_ip(main, ip, Some(&buckets.main_a)).await?;
        // MainUser Access Ip 설정 확인 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, prediction, true)
            .await
        {
            return Ok(false);
        }
        // MainUser Access Ip 해제
        self.delete_access_ip(main).await?;
        // Access Ip 설정 해제 확인 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        Ok(self
            .access_list_objects_test(test_name, buckets, true, true)
            .await)
    }

    /// SubUser 버킷에만 접근 IP를 설정했을 때 ListBuckets/ListObjects 결과가 예측과 일치하는지 확인한다.
    async fn test_sub_bucket_only(
        &self,
        ip: &str,
        buckets: &AccessIpsBucketList,
        prediction: bool,
    ) -> Result<bool, ScenarioError> {
        let test_name = "TestSubBucketOnly";
        let sub = self.access_ips.sub_user_name.as_str();

        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, true, true)
            .await
        {
            return Ok(false);
        }
        // SubUser Access Ip 설정
        self.put_access_ip(sub, ip, Some(&buckets.sub_a)).await?;
        // SubUser Access Ip 설정 확인 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, true, prediction)
            .await
        {
            return Ok(false);
        }
        // SubUser Access Ip 해제
        self.delete_access_ip(sub).await?;
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        Ok(self
            .access_list_objects_test(test_name, buckets, true, true)
            .await)
    }

    /// Main/Sub 버킷 모두에 접근 IP를 설정했을 때 ListBuckets/ListObjects 결과가 예측과 일치하는지 확인한다.
    async fn test_all_bucket(
        &self,
        ip: &str,
        buckets: &AccessIpsBucketList,
        prediction: bool,
    ) -> Result<bool, ScenarioError> {
        let test_name = "TestAllBucket";
        let main = self.access_ips.main_user_name.as_str();
        let sub = self.access_ips.sub_user_name.as_str();

        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, true, true)
            .await
        {
            return Ok(false);
        }
        // MainUser·SubUser Access Ip 설정
        self.put_access_ip(main, ip, Some(&buckets.main_a)).await?;
        self.put_access_ip(sub, ip, Some(&buckets.sub_a)).await?;
        // Access Ip 설정 확인 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        if !self
            .access_list_objects_test(test_name, buckets, prediction, prediction)
            .await
        {
            return Ok(false);
        }
        // MainUser·SubUser Access Ip 해제
        self.delete_access_ip(main).await?;
        self.delete_access_ip(sub).await?;
        // Access Ip 설정 해제 확인 테스트
        if !self.access_list_buckets_test(test_name, true, true).await {
            return Ok(false);
        }
        Ok(self
            .access_list_objects_test(test_name, buckets, true, true)
            .await)
    }

    // ---- Util ----

    /// 유저 자격증명을 조회하고 없으면 유저를 생성하거나 볼륨을 할당한 뒤 다시 조회한다.
    /// 끝내 못 가져오면 `None`이고, 호출한 쪽의 `user.AccessKey`에서 `NullReferenceException`이 난다.
    async fn get_user_credential(
        &self,
        volume_name: &str,
        user_name: &str,
        password: &str,
    ) -> Result<Option<UserData>, ScenarioError> {
        let mut user = self
            .portal
            .get_user_credential(volume_name, user_name)
            .await?;

        // 가져오는데 실패했다면 생성
        if user.is_none() {
            // User가 없을 경우 생성
            if !self.portal.is_user(user_name).await? {
                self.portal
                    .create_user(volume_name, user_name, DEFAULT_VOLUME_SIZE, password)
                    .await?;
                info!("User {user_name} create");
            }
            // User가 있을 경우 볼륨 할당
            else {
                self.portal
                    .assign_volume(volume_name, user_name, DEFAULT_VOLUME_SIZE)
                    .await?;
                info!("User {user_name} assign");
            }
            user = self
                .portal
                .get_user_credential(volume_name, user_name)
                .await?;
        }
        Ok(user)
    }

    /// Main/Sub 클라이언트들의 ListBuckets 접근 가능 여부가 각각 기대값과 일치하는지 확인한다.
    async fn access_list_buckets_test(
        &self,
        test_name: &str,
        main_flag: bool,
        sub_flag: bool,
    ) -> bool {
        let mut result = true;
        for client in &self.main_clients {
            if access_check(client).await != main_flag {
                error!(
                    "{test_name} : Main User ListBucket does not match predictions({})",
                    dotnet_bool(main_flag)
                );
                result = false;
            }
        }
        for client in &self.sub_clients {
            if access_check(client).await != sub_flag {
                error!(
                    "{test_name} : Sub User ListBucket does not match predictions({})",
                    dotnet_bool(sub_flag)
                );
                result = false;
            }
        }
        result
    }

    /// Main/Sub 클라이언트들의 자신 버킷 ListObjects 접근 가능 여부와 타인 버킷 접근 차단 여부를 확인한다.
    async fn access_list_objects_test(
        &self,
        test_name: &str,
        buckets: &AccessIpsBucketList,
        main_flag: bool,
        sub_flag: bool,
    ) -> bool {
        let mut result = true;
        for client in &self.main_clients {
            if access_bucket_check(client, &buckets.main_a).await != main_flag {
                error!(
                    "{test_name} : Main User ListObjects({}) does not match predictions({})",
                    buckets.main_a,
                    dotnet_bool(main_flag)
                );
                result = false;
            }
            if access_bucket_check(client, &buckets.main_b).await {
                error!(
                    "{test_name} : Main User ListObjects({}) access failed!",
                    buckets.main_b
                );
                result = false;
            }
        }
        for client in &self.sub_clients {
            if access_bucket_check(client, &buckets.sub_a).await != sub_flag {
                error!(
                    "{test_name} : Sub User ListObjects({}) does not match predictions({})",
                    buckets.sub_a,
                    dotnet_bool(sub_flag)
                );
                result = false;
            }
            if access_bucket_check(client, &buckets.sub_b).await {
                error!(
                    "{test_name} : Sub User ListObjects({}) access failed!",
                    buckets.sub_b
                );
                result = false;
            }
        }
        result
    }
}

/// `List<T>[0]`이 비었을 때의 `ArgumentOutOfRangeException`.
fn index_out_of_range() -> ScenarioError {
    ScenarioError::new(
        "System.ArgumentOutOfRangeException",
        "Index was out of range. Must be non-negative and less than the size of the collection. (Parameter 'index')",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_test_data_like_dotnet() {
        let data = AccessIpsTestData::parse(" 10.0.0.1 , TRUE ").unwrap();
        assert_eq!(data.address, " 10.0.0.1 ");
        assert!(data.pass);
        let error = AccessIpsTestData::parse("10.0.0.1").unwrap_err();
        assert_eq!(
            (error.dotnet_type.as_str(), error.message.as_str()),
            (
                "TestCore.Portal.InvalidAccessIpsTestDataException",
                "10.0.0.1"
            )
        );
        let error = AccessIpsTestData::parse("a,maybe").unwrap_err();
        assert_eq!(error.dotnet_type, "System.FormatException");
        assert_eq!(
            error.message,
            "String 'maybe' was not recognized as a valid Boolean."
        );
    }

    #[test]
    fn reads_lines_like_read_all_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.txt");
        std::fs::write(&path, "a,true\r\n# c\n\n  \rb,false\n").unwrap();
        let config = AccessIpsConfig::new(
            "p",
            false,
            path.to_str().unwrap(),
            "j",
            vec![],
            "v",
            "m",
            "s",
            "pw",
        );
        let list = get_test_list(&config).unwrap();
        assert_eq!(
            list,
            [
                AccessIpsTestData {
                    address: "a".into(),
                    pass: true
                },
                AccessIpsTestData {
                    address: "b".into(),
                    pass: false
                }
            ]
        );
    }
}
