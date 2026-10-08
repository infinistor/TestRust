//! awscli-rust 진입점 (TESTCore `Program.cs`).

use awscli_rust_cli::{app, logging};

/// Linux 정적(musl) 빌드의 할당기. musl 기본 할당기(mallocng)는 전역 락이라 스레드 32개가 1MiB 버퍼를
/// 계속 잡는 Put에서 락 경합으로 처리량이 떨어진다(docs/perf/2026-10-08-s3gw-155.md). Windows는 시스템 할당기 그대로.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// mimalloc이 비운 메모리를 바로 운영체제에 돌려주게 한다(기본값은 오래 쥐고 있어 최대 RSS가 몇 배 커진다).
/// 워크스페이스에서 unsafe를 허용하는 유일한 곳이다.
#[cfg(target_env = "musl")]
#[allow(unsafe_code)]
fn tune_allocator() {
    // mimalloc v3 `mi_option_e`의 값(include/mimalloc.h). libmimalloc-sys에는 이 두 상수가 없다.
    const ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
    const PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;
    // SAFETY: 옵션 값만 바꾸는 함수이고, 작업 스레드를 만들기 전에 부른다.
    unsafe {
        libmimalloc_sys::mi_option_set(ARENA_EAGER_COMMIT, 0);
        libmimalloc_sys::mi_option_set(PURGE_DELAY, 0);
    }
}

fn main() {
    #[cfg(target_env = "musl")]
    tune_allocator();
    logging::init();
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    std::process::exit(app::run(&args));
}
