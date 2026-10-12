//! 분산 실행(TESTCore `Distributed/*`): Controller가 여러 Worker에 같은 부하 테스트를 나눠 실행하고 결과를 모은다.
//!
//! 통신은 Worker의 `/driver` HTTP API(JSON, camelCase)다. .NET Controller·Worker와 섞어 쓸 수 있어야 하므로 계약
//! 형식([`contracts`])과 상태 전이(`testrust_scenarios::run_control`)를 원본과 같게 둔다.

pub mod console;
pub mod contracts;
pub mod controller;
pub mod diagnostics;
pub mod result_writer;
pub mod runner;
pub mod settings;
pub mod worker;

use testrust_config::Config;
use testrust_scenarios::ScenarioError;
use testrust_scenarios::shutdown::{self, Handler};
use tokio_util::sync::CancellationToken;

use crate::contracts::RunOptions;
use crate::settings::{DistributedSettings, argument};

/// 원본 `DistributedApplication.RunAsync(CommandOptions)`가 읽는 `CommandOptions` 값.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistributedArgs {
    pub worker: bool,
    pub controller: bool,
    /// `Menu != MenuList.None`.
    pub menu_selected: bool,
    pub debug: bool,
    pub config_path: String,
    /// `--save`.
    pub save: Option<String>,
    /// 메뉴에서 정한 테스트 종류와 테스트별 CLI 인자.
    pub run_options: RunOptions,
}

/// 원본 `DistributedApplication.RunAsync`: 분산 실행 모드를 고르고, Controller의 Ctrl+C를 원격 작업 중단으로 잇는다.
///
/// `load_config`는 원본 `ConfigBootstrapper.Load(options)`(설정 파일을 읽고 명령행 값으로 덮어쓴다; `--debug`는 이미
/// 꺼진 상태여야 한다)다. 실패하면 `None`. 돌려주는 값은 종료 코드이고, 예외는 `Err`다.
pub async fn run(
    args: DistributedArgs,
    load_config: impl FnOnce() -> Option<Config>,
) -> Result<i32, ScenarioError> {
    if args.worker && args.controller {
        return Err(argument(
            "--worker와 --controller는 함께 지정할 수 없습니다.",
        ));
    }
    let settings = DistributedSettings::load(&args.config_path, args.worker)?;
    if args.worker {
        if args.menu_selected {
            return Err(argument(
                "Worker 서버 모드에 테스트 명령을 함께 지정할 수 없습니다.",
            ));
        }
        if args.debug {
            diagnostics::print_settings(&settings, &args.config_path);
        }
        // 원본 호스트는 Ctrl+C에 정상 종료한다(종료 코드 0).
        let shutdown_token = CancellationToken::new();
        let _guard = shutdown::activate(&shutdown_token, Handler::Scoped);
        worker::run_worker(settings, args.debug, shutdown_token).await?;
        return Ok(0);
    }
    let config = load_config().ok_or_else(|| argument("테스트 설정 파일을 읽지 못했습니다."))?;
    let cancellation = CancellationToken::new();
    let _guard = shutdown::activate(&cancellation, Handler::Scoped);
    controller::run_async(
        &settings,
        &config,
        &args.run_options,
        args.save.as_deref(),
        &cancellation,
    )
    .await
}
