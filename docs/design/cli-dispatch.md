# CLI 실행 흐름과 명령 디스패치

TESTCore `Util/TestCoreApplication.cs`, `Util/ConfigBootstrapper.cs`, `Commands/CommandDispatcher.cs`를 `crates/cli/src/`로 옮긴 구조와 규칙이다.

## 구조

| 파일 | 원본 |
| --- | --- |
| `app.rs` | `TestCoreApplication.Run` (파싱 오류, 분산 실행, 버전, 설정 로드, 클라이언트 생성, 디스패치, `Main complete time`) |
| `bootstrap.rs` | `ConfigBootstrapper.Load` |
| `options/` | `Cli/CliOptionParser.cs`(Mono.Options 규칙), `HelpWriter` |
| `usage.rs` | `Data/Usage.cs` (오라클 리플렉션 값) |
| `dispatch/mod.rs` | `CommandDispatcher.Execute`: `CommandContext`, `CommandError`, 메뉴 → 모듈 배정 |
| `dispatch/output.rs` | 공용 출력(`LINE`, `PadRight`, 날짜 형식, SDK 응답 JSON 덤프) |
| `dispatch/<묶음>.rs` | `switch`의 `case`들 |

메뉴 묶음

| 모듈 | 메뉴 |
| --- | --- |
| `bucket` | CreateBucket, DeleteBucket, HeadBucket, ListBuckets, ListDirectoryBuckets, GetBucketLocation, Get/PutBucketVersioning, Get/Put/DeleteBucketOwnershipControls |
| `acl` | Get/PutBucketAcl, Get/PutObjectAcl |
| `multipart` | Abort/Complete/CreateMultipartUpload, ListMultipartUploads, ListParts, UploadPart, UploadPartCopy |
| `bucket_config` | Analytics, Cors, Encryption, Inventory, Metrics(Get/Put/Delete/List), Logging, Notification(Get/Put), Website |
| `bucket_rules` | PublicAccessBlock, Policy(+Status), BucketTagging, Lifecycle, Replication |
| `object` | CopyObject, Delete/DeleteObjects/DeleteObjectTagging, GetObject(+LegalHold/Lock/Retention/Tagging), GetPresignedUrl, HeadObject, RestoreObject, ListObjects(V2)/ListObjectVersions, PutObject(s)(+LegalHold/Lock/Retention/Tagging), StorageMove |
| `ksan` | Delete/Get/PutBucketTagIndex, ListBucketTagSearch |
| `backend` | S3backendPause/Resume |
| `util` | Set/DelObjectLock, Encryption, Upload, Download, Clear, Bucket/Current/Noncurrent/MarkerClear |
| `tests` | 테스트 시나리오(`Test/*`), RangeReadCopy, ManualUpload. 도움말·실행 전 검증은 이 모듈, 시나리오 실행은 `crates/scenarios`([scenarios.md](scenarios.md)) |

각 모듈은 `pub(super) async fn run(ctx, menu) -> CommandResult`와 옮긴 메뉴 목록 `PORTED`를 둔다. 모듈이 커지면 `<묶음>/mod.rs`와 하위 파일(입력 DTO 등)로 나눈다.

## 규칙

-   원본 `case` 순서대로 `if (help)` → 필수 값 검증(`else if (string.IsNullOrWhiteSpace(x)) Console.WriteLine(Usage.ERROR_X)`) → 실행을 옮긴다.
-   `Console.WriteLine` → `println!`, `_log.Info/Error` → `tracing::info!/error!`. 원본 지역 변수는 `ctx.options`의 같은 이름 필드(`marker`는 `darker`).
-   도움말 문자열은 `usage::main_flag`/`sub_flag`/`optional`/`optional_value`로 원본과 같은 순서로 만든다. `perl tools/dispatcher-cases.pl <시작 줄> <끝 줄>`이 C# 식을 Rust 식으로 바꿔 준다.
-   도움말의 예제 JSON(`JsonSerializer.Serialize(example, jsonOptions)`)은 .NET 출력(`baseline/cli-run/help.json`)을 문자열 상수로 그대로 쓴다.
-   SDK 응답 JSON 덤프(`if (print) Console.WriteLine(JsonSerializer.Serialize(response.X, jsonOptions))`)는 `output::print_json(&x)`. .NET과 글자 단위로 맞추지 않는다(사용자 결정). 이런 사례는 `"dump": true`.
-   응답 상태 비교(`response.HttpStatusCode == HttpStatusCode.OK`)는 `response.status == 200`. 실패 로그의 상태 이름은 `testrust_common::dotnet_http::status_name`.
-   S3 예외는 `?`로 `CommandError`가 된다. 최상위가 `ERROR` 로그(`형식: 메시지`)를 남기고 -1로 끝낸다(`Main complete time` 없음). 그 밖의 .NET 예외는 `CommandError::new(".NET 형식", "메시지")`.
-   입력 JSON 파일(`JsonSerializer.Deserialize<T>`)은 `testrust_common::json`의 `FromJson`(System.Text.Json 읽기 규칙, `JsonException` 메시지 포함)으로 읽는다.
-   날짜: `ToString("yyyy-MM-dd HH:mm:ss", InvariantInfo)`는 `output::invariant_time`, 기본 `ToString()`은 ko-KR 형식 `output::ko_kr_time`(사용자 결정). .NET SDK v4는 응답 시각을 UTC로 읽으므로 둘 다 UTC로 쓴다.
-   S3 오류의 예외 형식: `?`(`From<S3Error>`)는 서비스 오류를 모두 `AmazonS3Exception`으로 만든다. 연산이 전용 예외로 모델링한 코드가 있으면 `CommandError::s3(e, &["NoSuchKey"])`.
-   SDK 모델이 필수로 요구하지만 .NET은 생략하는 값은 `testrust_s3::UNSET`을 넣는다. S3 클라이언트가 서명 전에 그 요소·속성을 지운다(`mutate = strip_unset`).
-   공용 입력 처리(`IsNullOrWhiteSpace`, `File.ReadAllText`, 파일 예외, `NullReferenceException`)는 `dispatch::input`.
-   원본 버그는 고치지 않는다. 모듈 문서 주석에 적는다.

## 실행 비교 (`tests/parity/cli_run.rs`)

`support/cli_harness.rs`가 사례마다 캡처 서버와 임시 작업 디렉터리(`config.ini`, 입력 파일)를 만들고 실행 파일을 자식 프로세스로 돌린다. 표준 출력·오류(로그 시각, `Nms`, 서버 주소, 작업 디렉터리, 스택 추적 줄 정규화), 종료 코드, 서버가 받은 요청(메서드·경로·쿼리·`x-` 헤더·본문)을 비교한다.

-   자동 사례: `baseline/cli-run/top.json`(최상위 흐름), `help.json`(메뉴마다 `--X --help`), `bare.json`(S3·KSAN 메뉴를 인자 없이 실행).
-   파일 사례: `tests/parity/cli/run/<모듈>/<이름>.json` → 기준 출력 `tests/parity/baseline/cli-run/<모듈>/<이름>.json`.

```json
{
  "args": ["--get-bucket-acl", "-b", "bucket1"],
  "dump": true,
  "routes": [{ "contains": "GET /bucket1?acl", "status": 200, "responseBody": "<AccessControlPolicy>...</AccessControlPolicy>", "responseHeaders": { "Content-Type": "application/xml" } }],
  "default": { "status": 404, "responseBody": "<Error><Code>NoSuchKey</Code></Error>" },
  "files": { "acl.json": "{ ... }" },
  "unordered": false,
  "config": "[Main User]\r\nURL = {URL}\r\n..."
}
```

`HEAD` 응답의 `responseHeaders`에 `Transfer-Encoding`을 넣으면 서버가 `Content-Length`를 붙이지 않는다(크기 헤더 없는 응답). `routes`는 요청 줄에 `contains`가 들어 있는 첫 항목, 없으면 `default`(기본 빈 200)로 응답한다. 요청은 받은 순서대로 비교한다. 멀티파트 전송·버킷 비우기처럼 요청을 동시에 보내는 사례는 `"unordered": true`로 순서를 무시한다. `config`를 주지 않으면 `cli_harness::DEFAULT_CONFIG`(버킷 이름 없음)를 쓴다.

시나리오 사례(`cli/run/scenarios/<시나리오>/`)용 옵션:

-   `"stats": true`: 평균·대역폭·시간을 담은 통계 줄과 결과 JSON의 숫자(단위 포함)를 `<N>`으로 가린다. 건수 줄은 그대로 비교한다.
-   `"ignore_body": true`: XML이 아닌 요청 본문은 MD5 없이 길이만 비교한다(무작위 더미 파일·본문).
-   `"drop_lines": "정규식"`: 정규화한 출력 줄 중 맞는 줄을 버린다(시간에 따라 횟수가 달라지는 줄).
-   `"mask": "정규식"`: 출력·요청 경로에서 맞는 부분을 `<RAND>`로 바꾼다(무작위 버킷 이름 등).
-   `"db_rows": [[1, 10], null, ...]`: 가짜 MySQL 서버(`support/fake_mysql.rs`)를 띄워 `SELECT filecount, used FROM bucket ...`마다 이 행을 차례로 돌려준다(`null`이나 소진은 행 없음). 설정의 `{DBPORT}`가 서버 포트로 바뀌고, 받은 사용량 조회 SQL이 `outputs`의 `<db>`로 기록된다.
-   `"dirs": ["in", "out/a"]`: 작업 디렉터리에 빈 디렉터리를 만든다.
-   `"outputs": ["result.json", "save"]`: 실행 뒤 파일(디렉터리면 아래 파일 전부) 내용을 비교한다. 시각은 `<TIME>`, 파일 이름의 `yyyyMMdd_HHmmss`는 `<TS>`.
-   동시 요청 사례(`unordered`)의 기준 출력은 요청을 정렬해 저장한다.
-   `"delays": [{ "contains": "PUT /bkt", "ms": 1300 }]`: 요청 줄에 `contains`가 들어 있는 요청의 응답을 `ms`만큼 늦춘다. 시간 제한(`Times`) 시나리오가 `Times`(1초)가 지난 뒤에 첫 요청이 끝나게 해 스레드마다 요청이 정확히 하나만 나가게 한다.
-   `stats` 사례는 비교할 때 `<N>`이 든 줄의 연속 공백을 하나로 본다(값 길이에 따라 맞춤 공백이 달라진다).
-   요청 경로·출력의 시각 키(`2026/10/07/16/03/`, `2026/10/07/`)는 `<YYYY/MM/DD/HH/mm>/`, `<YYYY/MM/DD>/`로 가린다(AWSTest 등).

기준 출력 만들기(TESTCore HEAD 빌드, `tests/parity/README.md`):

```powershell
$env:TESTCORE_BIN = pwsh tools/dotnet-oracle/build-testcore.ps1 | Select-Object -Last 1
$env:CLI_RUN_FILTER = "bucket/"   # 내 모듈 사례만 (자동 사례 묶음은 건드리지 않는다)
cargo test -p testrust-cli --test parity_cli_run -- --ignored generate
cargo test -p testrust-cli --test parity_cli_run
```

원본 디스패처의 `case`는 모두 옮겼다. `PORTED`에 없는 메뉴는 원본에도 `case`가 없는 것(`ReplicationTest`, `GetTestAll`, `AccessKey`·`SecretKey`·`URL`·`MaxCount`)뿐이다. `dispatch::is_ported`가 `false`인 메뉴의 사례는 비교하지 않으므로, 메뉴를 새로 추가하면 해당 모듈의 `PORTED`에도 넣는다.
