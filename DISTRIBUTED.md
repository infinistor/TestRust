# Controller·Worker 분산 부하 테스트

Worker는 HTTP 서버로 실행하고 Controller는 기존 TestRust 명령으로 테스트를 전달합니다. Controller는 테스트가 끝날 때까지 준비 상태, heartbeat, 통계와 최종 결과를 수집합니다. S3 요청은 각 Worker에서 직접 전송합니다.

## 설정과 실행

`worker.sample.ini`를 각 Worker에 복사하고 `name`과 경로를 설정합니다. `controller.sample.ini`에는 S3 접속 정보와 Worker 목록을 지정합니다. 기존 `config.ini`에 같은 섹션을 추가해도 됩니다.

```ini
[controller]
drivers = 2
ResultPath = /var/log/test/

[driver1]
name = driver1
url = http://192.168.31.191:18088/driver

[driver2]
name = driver2
url = http://192.168.31.192:18088/driver
```

Worker별 사용자를 지정하지 않으면 Controller의 S3 접속 정보를 사용합니다.

```ini
[worker]
name = driver1
url = http://0.0.0.0:18088/driver
WorkPath = ./worker-data
ResultPath = ./worker-results
LeaseTimeoutSeconds = 15
```

Worker별 버킷 접미어는 `[worker]`에 `BucketSuffix = -user1`처럼 설정합니다. Controller가 전달한 버킷 이름이 `test-bucket`이면 `test-bucket-user1`을 사용합니다. 구분자를 자동으로 추가하지 않으므로 필요한 하이픈도 값에 포함합니다. 옵션을 생략하거나 값을 비워 두면 기존 이름을 사용하며, 값 앞뒤 공백은 제거합니다. Prepare·PUT·GET·DELETE·MIX 모두 같은 규칙을 적용합니다. BucketType=Thread에서는 `{BucketName}{BucketSuffix}-{WorkerId}-{ThreadIndex:0000}`이 됩니다. 접미어를 포함한 최종 버킷 이름은 기존 S3 이름 검증을 거치며, `--debug`로 실제 적용된 이름을 확인할 수 있습니다.

Worker별로 다른 S3 사용자를 사용하려면 해당 Worker가 읽는 INI 파일에 기존 `[Main User]` 섹션을 추가합니다.

```ini
[Main User]
URL = http://192.168.31.100:9000
RegionName =
AccessKey = worker-specific-access-key
SecretKey = worker-specific-secret-key
```

우선순위는 **Worker 로컬 `[Main User]` → Controller가 전달한 사용자 정보**입니다. URL·RegionName·AccessKey·SecretKey 전체를 한 묶음으로 적용하며 필드별로 섞지 않습니다. 로컬 섹션이 있으면 URL·AccessKey·SecretKey가 모두 필요하고 RegionName만 생략하거나 비울 수 있습니다. 불완전한 로컬 섹션은 Worker 시작 오류로 처리합니다. Controller 설정으로 돌아가려면 로컬 `[Main User]` 섹션 전체를 제거하고 Worker를 재시작합니다.

Controller는 각 Worker의 로컬 사용자 설정 여부를 먼저 확인합니다. 로컬 설정을 쓰는 Worker에는 Controller 자격 증명을 전송하지 않습니다. 모든 Worker가 로컬 사용자를 설정했다면 Controller의 `[Main User]`는 생략할 수 있습니다. 혼합 구성에서는 로컬 설정이 없는 Worker를 위한 Controller 사용자 정보가 필요합니다. 실행 시 설정 출처만 출력하며 자격 증명은 상태 응답이나 결과에 포함하지 않습니다.

```text
TestRust --worker --config=worker.ini
TestRust --controller --test-prepare --config=config.ini
TestRust --controller --test-get --config=config.ini
TestRust --controller --test-put --config=config.ini
TestRust --controller --test-mix --config=config.ini
TestRust --controller --test-delete --config=config.ini
```

TestRust는 단일 실행 파일이므로 별도 런타임이 필요 없습니다. Worker는 내부 테스트망의 신뢰된 Controller가 접근하는 서비스를 전제로 합니다. 인터넷 공개용 인증·권한 체계는 포함하지 않습니다.

각 Worker는 하나의 실행만 처리합니다. `--controller` 없는 기존 CLI 명령은 로컬에서 실행합니다. 현재 분산 모드에서 지원하는 명령은 위의 다섯 가지입니다. `driver` 설정 이름은 COSBench의 형태를 따르지만 통신 프로토콜은 TESTCore·TestRust 전용입니다.

## 부하와 동기화

Worker 설정을 확인하려면 `TestRust --worker -c worker.ini --debug`로 실행합니다. 시작 시 설정 파일의 절대 경로, Worker 이름·주소, 작업·결과 경로, lease와 로컬 S3 접속 설정을 출력합니다. 로컬 사용자가 없으면 Controller의 테스트 요청을 기다린다고 표시합니다. 테스트를 수신하면 실제 적용된 S3 접속 설정, 부하 설정, Worker ID가 반영된 버킷·prefix를 출력합니다. AccessKey·SecretKey는 `***`로 표시하며 URL의 사용자 정보·쿼리·fragment는 제외합니다. Controller 모드의 `--debug` 출력은 지원하지 않습니다.

- Controller의 `[Default]`, `[UpDown]`, `[Main User]`와 CLI override로 실행 요청을 만듭니다. Worker의 로컬 `[Main User]`가 있으면 S3 접속 정보는 로컬 설정을 우선 적용합니다. 명령 종류·옵션도 필요하므로 `UpDownConfig`만 전송하지 않습니다.
- `ThreadCount`는 Worker별 스레드 수, `FileCount`는 스레드별 개수입니다. 4개 Worker에 ThreadCount=10이면 총 40스레드입니다.
- `FileCount`는 Prepare·GET에서만 양수가 필요합니다. PUT·MIX·DELETE에서는 생략할 수 있습니다. PUT·GET·MIX의 실행 시간은 `Times` 또는 `--times`로 지정합니다. 부하 설정 범위 오류에는 잘못된 항목명·값·허용 범위를 표시합니다.
- Worker 이름은 1~40자의 소문자 영숫자·하이픈입니다. 이름과 주소는 중복될 수 없고 실제 Worker 이름과 일치해야 합니다.
- 기본 ThreadPrefix=TH, Worker=driver1이면 실제 스레드 prefix는 `TH/driver1_000`입니다. RunId를 객체 이름에 자동으로 넣지 않아 별도 Prepare·GET·DELETE 사이에 데이터를 재사용할 수 있습니다.
- BucketType=Thread는 `{BucketName}-{WorkerId}-{ThreadIndex:0000}` 버킷을 사용합니다. 공유 버킷의 DELETE는 해당 스레드 prefix로 제한합니다.
- Worker는 초기화를 끝내고 모든 스레드가 게이트에 도달하면 Ready가 됩니다. Controller가 전체 Ready를 확인한 뒤 공통 UTC 시작 시각을 전달합니다.
- 서버 시계는 운영 환경에서 동기화해야 합니다. 실행 시간은 단조 시계로 측정하며, 실제 시작 시각과 Worker 간 시작 편차를 기록합니다. 네트워크 단절 상황에서 원자적인 전체 동시 시작을 보장하지는 않습니다.
- PUT·GET·MIX는 공통 종료 시각에 신규 작업을 중단하고 진행 중 요청을 정리합니다. Prepare·DELETE는 기존 개수·완료 조건을 따릅니다.
- ETag 검사에 사용할 원본 더미 파일은 `WorkPath/datasets/{설정 해시}`에 보존합니다. 동일 Worker·S3 주소·버킷·prefix·크기로 Prepare 후 GET을 실행해야 합니다. 파일을 지우면 ETag 검사 GET은 준비 단계에서 실패합니다. 매번 다른 본문을 쓰는 random 테스트와 고정 원본 ETag 검사는 함께 사용하지 마세요.

Controller 시간 설정의 기본값(초):

| 설정 | 기본값 | 의미 |
| --- | ---: | --- |
| PrepareTimeoutSeconds | 300 | 전체 Ready 대기 제한 |
| StartDelaySeconds | 5 | 전체 Ready 후 예약 시작까지 여유 |
| RequestTimeoutSeconds | 5 | HTTP 요청 제한 |
| PollIntervalSeconds | 2 | 진행 상태 수집·heartbeat 간격 |
| LeaseTimeoutSeconds | 15 | Controller 연결 상실 시 중단 기준 |

Controller lease는 조회 간격과 요청 제한 시간의 합보다 커야 하며 Worker의 lease 상한을 넘을 수 없습니다. Worker 실행 오류나 지속적인 통신 단절 시 Controller는 전체 중단을 요청합니다. Ctrl+C도 동일하게 처리합니다. Controller가 사라지면 Worker는 자체 lease 검사로 중단합니다. 개별 S3 요청 실패는 별도 통계이며 Worker 실행기 오류와 구분합니다.

중단은 진행 중 S3 요청을 즉시 강제 종료하는 의미가 아닙니다. 요청 정리가 끝나기 전에는 새 테스트를 받지 않습니다. Controller의 중단 후 결과 대기는 최대 30초이며, 그 이후 응답이 없는 Worker는 마지막 확인 상태만 남습니다. Worker 재시작 시 진행 중 실행은 재개하지 않습니다.

## 결과 경로와 CSV

Controller 결과 경로 우선순위는 `--save` → `[controller] ResultPath` → `./results`입니다. 상대 경로는 사용한 INI 파일 디렉터리를 기준으로 해석합니다. 폴더를 자동 생성하고 테스트 전 쓰기 가능 여부와 파일 충돌을 확인합니다.

```text
/var/log/test/{RunId}.json  최종 결과와 Worker별 결과
/var/log/test/{RunId}.csv   준비부터 완료까지의 중간 통계
```

`--save=report.json`이면 `report.json`과 `report.csv`를 함께 생성합니다. 기존 파일은 덮어쓰지 않습니다. Worker 최종 결과는 Worker의 `[worker] ResultPath`에 별도로 남깁니다. 자격 증명은 결과 파일에 저장하지 않습니다.

CSV는 UTF-8 BOM, 고정 헤더, UTC ISO 8601 시각, 소수점 `.`을 사용하며 매 수집 회차마다 flush합니다. 각 회차에 Worker별 한 행과 전체 합산 한 행을 저장합니다. 콘솔은 기존 INFO 로그 형식으로 전체 합산 통계와 `Worker Count`(응답 수/설정 수), 실행 상태를 표시합니다. 중간 결과의 `Write Count : 162634 (+ 17468)`처럼 누적 성공 수 옆에 직전 수집 이후의 증가량을 표시합니다. Read·Delete도 동일하며, 증분은 초당 값이 아닌 수집 구간 전체의 건수입니다. 첫 수집·Worker 응답 누락·누락 후 첫 복구 수집에서는 증분과 중간 처리량을 N/A로 표시합니다. 중간 결과는 수집 구간 처리량을, 최종 결과는 누적 성공 수/전체 실행 시간으로 계산한 평균을 표시하며 대역폭 단위는 MiB/s입니다. Worker 응답이 누락되면 부분 통계임을 표시합니다. 종료·취소·실패 시 최종 결과 블록은 한 번 출력합니다.

| 열 | 의미 |
| --- | --- |
| RunId, SampleId, TestType | 실행과 수집 회차 |
| Scope, WorkerId | worker 또는 total, Worker 식별자 |
| CollectedAtUtc, WorkerSampleAtUtc | Controller 수집 시각과 Worker 스냅샷 시각 |
| ElapsedSeconds, IntervalSeconds | 실제 부하 경과 시간과 구간 계산 간격 |
| State, Available, IsFinal, Error | 실행 상태·조회 성공 여부·최종 여부·오류 |
| Read/Write/Head/Delete/List + Success/Failed | 연산별 누적 건수 |
| Read/Write/Head/Delete/List + OpsPerSecond | 이전 유효 스냅샷 대비 구간 성공 처리량 |
| EstimatedReadBytesPerSecond, EstimatedWriteBytesPerSecond | 성공 건수 × 설정 FileSize로 추정한 속도; 실제 네트워크 바이트 수가 아님 |
| ExpectedWorkers, ReportedWorkers | 설정 Worker 수와 이번 회차 조회 성공 수 |

첫 스냅샷의 구간 처리량은 비워 둡니다. 조회 실패는 빈 통계로 표현하며 0으로 간주하지 않습니다. 전체 누적값에는 Worker별 마지막 확인값을 사용하고 일부 조회 실패 시 전체 구간 처리량은 비웁니다. Worker별 조회 시각은 미세하게 다르므로 전체 구간 처리량은 Controller 수집 회차 기준의 근사치입니다.

MIX 최종 콘솔 결과에는 `Read Average`, `Write Average`, `Total Average`를 표시합니다. 각각 전체 Worker의 읽기 성공 수, 쓰기 성공 수, 두 성공 수의 합을 전체 실행 시간으로 나눈 값입니다. 중단 후 진행 중인 요청을 정리하는 구간에서는 새로운 쓰기 완료 없이 읽기만 완료될 수 있으므로 중간 `Write Average`가 0이어도 최종 평균은 0이 아닐 수 있습니다.

최종 JSON은 실제 시작부터 마지막 요청 정리 완료까지의 총 경과 시간을 사용합니다. 준비·예약 대기 시간은 부하 시간에 포함하지 않습니다. 실패·취소된 실행도 부분 결과를 저장합니다. CSV 기록이 실패하면 전체 테스트를 중단하고 가능한 최종 JSON을 저장합니다.

## 혼합 구성

.NET(TESTCore)과 Rust(TestRust) 구현은 같은 통신 계약을 쓰므로 섞어서 운영할 수 있습니다.

-   .NET Controller + Rust Worker
-   Rust Controller + .NET Worker

`tests/parity/distributed_e2e.rs`가 상태 있는 가짜 S3와 Worker 2개로 Prepare → Get(ETag) → Put → Mix → Delete를 실행해 두 혼합 구성의 결과 JSON 구조와 S3 객체를 비교합니다(.NET 빌드 경로를 `TESTCORE_BIN`에 지정해야 .NET 쪽이 포함됩니다). 계약 JSON(TestRequest·WorkloadSettings·WorkerStatus·StartRequest·RunSnapshot)은 `tests/parity/distributed.rs`가 .NET 출력과 글자 단위로 비교합니다. 자세한 내용은 [docs/design/distributed.md](docs/design/distributed.md)를 참고하세요.

알려진 차이는 [README](README.md#알려진-차이)에 있습니다. Worker를 하나씩 교체하는 순서는 [운영 절차](docs/operations.md)를 참고하세요.

## 검증

```text
cargo test --workspace
```

회귀 검사는 임시 디렉터리, 루프백 HTTP 서버, 가짜 S3로 수행하며 실제 S3 자격 증명이 필요 없습니다. 설정·예약 시작·중복 요청·lease·CSV, Worker 2개와 가짜 S3를 연결한 실행을 검증합니다. 운영 S3의 성능·시계 동기화·방화벽 구성은 별도 환경에서 확인해야 합니다.
