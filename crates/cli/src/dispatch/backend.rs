//! 원본 `CommandDispatcher` 중 S3backend 서비스 일시정지·재개(ZeroMQ).
//!
//! 원본과 같게 맞춘 동작
//!
//! - 검증 순서는 `--service-type`(`--pause=`·`--resume=`의 값), `--address`, `--port`(1 미만)이다.
//! - `ZeroMqClient.Pause/Resume`이 돌려주는 값(성공 0, 응답이 `OK!`가 아니면 -1)이 그대로 종료 코드가 된다.
//!   응답이 `OK!`가 아니어도 `S3backend... complete time` 로그는 남는다.
//! - 서버에 연결할 수 없거나 응답이 없으면 NetMQ처럼 끝나지 않는다(`zeromq` 크레이트도 연결을 계속 다시 시도한다).
//!
//! .NET과 다른 점
//!
//! - 주소를 해석하지 못할 때 .NET은 `System.Net.Sockets.SocketException`(OS 언어로 된 메시지)을, 여기서는
//!   `NetMQ.NetMQException` 형식의 `zeromq` 크레이트 오류 메시지를 낸다. 하네스는 ZeroMQ 상대를 만들지 못하므로
//!   성공·`OK!` 아닌 응답·연결 실패 경로는 실행 비교를 하지 않는다(`tests/parity/zeromq.rs`가 클라이언트 단위로 확인한다).

use super::input::blank;
use std::time::Instant;

use testrust_clients::zeromq;
use tracing::info;

use super::{CommandContext, CommandError, CommandResult, not_ported};
use crate::menu::MenuList;
use crate::usage;

/// 옮긴 메뉴.
pub(super) const PORTED: &[MenuList] = &[MenuList::S3backendPause, MenuList::S3backendResume];

pub(super) async fn run(ctx: &mut CommandContext, menu: MenuList) -> CommandResult {
    let (flag, name) = match menu {
        MenuList::S3backendPause => (usage::BACKEND_PAUSE, "S3backendPause"),
        MenuList::S3backendResume => (usage::BACKEND_RESUME, "S3backendResume"),
        _ => return not_ported(menu),
    };
    let options = &ctx.options;
    if options.help {
        println!(
            "{}",
            [
                usage::main_flag(flag, ""),
                usage::sub_flag(usage::SERVICE_TYPE, "replication, lifecycle", ""),
                usage::sub_flag(usage::ADDRESS, "string", ""),
                usage::sub_flag(usage::PORT, "int", ""),
            ]
            .concat()
        );
        return Ok(0);
    }
    if blank(&options.service_type) {
        println!("{}", usage::ERROR_SERVICE_TYPE);
        return Ok(0);
    }
    if blank(&options.address) {
        println!("{}", usage::ERROR_ADDRESS);
        return Ok(0);
    }
    if options.port < 1 {
        println!("{}", usage::ERROR_PORT);
        return Ok(0);
    }
    let service_type = options.service_type.clone().unwrap_or_default();
    let address = options.address.clone().unwrap_or_default();
    let port = options.port;

    info!("{name} Start");
    let sw = Instant::now();
    let result = if menu == MenuList::S3backendPause {
        zeromq::pause(&service_type, &address, port).await
    } else {
        zeromq::resume(&service_type, &address, port).await
    }
    .map_err(|e| CommandError::new(e.dotnet_type(), e.to_string()))?;
    info!("{name} complete time = {}ms", sw.elapsed().as_millis());
    Ok(result)
}
