//! `Test/IoTest.cs`: 폴더의 파일을 모두 업로드한 뒤 다시 내려받아 MD5로 비교하고 정리한다.
//!
//! 진행 상황은 로그가 아니라 콘솔(`Console.WriteLine`)에 찍는다.
//!
//! 1. `IO Test Start` → 버킷 생성(`CreateBucket`) → 100ms 대기 → 업로드 폴더의 파일 목록(없으면 `File Not Found`로 끝).
//! 2. 파일마다 `Upload`(키는 `전체 경로.Replace(uploadPath, "")`). 실패하면 `{경로} upload failed!` 로그를 남기고 끝낸다.
//!    모두 올리면 `Upload Complete`.
//! 3. `ListObjectsV2`의 첫 쪽 키를 `downloadPath + key`(구분자 없이 이어 붙인다)에 저장하고 `Download Complete`.
//!    이 단계의 예외는 로그로 남기고 끝낸다.
//! 4. 업로드 폴더와 다운로드 폴더를 `CompareDirMD5`로 비교한다. 같으면 `Compare Success`, 다운로드 폴더를 지우고
//!    `Delete Complete`; 다르면 `Compare Fail`(폴더는 남긴다).
//! 5. 키를 모두 지우고 버킷을 지운 뒤 `Delete Complete`, `IO Test End`.
//!
//! 단계 1·2의 `GetFileList`, 4·5의 비교·삭제 오류는 잡지 않아 호출한 쪽으로 올라간다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 키는 `string.Replace`로 `uploadPath`를 모두 지워 만든다. 업로드 폴더를 상대 경로로 주면 전체 경로에서 그 문자열이
//!   나오는 곳이 모두 지워지고, 남은 앞부분(드라이브·상위 폴더)이 키에 들어간다.
//! - 다운로드 경로에 구분자를 붙이지 않으므로 `downloadPath`는 `/`(또는 `\`)로 끝내야 폴더 안에 저장된다.
//! - `ListObjectsV2`는 첫 쪽(1,000개)만 읽는다. 목록이 비면 AWSSDK v4의 `null` 목록 때문에 `NullReferenceException`이다.
//! - 단계 4에서 `Compare Fail`이어도 이어서 객체와 버킷을 지운다.
//! - 인자를 검증하지 않는다. 업로드 폴더가 없으면(`null`, 빈 문자열, 없는 폴더) 예외가 호출한 쪽으로 올라가고, 다운로드
//!   폴더가 없으면(`null`) 다운로드 단계의 예외로 로그만 남기고 끝난다.

use std::path::Path;
use std::time::Duration;

use testrust_config::UserData;
use testrust_s3::S3Client;
use tracing::error;

use crate::ScenarioError;
use crate::files::{create_dir_error, file_list, full_path, io_error, save_file};
use crate::input::null_reference;
use crate::util::compare_dir_md5;

/// 원본 `S3Client.Upload`의 기본 파트 크기(`5 * Utility.MiB`)와 동시 요청 수.
const PART_SIZE: i64 = 5 * 1024 * 1024;
const THREAD_COUNT: usize = 10;

/// 원본 `IoTest`.
pub struct IoTest {
    user: UserData,
}

impl IoTest {
    /// 원본 `IoTest(Config config)`: 주 사용자(`config.MainUser`)를 쓴다.
    pub fn new(user: UserData) -> Self {
        Self { user }
    }

    /// 원본 `Start(bucketName, uploadPath, downloadPath)`.
    pub async fn start(
        &self,
        bucket_name: Option<&str>,
        upload_path: Option<&str>,
        download_path: Option<&str>,
    ) -> Result<(), ScenarioError> {
        println!("IO Test Start");

        // 클라이언트 생성
        let client = S3Client::from_user(&self.user, false, 3, false);

        // 버킷 생성
        let bucket = bucket_name.unwrap_or_default();
        // 버킷 이름이 비어 있어도 원본은 `GET /?acl`을 보내고(응답이 200이면 있는 것으로 본다, 없으면 `PutBucket`이
        // `ArgumentException`), 이어서 `Upload`가 `InvalidOperationException`으로 실패한다.
        client.create_bucket(bucket).await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 입력된 경로의 파일 업로드
        // `new DirectoryInfo(root)`: `null`은 `ArgumentNullException`, 빈 문자열은 `ArgumentException`.
        let files = match upload_path {
            Some("") => {
                return Err(ScenarioError::new(
                    "System.ArgumentException",
                    "The path is empty. (Parameter 'path')",
                ));
            }
            Some(path) => file_list(path)?,
            None => return Err(argument_null("path")),
        };
        if files.is_empty() {
            println!("File Not Found");
            return Ok(());
        }
        let upload_path = upload_path.unwrap_or_default();

        for file in &files {
            if let Err(e) = upload(&client, bucket, upload_path, file).await {
                error!("{file} upload failed!\n\n{e}");
                return Ok(());
            }
        }
        println!("Upload Complete");

        // 업로드된 파일 다운로드
        let keys = match self.download(&client, bucket, download_path).await {
            Ok(keys) => keys,
            Err(e) => {
                error!("{e}");
                return Ok(());
            }
        };

        // 업로드한 파일과 다운로드한 파일 비교
        let download_path = download_path.unwrap_or_default();
        if compare_dir_md5(upload_path, download_path)? {
            println!("Compare Success");

            // 다운로드 경로 삭제
            println!("Delete Download Path");
            let full = full_path(download_path);
            std::fs::remove_dir_all(&full).map_err(|e| io_error(&full, &e))?;
            println!("Delete Complete");
        } else {
            println!("Compare Fail");
        }

        // 업로드한 파일 삭제
        println!("Delete S3 Object List");
        for key in &keys {
            client.delete_object(bucket, key, None, None).await?;
        }

        // 버킷 삭제
        client.delete_bucket(bucket).await?;
        println!("Delete Complete");

        println!("IO Test End");
        Ok(())
    }

    /// 원본의 다운로드 `try` 블록: 첫 쪽 목록의 오브젝트를 `downloadPath + key`에 저장하고 키 목록을 돌려준다.
    async fn download(
        &self,
        client: &S3Client,
        bucket: &str,
        download_path: Option<&str>,
    ) -> Result<Vec<String>, ScenarioError> {
        let response = client
            .list_objects_v2(bucket, None, None, 1000, None, None)
            .await?;
        let contents = response.output.contents();
        // 빈 목록은 `null`이라 `ConvertAll`에서 `NullReferenceException`.
        if contents.is_empty() {
            return Err(null_reference());
        }
        let keys: Vec<String> = contents
            .iter()
            .map(|o| o.key().unwrap_or_default().to_string())
            .collect();

        // 다운로드 경로 생성
        create_directory(download_path)?;

        let prefix = download_path.unwrap_or_default();
        for object in &keys {
            let response = client
                .get_object(bucket, object, None, None)
                .await
                .map_err(|e| ScenarioError::s3(e, &["NoSuchKey", "InvalidObjectState"]))?;
            save_file(Some(&format!("{prefix}{object}")), response.output.body).await;
        }
        println!("Download Complete");
        Ok(keys)
    }
}

/// `client.Upload(bucketName, File.Replace(uploadPath, ""), File)`.
async fn upload(
    client: &S3Client,
    bucket: &str,
    upload_path: &str,
    file: &str,
) -> Result<(), ScenarioError> {
    if upload_path.is_empty() {
        return Err(ScenarioError::new(
            "System.ArgumentException",
            "String cannot be of zero length. (Parameter 'oldValue')",
        ));
    }
    let key = file.replace(upload_path, "");
    if bucket.is_empty() {
        return Err(ScenarioError::new(
            "System.InvalidOperationException",
            "Please specify BucketName to PUT an object into Amazon S3.",
        ));
    }
    client
        .upload(
            bucket,
            &key,
            Some(Path::new(file)),
            PART_SIZE,
            THREAD_COUNT,
            None,
            None,
            None,
        )
        .await?;
    Ok(())
}

/// `Directory.CreateDirectory(path)`.
fn create_directory(path: Option<&str>) -> Result<(), ScenarioError> {
    let Some(path) = path else {
        return Err(argument_null("path"));
    };
    if path.is_empty() {
        return Err(ScenarioError::new(
            "System.ArgumentException",
            "The value cannot be an empty string. (Parameter 'path')",
        ));
    }
    let full = full_path(path);
    if !full.is_dir() {
        std::fs::create_dir_all(&full).map_err(|e| create_dir_error(&full, &e))?;
    }
    Ok(())
}

fn argument_null(parameter: &str) -> ScenarioError {
    ScenarioError::new(
        "System.ArgumentNullException",
        format!("Value cannot be null. (Parameter '{parameter}')"),
    )
}
