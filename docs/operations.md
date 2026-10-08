# 운영 절차

awscli-rust의 빌드·배포, 릴리스, TESTCore에서의 전환, 분산 실행 교체 순서, 성능 비교, 롤백, 기준 출력 갱신 절차를 정리한다.

## 빌드와 배포

### Windows

PowerShell에서 실행한다(Git Bash의 `/usr/bin/link`가 MSVC `link.exe`를 가려 링크가 실패한다). Rust stable과 Visual Studio Build Tools의 C++ 워크로드가 필요하다.

```powershell
cargo build --release
```

산출물은 `target\release\awscli-rust.exe`다. 이 파일 하나와 `config.ini`만 옮기면 되고 .NET 런타임은 필요 없다.

### Linux (정적 musl 바이너리)

Windows 실행 파일(`.exe`)은 Linux에서 쓸 수 없다. Docker Desktop이 있는 Windows에서 저장소 루트의 `build-linux.ps1`(TESTCore `build.ps1`·`upload.ps1`과 같은 흐름)을 실행하면 `rust:alpine` 컨테이너에서 `x86_64-unknown-linux-musl` 정적 바이너리를 만들고 배포까지 한다. 대상 장비의 glibc 버전과 무관하게 실행되고 런타임 설치도 필요 없다.

```powershell
pwsh ./build-linux.ps1                 # 빌드 후 기본 대상(root@192.168.11.156:/root/workspace/awscli-rest)에 배포
pwsh ./build-linux.ps1 -SkipDeploy     # 빌드만
pwsh ./build-linux.ps1 -Targets root@192.168.31.103:/root/workspace/awscli-rust, root@192.168.31.104:/root/workspace/awscli-rust
```

1.  정리: `dist/linux/awscli-rust/`와 이전 압축 파일을 지운다(빌드 캐시 `target/linux`는 남긴다).
2.  빌드: 버전은 TESTCore와 같은 `태그_커밋수_해시`다(컨테이너에 `git`을 설치해 계산한다). 할당기는 musl 기본 할당기 대신 mimalloc을 쓴다(`v1.0.1`부터, [성능 분석](perf/2026-10-08-s3gw-155.md#put-분석)).
3.  추가 파일: `sample.ini`, `controller.sample.ini`, `worker.sample.ini`를 함께 둔다.
4.  압축: `dist/linux/awscli-rust_<버전>.tar.gz`(실행 권한 포함).
5.  배포: 대상마다 디렉터리가 없으면 만들고(`mkdir -p`), 파일을 올리고, 실행 권한을 준 뒤 `--version`으로 확인한다. 대상 장비의 `config.ini`는 올리지도 덮어쓰지도 않으므로 장비마다 직접 둔다. ssh 키 인증(`BatchMode`)이 되어 있어야 한다.

컨테이너 안에서 직접 만들 때는 다음과 같다.

```bash
apk add --no-cache musl-dev gcc git
cargo build --release --target x86_64-unknown-linux-musl
```

### 변경 후 검증

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 릴리스

버전 문자열은 TESTCore와 같은 `태그_커밋수_해시`(예: `v1.0.0_101_289204a`)이며 `--version`으로 확인한다. 태그가 없으면 `v0.0.0`으로 나온다.

1.  변경 후 검증(위)을 통과시키고 커밋한다.
2.  주석 태그를 단다: `git tag -a vX.Y.Z -m "awscli-rust X.Y.Z: <요약>"`
3.  `pwsh ./build-linux.ps1`로 빌드·배포한다. 압축본은 `dist/linux/awscli-rust_<버전>.tar.gz`다. Windows용은 `cargo build --release`.
4.  원격 저장소에 커밋과 태그를 올린다: `git push origin main vX.Y.Z`

| 버전 | 커밋 | 내용 |
| --- | --- | --- |
| `v1.0.0` | `289204a` | TESTCore(.NET, `dotnet-final`) 이식 완료. 성능 비교는 [docs/perf/2026-10-08-ksan.md](perf/2026-10-08-ksan.md) |
| `v1.0.1` | (태그 참조) | Linux 정적 빌드의 할당기를 mimalloc(메모리 즉시 반환)으로 변경: Put 처리량 .NET과 동등, 요청당 CPU 감소. [docs/perf/2026-10-08-s3gw-155.md](perf/2026-10-08-s3gw-155.md) |

## TESTCore에서 전환

-   `config.ini`와 명령행 인자는 TESTCore와 같다. 설정 파일은 그대로 쓰고, 실행 파일 이름만 `TestCore`에서 `awscli-rust`로 바꾼다.
-   `TestCore`(또는 `dotnet TESTCore.dll`)를 부르는 스크립트·작업 정의(Jenkins 등)를 `awscli-rust`로 바꾼다.
-   콘솔 출력, CSV·JSON 결과 파일, 종료 코드를 읽는 후처리는 그대로 동작한다.
-   원본과 다른 점은 [README의 알려진 차이](../README.md#알려진-차이)를 확인한다.

## 분산 실행 전환 순서

.NET과 Rust 구현은 Controller·Worker HTTP 계약이 같고 혼합 구성이 검증되어 있다([DISTRIBUTED.md](../DISTRIBUTED.md#혼합-구성)). 따라서 한 번에 바꾸지 않아도 된다.

1.  Worker를 하나씩 awscli-rust로 교체한다. 교체한 Worker는 `--worker -c worker.ini`로 다시 띄우고, Worker 설정(`worker.sample.ini`)은 그대로 쓴다. 교체할 때마다 .NET Controller로 짧은 Prepare·Get을 실행해 확인한다.
2.  모든 Worker를 교체한 뒤 Controller를 awscli-rust로 바꾼다(`--controller`, 설정 `controller.sample.ini`).
3.  문제가 생기면 해당 Worker·Controller만 .NET으로 되돌린다.

Worker는 한 번에 하나의 실행만 처리하므로, 교체는 진행 중인 실행이 없을 때 한다.

## 성능 비교 다시 하기

TESTCore와 awscli-rust를 같은 KSAN·같은 부하로 번갈아 실행해 처리량·평균 지연·최대 메모리를 비교한다.

1.  묶음을 만든다(Docker Desktop 필요). `-Ref`로 비교할 TESTCore 커밋을 고를 수 있다(기본 `HEAD`).
    ```powershell
    pwsh tools/perf/build-linux.ps1
    ```
    결과는 `dist/perf-bundle.tar.gz`(awscli-rust 정적 바이너리, self-contained TESTCore, `compare.sh`, `README.txt`)다.
2.  묶음을 클라이언트 테스트 장비에 올리고 푼다.
3.  묶음 디렉터리에 설정 파일 두 개를 둔다: `testcore.ini`, `awscli-rust.ini`. 접속 정보(`[Main User]` URL·AccessKey·SecretKey)와 `[Default] BucketName`만 쓰며 부하 값은 `compare.sh`가 실행용 사본에서 덮어쓴다. **자격 증명이 들어 있으므로 저장소에 커밋하지 않는다**(`ksan.ini`는 `.gitignore`에 있다).
4.  실행한다.
    ```bash
    ./compare.sh --net-ini testcore.ini --rs-ini awscli-rust.ini
    ```
    기본값은 3회, 스레드 32, 60초, 크기 1M, 파일 32개이며 `--rounds`, `--threads`, `--times`, `--size`, `--files`로 바꾼다. 버킷은 도구별로 `{BucketName}-perf-net`·`-perf-rs`를 만들고 끝나면 비워서 지운다.
5.  결과는 `results/summary.md`(표와 ±10% 판정), `results/runs.csv`(원시 값), `results/logs/`에 남는다.

.NET 쪽은 self-contained로 게시되어 런타임은 필요 없지만, Linux에서 `libicu`와 `libssl`이 설치되어 있어야 실행된다. awscli-rust는 정적 바이너리라 추가 라이브러리가 필요 없다.

## 롤백 (TESTCore 다시 빌드)

문제가 있으면 .NET TESTCore로 되돌린다. 전환 직전 최종 .NET 소스는 TESTCore 저장소의 태그 `dotnet-final`이다. 다음 명령은 해당 태그의 소스를 `git archive`로 임시 디렉터리에 내보내(TESTCore 작업 디렉터리는 건드리지 않는다) Release로 빌드하고 `TestCore.dll`이 있는 경로를 출력한다.

```powershell
pwsh tools/dotnet-oracle/build-testcore.ps1 -Ref dotnet-final
```

-   `-Ref`는 TESTCore의 빌드할 커밋·태그다(생략하면 `HEAD`). `-TestCore`로 저장소 경로, `-Out`으로 빌드 위치를 바꿀 수 있다.
-   출력 경로의 파일을 배포하고, 스크립트의 실행 파일 이름을 `TestCore`로 되돌린다. 설정 파일은 그대로 쓴다.
-   Linux용 self-contained 묶음은 `pwsh tools/perf/build-linux.ps1 -Ref dotnet-final`로 만든다.

## 기준 출력 갱신

TESTCore 쪽이 바뀌었거나 새 시나리오를 추가했을 때 parity 기준 출력을 다시 만드는 방법은 [tests/parity/README.md](../tests/parity/README.md)의 "기준 출력 다시 수집하기"를 따른다. 기준 출력은 항상 TESTCore HEAD(또는 비교하려는 커밋)로 빌드한 결과로 만들고, .NET과 섞어 도는 테스트는 `TESTCORE_BIN` 환경 변수에 그 빌드 경로를 지정한다.
