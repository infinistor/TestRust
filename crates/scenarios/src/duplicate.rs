//! `Test/DuplicateTest.cs`: 폴더 구조를 흉내 낸 오브젝트 목록을 버저닝 버킷에 반복해서 만들고 지운다.
//!
//! 원본과 같게 맞춘 동작
//!
//! - 버킷이 없으면 만들고(`S3Client.CreateBucket`, 결과 로그) 버저닝을 `Enabled`로 켠다. 버저닝 설정 오류는 로그만 남기고 계속한다.
//! - `ObjectPath`를 `/`로 쪼개 `{접두어}_0`, `a/`, `a/{접두어}_1`, `a/b/`, `a/b/{접두어}_2` 순의 키 목록을 만든다.
//!   `LoopCount`번 반복하며 키마다 요청 하나를 만들어(폴더가 아니면 본문은 `RandomTextLong(FileSize)`) 폴더는 한 번,
//!   그 밖은 `ObjectCount`번 올린 뒤 로그(`[Loop:0001] Create N Objects`)를 남기고, 키를 거꾸로 모두 지운다(`Delete N Objects`).
//! - `PutObject`·`DeleteObject` 예외는 잡지 않아 호출자(최상위)로 전파된다.
//!
//! 원본 특이점(그대로 둔다)
//!
//! - 본문 길이는 `(int)FileSize`다(`long` → `int` 잘림). 음수면 `Enumerable.Range`가 `ArgumentOutOfRangeException`을 던진다.
//! - `Delete N Objects`의 N은 키 개수(폴더 포함)이고 실제 삭제 요청 수와 같다. 오브젝트를 `ObjectCount`번 올려도 버전만 늘 뿐이다.

use aws_sdk_s3::types::BucketVersioningStatus;
use testrust_config::util::random_text_long;
use testrust_config::{DuplicateConfig, MainConfig, UserData};
use testrust_s3::S3Client;
use testrust_s3::s3_client::{PutBody, PutObjectRequest};
use tracing::{error, info};

use crate::ScenarioError;

/// 원본 `DuplicateTest`.
pub struct DuplicateTest {
    default_config: MainConfig,
    config: DuplicateConfig,
    client: S3Client,
}

impl DuplicateTest {
    pub fn new(default_config: MainConfig, config: DuplicateConfig, user: &UserData) -> Self {
        Self {
            default_config,
            config,
            client: S3Client::from_user(user, false, 3, false),
        }
    }

    /// 원본 `Start`.
    pub async fn start(&self) -> Result<(), ScenarioError> {
        let bucket = &self.default_config.bucket_name;

        // Create Bucket
        self.client.create_bucket(bucket).await;
        self.set_bucket_versioning(bucket).await;

        // 생성 목록 생성
        let keys = object_list(&self.default_config.object_prefix, &self.config.object_path);
        // 삭제 목록 생성 (역순)
        let delete_keys: Vec<&String> = keys.iter().rev().collect();

        for i in 0..self.config.loop_count {
            // Object 생성
            for key in &keys {
                // 폴더는 빈 본문, 그 밖은 `RandomTextLong((int)FileSize)`
                let content = if is_folder(key) {
                    String::new()
                } else {
                    random_text_long_int(self.default_config.file_size as i32)?
                };

                // 폴더 생성시 1개만 생성
                let count = if is_folder(key) {
                    1
                } else {
                    self.config.object_count
                };

                // Object 생성
                for _ in 0..count {
                    self.client
                        .put_object_request(PutObjectRequest {
                            bucket_name: bucket.clone(),
                            key: Some(key.clone()),
                            body: Some(PutBody::Text(content.clone())),
                            // 원본 `PutObjectRequest`의 `UseChunkEncoding` 기본값
                            use_chunk_encoding: true,
                            ..PutObjectRequest::default()
                        })
                        .await
                        .map_err(|e| ScenarioError::s3(e, &[]))?;
                }
            }
            info!("[Loop:{:04}] Create {} Objects", i + 1, keys.len());

            // Object 삭제
            for key in &delete_keys {
                self.client
                    .delete_object(bucket, key, None, None)
                    .await
                    .map_err(|e| ScenarioError::s3(e, &[]))?;
            }
            info!("[Loop:{:04}] Delete {} Objects", i + 1, delete_keys.len());
        }
        Ok(())
    }

    /// 원본 `SetBucketVersioning`: 버킷의 버저닝을 켠다. 실패는 로그만 남긴다.
    async fn set_bucket_versioning(&self, bucket: &str) {
        if let Err(e) = self
            .client
            .put_bucket_versioning(bucket, Some(BucketVersioningStatus::Enabled))
            .await
        {
            error!("{}", ScenarioError::s3(e, &[]));
        }
    }
}

/// 원본 `GetObjectList`: 디렉터리 경로를 계층별로 나눠 루트 오브젝트, 폴더, 각 계층의 오브젝트 키를 만든다.
fn object_list(prefix: &str, dir_path: &str) -> Vec<String> {
    let mut keys = vec![object_name(prefix, "", 0)];

    // /로 시작할 경우 제거
    let mut dir_path = dir_path.strip_prefix('/').unwrap_or(dir_path).to_string();
    // /로 끝나지 않을 경우 추가
    if !dir_path.ends_with('/') {
        dir_path.push('/');
    }

    let mut start_index = 0;
    let mut count = 1;
    while let Some(found) = dir_path[start_index..].find('/') {
        let index = start_index + found;
        let key = dir_path[..=index].to_string();
        keys.push(key.clone());
        keys.push(object_name(prefix, &key, count));
        count += 1;
        start_index = index + 1;
    }
    keys
}

/// 원본 `GetObjectName`.
fn object_name(prefix: &str, key_path: &str, index: i32) -> String {
    format!("{key_path}{prefix}_{index}")
}

/// 원본 `IsFolder`.
fn is_folder(path: &str) -> bool {
    path.ends_with('/')
}

/// `Utility.RandomTextLong(int length)`: 음수 길이는 `Enumerable.Range`가 던지는 예외.
fn random_text_long_int(length: i32) -> Result<String, ScenarioError> {
    match usize::try_from(length) {
        Ok(length) => Ok(random_text_long(length)),
        Err(_) => Err(ScenarioError::new(
            "System.ArgumentOutOfRangeException",
            "Specified argument was out of the range of valid values. (Parameter 'count')",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_key_list() {
        assert_eq!(object_list("OBJ", ""), ["OBJ_0", "/", "/OBJ_1"]);
        assert_eq!(
            object_list("OBJ", "/a/b"),
            ["OBJ_0", "a/", "a/OBJ_1", "a/b/", "a/b/OBJ_2"]
        );
        assert_eq!(
            object_list("OBJ", "a/b/"),
            ["OBJ_0", "a/", "a/OBJ_1", "a/b/", "a/b/OBJ_2"]
        );
    }

    #[test]
    fn negative_length_fails() {
        assert!(random_text_long_int(-1).is_err());
        assert_eq!(random_text_long_int(4).unwrap().len(), 4);
    }
}
