//! 원본 `Distributed/TestRunner.cs`: `DistributedTestRunner`(데이터셋 재사용, UpDownTest 연결)와 `BasicTestRunner`.
//!
//! 원본 `IDistributedTestRunner`는 [`DistributedTestRunner`] 트레이트로 둔다. Worker는 이 트레이트만 알므로 검증에서는
//! S3 없이 준비 게이트만 통과하는 가짜 실행기로 바꿀 수 있다.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use testrust_clients::up_down::UpDownClient;
use testrust_config::{MainConfig, UpDownConfig};
use testrust_model::UpDownResult;
use testrust_scenarios::ScenarioError;
use testrust_scenarios::run_control::RunControl;
use testrust_scenarios::up_down::UpDownTest;
use testrust_scenarios::util::dummy_file_name;
use tokio_util::sync::CancellationToken;

use crate::contracts::TestRequest;
use crate::settings::argument;

/// 박스로 감싼 `Send` 퓨처.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 원본 `IDistributedTestRunner`: 실제 S3 부하 실행과 Worker 수명 관리를 분리한다.
pub trait DistributedTestRunner: Send + Sync {
    /// 원본 `Execute()`. 취소로 끝나면 `System.OperationCanceledException`.
    fn execute(&self) -> BoxFuture<'_, Result<(), ScenarioError>>;

    /// 원본 `Snapshot()`: 실행 중에도 부를 수 있는 누적 통계.
    fn snapshot(&self) -> UpDownResult;

    /// 원본 `Dispose()`.
    fn dispose(&self) -> Result<(), ScenarioError>;
}

/// 원본 `Func<TestRequest, RunControl, IDistributedTestRunner>`. 실패하면 Worker 작업이 `Failed`로 끝난다.
pub type RunnerFactory = dyn Fn(&TestRequest, Arc<RunControl>) -> Result<Arc<dyn DistributedTestRunner>, ScenarioError>
    + Send
    + Sync;

/// `Exception.GetType().Name`: 네임스페이스를 뺀 형식 이름.
pub fn short_type(error: &ScenarioError) -> &str {
    error
        .dotnet_type
        .rsplit('.')
        .next()
        .unwrap_or(&error.dotnet_type)
}

/// `System.OperationCanceledException`인지.
pub fn is_canceled(error: &ScenarioError) -> bool {
    error.dotnet_type == "System.OperationCanceledException"
}

/// 원본 `DistributedTestRunner`: 원격 요청을 `UpDownTest`에 연결하고, 데이터셋 재사용과 실행 종료를 관리한다.
/// `BasicTestRunner.Execute` 인자.
#[derive(Clone)]
struct Plan {
    test_type: String,
    check: bool,
    start: i32,
    random: bool,
    bulk: bool,
    max_count: i32,
}

pub struct UpDownRunner {
    plan: Plan,
    test_type: String,
    control: Arc<RunControl>,
    main: MainConfig,
    up_down: UpDownConfig,
    published: Arc<Mutex<Vec<Arc<UpDownClient>>>>,
    /// 실행 중에는 `execute`가 가져가고, 끝나면 돌려놓는다(`dispose`가 버린다).
    test: Mutex<Option<UpDownTest>>,
}

/// System.Text.Json 기본 인코더가 문자열을 쓰는 방식: `"<>&'+`` ·제어 문자·비 ASCII는 `\uXXXX`.
fn json_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if !c.is_ascii()
                || c.is_ascii_control()
                || matches!(c, '"' | '<' | '>' | '&' | '\'' | '+' | '`') =>
            {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_optional(text: Option<&str>, out: &mut String) {
    match text {
        Some(text) => json_string(text, out),
        None => out.push_str("null"),
    }
}

/// 데이터셋 식별 JSON: `new { WorkerId, URL, BucketName, ThreadPrefix, ObjectPrefix, FileSize, BucketType }`를
/// `JsonSerializer.Serialize`한 문자열(압축, 기본 인코더). RunId는 제외하여 다음 실행에서도 재사용한다.
pub fn dataset_identity(request: &TestRequest) -> String {
    let w = request.workload.clone().unwrap_or_default();
    let mut out = String::from("{\"WorkerId\":");
    json_optional(request.worker_id.as_deref(), &mut out);
    out.push_str(",\"URL\":");
    json_optional(request.user.as_ref().map(|u| u.url.as_str()), &mut out);
    out.push_str(",\"BucketName\":");
    json_optional(w.bucket_name.as_deref(), &mut out);
    out.push_str(",\"ThreadPrefix\":");
    json_optional(w.thread_prefix.as_deref(), &mut out);
    out.push_str(",\"ObjectPrefix\":");
    json_optional(w.object_prefix.as_deref(), &mut out);
    out.push_str(&format!(
        ",\"FileSize\":{},\"BucketType\":{}}}",
        w.file_size, w.bucket_type
    ));
    out
}

/// 데이터셋 디렉터리 이름: 식별 JSON의 SHA-256(소문자 16진수).
pub fn dataset_name(request: &TestRequest) -> String {
    hex::encode(Sha256::digest(dataset_identity(request).as_bytes()))
}

impl UpDownRunner {
    /// 원본 `DistributedTestRunner(request, control, workPath)`.
    pub fn new(
        request: &TestRequest,
        control: Arc<RunControl>,
        work_path: &Path,
    ) -> Result<Self, ScenarioError> {
        let w = request
            .workload
            .clone()
            .ok_or_else(|| argument("Workload 설정 필요"))?;
        let worker_id = request.worker_id.clone().unwrap_or_default();
        let test_type = request.test_type.clone().unwrap_or_default();
        // Prepare와 별도 GET의 ETag 검사가 동일한 원본 파일을 사용해야 한다.
        let path = work_path.join("datasets").join(dataset_name(request));
        std::fs::create_dir_all(&path)?;
        let path_text = path.to_string_lossy().into_owned();
        if test_type == "Get" && w.e_tag_check {
            for i in 0..w.thread_count {
                if !Path::new(&dummy_file_name(i, Some(&path_text))).exists() {
                    return Err(ScenarioError::new(
                        "System.InvalidOperationException",
                        "ETag 검사용 원본 파일이 없습니다. 동일 Worker·설정에서 Prepare를 먼저 실행하세요.",
                    ));
                }
            }
        }
        let main = w.to_main(&worker_id, &path_text)?;
        let up_down = w.to_up_down();
        let user = request.user.clone().unwrap_or_default();
        let test = UpDownTest::new(&main, &up_down, &user, &CancellationToken::new())
            .with_control(control.clone());
        Ok(Self {
            plan: Plan {
                test_type: test_type.clone(),
                check: w.check,
                start: w.start,
                random: w.random,
                bulk: w.bulk,
                max_count: w.max_count,
            },
            test_type,
            control,
            main,
            up_down,
            published: test.published_clients(),
            test: Mutex::new(Some(test)),
        })
    }
}

/// `UpDownTest`의 시나리오 퓨처는 `Send`가 아니므로 블로킹 스레드에서 `block_on`으로 돌린다(내부 태스크는 런타임이 그대로
/// 병렬로 실행한다). 끝나면 테스트를 돌려준다.
fn run_to_end(
    mut test: UpDownTest,
    plan: Plan,
    control: Arc<RunControl>,
) -> (UpDownTest, Result<(), ScenarioError>) {
    tokio::runtime::Handle::current().block_on(async move {
        let result = match basic_execute(
            &mut test,
            &plan.test_type,
            plan.check,
            plan.start,
            plan.random,
            plan.bulk,
            plan.max_count,
        )
        .await
        {
            Err(e) if is_canceled(&e) && control.is_stopped() => Err(e),
            Err(e) => {
                // 일부 스레드만 시작된 경우에도 게이트 대기를 해제한 뒤 Join해야 한다.
                control.stop(Some(&format!("테스트 실행 오류: {}", short_type(&e))), true);
                Err(e)
            }
            Ok(()) => Ok(()),
        };
        // 이미 발행한 요청까지 정리한 후 반환해야 Worker의 최종 스냅샷에서 완료 건수가 누락되지 않는다.
        test.drain_distributed().await;
        (test, result)
    })
}

impl DistributedTestRunner for UpDownRunner {
    fn execute(&self) -> BoxFuture<'_, Result<(), ScenarioError>> {
        Box::pin(async move {
            let taken = self.test.lock().unwrap_or_else(|e| e.into_inner()).take();
            let Some(test) = taken else {
                return Err(ScenarioError::new(
                    "System.ObjectDisposedException",
                    "실행기가 이미 사용되었습니다.",
                ));
            };
            let plan = self.plan.clone();
            let control = self.control.clone();
            let (test, result) =
                tokio::task::spawn_blocking(move || run_to_end(test, plan, control))
                    .await
                    .map_err(|e| {
                        ScenarioError::new("System.InvalidOperationException", e.to_string())
                    })?;
            *self.test.lock().unwrap_or_else(|e| e.into_inner()) = Some(test);
            result
        })
    }

    fn snapshot(&self) -> UpDownResult {
        UpDownTest::distributed_result(
            &self.published,
            &self.test_type,
            &self.up_down,
            &self.main,
            self.control.elapsed_seconds(),
        )
    }

    fn dispose(&self) -> Result<(), ScenarioError> {
        // 원본 `DisposeDistributed()`: 클라이언트와 S3 클라이언트를 해제한다(여기서는 버리면 된다).
        self.test.lock().unwrap_or_else(|e| e.into_inner()).take();
        Ok(())
    }
}

/// 원본 `BasicTestRunner.Execute`: 테스트 종류별 진입점.
pub async fn basic_execute(
    test: &mut UpDownTest,
    test_type: &str,
    check: bool,
    start: i32,
    random: bool,
    bulk: bool,
    max_count: i32,
) -> Result<(), ScenarioError> {
    match test_type {
        "Prepare" => test.prepare(check, start, random).await,
        "Put" => test.write(start, random).await,
        "Get" => test.read().await,
        "Delete" => test.delete(bulk, max_count).await,
        "Mix" => test.mix().await,
        _ => Err(argument("지원하지 않는 테스트")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_matches_system_text_json() {
        let request = TestRequest {
            worker_id: Some("driver1".into()),
            user: Some(testrust_config::UserData::new(
                "http://h:9/?a=b+c",
                "",
                "",
                "",
            )),
            workload: Some(crate::contracts::WorkloadSettings {
                bucket_name: Some("b<ucket>".into()),
                file_size: 1024,
                bucket_type: 4,
                ..Default::default()
            }),
            ..TestRequest::default()
        };
        assert_eq!(
            dataset_identity(&request),
            "{\"WorkerId\":\"driver1\",\"URL\":\"http://h:9/?a=b\\u002Bc\",\"BucketName\":\"b\\u003Cucket\\u003E\",\"ThreadPrefix\":\"TH\",\"ObjectPrefix\":\"FILE\",\"FileSize\":1024,\"BucketType\":4}"
        );
    }
}
