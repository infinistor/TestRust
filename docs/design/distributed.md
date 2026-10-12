# 분산 실행 (`crates/distributed`)

TESTCore `Distributed/*`를 옮겼다. `--controller`가 여러 `--worker`에 같은 부하 테스트(Prepare·Put·Get·Delete·Mix)를 나눠 실행하고, 결과를 CSV·JSON으로 모은다. .NET Controller·Worker와 섞어 쓸 수 있도록 통신 계약, 상태 전이, 결과 파일 형식을 원본과 같게 맞췄다.

| 모듈 | 원본 |
| --- | --- |
| `contracts` | `Contracts.cs`(TestRequest·WorkloadSettings·WorkerStatus·StartRequest·RunSnapshot) |
| `settings` | `DistributedSettings.cs` |
| `worker` | `WorkerHost.cs`(WorkerManager·WorkerJob·axum `/driver` 서버) |
| `runner` | `TestRunner.cs`(DistributedTestRunner·BasicTestRunner) |
| `diagnostics` | `WorkerDiagnostics.cs` |
| `controller` | `Controller.cs` |
| `result_writer` | `ResultWriter.cs` |
| `console` | `ResultConsoleFormatter.cs` |
| `lib::run` | `DistributedApplication.cs` |
| `testrust_scenarios::run_control` | `RunControl.cs` |

## 통신 계약

-   serde 이름은 .NET 속성 이름(PascalCase)으로 둔다.
-   통신은 `testrust_common::{to_web_json, from_web_json}`을 쓴다. 쓸 때는 `JsonSerializerDefaults.Web`처럼 camelCase로 압축하고, 읽을 때는 첫 글자를 대문자로 바꿔 받는다.
-   Worker 결과 파일과 Controller 결과 JSON은 `to_dotnet_json`(PascalCase, 들여쓰기)으로 쓴다.
-   날짜 형식
    -   `DateTimeOffset`은 `DotnetDateTimeOffset`을 쓴다. JSON에서는 소수 끝의 0을 지우고 `+00:00`을 붙인다. CSV에서는 `"O"`(소수 7자리)로 쓴다.
    -   `DateTime`은 `DotnetDateTime::to_json_text`를 쓴다.
    -   System.Text.Json은 날짜를 인코더 없이 쓰므로 `+`를 이스케이프하지 않는다.
-   `tests/parity/distributed.rs`는 `tools/dotnet-oracle contracts`가 만든 .NET JSON과 글자 단위로 비교한다(직렬화와 왕복).

## Worker와 UpDownTest

-   `UpDownTest::with_control(RunControl)`이 원본 `_control != null` 분기다.
    -   스레드는 시작 게이트를 기다리고, 예외가 나면 프로세스를 끝내지 않고 `Stop(failed)`를 부른다.
    -   작업 시작은 `ReadyAndWait`(예약 시각까지 대기)다.
    -   감시 루프의 시간 제한은 예약 시각 기준 `DurationReached`다.
    -   버킷·파일 준비가 실패하면 예외다.
-   UpDownTest 시나리오 퓨처가 `Send`가 아니어서, Worker는 `spawn_blocking` 안에서 `Handle::block_on`으로 실행한다.

## TESTCore에서 고친 원본 버그 (`ec427f2`, 사용자 결정)

1.  `WorkerJob` 마무리: 실행기 자원 정리를 먼저 하고, 그 결과를 반영해 결과 파일을 저장한다.
2.  분산 실행의 `PrepareRandom`·`WriteRandom`(`--not-empty`): 공유 데이터셋 파일(Get ETag 기준) 대신 `.random` 파일을 덮어쓴다.
3.  `ResultWriter` 생성: CSV 생성이 실패하면 먼저 만든 결과 파일을 지운다.

## 비교 테스트

-   `parity_distributed`: 계약 JSON.
-   `parity_distributed_worker`: DistributedChecks의 Worker 쪽 검사와 HTTP API를 비교한다. `TESTCORE_BIN`이 있으면 .NET Worker와 같은 요청 순서로 비교한다.
-   `parity_distributed_controller`: .NET ResultWriter 출력(`baseline/distributed/writer.json`)과 비교하고, 가짜 Worker로 Controller 흐름을 확인한다.
-   `parity_distributed_e2e`(cli)
    -   상태 있는 가짜 S3(`support/mock_s3.rs`)를 쓴다. Worker 2개와 Controller 프로세스로 Prepare → Get(ETag) → Put → Mix → Delete를 실행한다.
    -   Rust 단독은 항상 돌린다. `TESTCORE_BIN`이 있으면 .NET 단독, .NET Controller + Rust Worker, Rust Controller + .NET Worker도 돌려 결과 JSON 구조와 S3 객체를 비교한다.
-   `cli/run/distributed/`: 인자·설정·상태 확인 오류 경로의 실행 비교.

## 원본 특이점(그대로 둔다)

-   최종 JSON의 `Workers` 순서는 처음 응답한 순서다.
-   집계의 `FileSize`는 마지막 Worker의 값이다.
-   `Total.State`는 Worker 상태 집계라서 전체 `State`(예: Cancelled)와 다를 수 있다.
-   `Validate`는 FileSize 0을 허용한다.
-   중복 실행 판정 해시는 받은 요청을 읽어 다시 쓴 JSON으로 계산한다.

## 알려진 차이

-   CSV 실수의 `1E+15` 이상 지수 표기는 다루지 않는다(통계 값이 그만큼 커지지 않는다).
-   닫힌 S3 포트로 제출 직후 중단하면, 시작 속도에 따라 .NET은 Cancelled, Rust는 Failed로 끝날 수 있다. 두 경로 모두 원본 코드에 있다.
