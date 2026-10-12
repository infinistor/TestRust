# parity 테스트

TESTCore(.NET)와 TestRust의 외부 동작을 비교한다.

-   `baseline/`: .NET으로 수집한 기준 출력. 콘솔 출력은 UTF-8로 저장하고, 로그 줄(`INFO `, `ERROR` 등)은 필요할 때만 남긴다.
    -   `version.txt`: `TestCore --version` 출력 (`78004ff` 빌드)
    -   `help.txt`: `TestCore --help` 출력에서 로그 줄을 뺀 것 (`78004ff` 빌드, 옵션 파서 비교용)
    -   `ini/*.json`: `ini/*.ini`를 TESTCore `IniFile`로 읽은 결과
    -   `config/*.json`: `config/*.ini`를 `Config.GetConfig`로 읽은 뒤 `Config.ToString()`한 JSON. 사용자 섹션을 지정한 경우 `<픽스처>.user-<이름>.json`(Windows는 파일 이름 대소문자를 구분하지 않으므로 이름이 겹치지 않게 한다). 로드에 실패한 입력은 `null`이다. `Default.FilePath`가 비어 있으면 원본이 현재 디렉터리를 쓰므로 `<CWD>`로 바꿔 저장했다.
    -   `portal/*.json`·`mover/*.json`·`zeromq/*.json`: 오라클의 같은 이름 명령(`portal`, `mover`, `zeromq`)이 실제 `PortalManager`·`MoverClient`·`ZeroMqClient`를 로컬 서버에 연결해 기록한 요청(요청 줄·헤더·본문), 결과(반환값·예외 형식과 메시지), log4net 로그. `variants`가 있는 사례는 응답 본문만 바꿔가며 JSON 읽기 경계(`System.Text.Json` 오류 메시지 포함)를 비교한다. 다시 만들 때는 `pwsh tests/parity/gen-client-baselines.ps1`.
    -   `cli/options.json`·`cli/help.txt`·`cli/usage.json`: 오라클 `cli-options`(OptionSet 옵션 표), `cli-help`(`WriteOptionDescriptions` 원문), `cli-usage`(`Usage` 공개 문자열 필드). `crates/cli/src/usage.rs`는 `usage.json`에서 만든다.
    -   `cli/parse.json`: 오라클 `cli-parse tests/parity/cli/parse-cases.json`. 사례마다 기본값과 다른 `CommandOptions` 속성, Extra, 예외(형식·메시지·`OptionName`).
    -   `distributed/writer.json`: 오라클 `distributed-writer <긴 경로의 빈 디렉터리>`(분산 Controller의 `ResultWriter`·`ResultConsoleFormatter`를 고정 표본으로 돌린 CSV·JSON·콘솔 로그·`--save` 경로 규칙). `Total.SampleAtUtc`는 `<NOW>`로 가린다. 경로에 8.3 이름이 섞이면 오류 메시지의 경로 가리기가 어긋나므로 긴 경로를 쓴다.
    -   `cli-run/`: `TestCore.exe`를 자식 프로세스로 실행해 정규화한 표준 출력·오류, 종료 코드, 서버가 받은 요청(`cli_run.rs`의 `generate`, 절차는 `docs/design/cli-dispatch.md`). `top.json`·`help.json`·`bare.json`은 자동 사례, 하위 디렉터리는 `cli/run/`의 같은 경로 사례.
-   `config/`: Config 픽스처(`-text`로 보관).
-   `portal/`, `mover/`, `zeromq/`: 위 기준 출력을 만드는 사례(`op`와 응답 본문 등). Portal·Mover 테스트는 이 디렉터리를 모두 읽는다.
-   `ini/`: INI 파서 픽스처. 줄 끝과 BOM을 그대로 보관하도록 `.gitattributes`에서 `-text`로 지정했다.
-   테스트 파일(`*.rs`)은 해당 크레이트의 `Cargo.toml`에 `[[test]]`로 등록한다.

## 기준 출력 다시 수집하기

`tools/dotnet-oracle`은 TESTCore 빌드 결과(`TestCore.dll`)를 그대로 호출해 기준 출력을 만든다.

**TESTCore 저장소의 `bin/TestCore`는 예전 커밋으로 빌드된 것일 수 있다.** 기준 출력은 반드시 기준 구현 태그 `dotnet-final`(`ec427f2`, 지금의 TESTCore HEAD)로 빌드한 결과로 만든다. `build-testcore.ps1`이 지정한 커밋(`-Ref`, 기본 `HEAD`)의 소스를 임시 디렉터리로 내보내(`git archive`, TESTCore는 건드리지 않음) 빌드하고 그 경로를 출력한다.

```powershell
$bin = pwsh tools/dotnet-oracle/build-testcore.ps1 -Ref dotnet-final
dotnet build tools/dotnet-oracle -p:TestCoreBin=$bin
$env:TESTCORE_BIN = $bin
dotnet tools/dotnet-oracle/bin/Debug/net10.0/DotnetOracle.dll ini tests/parity/ini/sample.ini > tests/parity/baseline/ini/sample.json
dotnet tools/dotnet-oracle/bin/Debug/net10.0/DotnetOracle.dll config tests/parity/ini/sample.ini
pwsh tools/dotnet-oracle/gen-updown-cases.ps1   # UpDownClient 사례와 기준 출력
```

기준 출력은 만든 시점의 TESTCore 커밋으로 빌드해 만들었다.

-   `78004ff`: `version.txt`, `help.txt`, ini·config·checksum·sign·uri·ksan·s3·json·stats. 그 뒤 바뀐 TESTCore 파일과 관계없어 그대로 쓴다.
-   `8d27dc2`: `updown/`, `local/`, `multisystem/`.
-   `c83e35f`·`3c4b0ea`·`ec427f2`: 사용자 결정으로 고친 원본 버그(README의 "TESTCore에서 함께 고친 원본 버그"). 관련 시나리오·분산 사례의 기준 출력은 고친 뒤의 빌드로 만든다.

.NET과 섞어 도는 테스트(`TESTCORE_BIN`이 있을 때만 도는 분산 E2E·Worker 비교 등)도 `dotnet-final` 빌드를 `TESTCORE_BIN`에 지정해 돌린다.

Windows에서 .NET 콘솔은 파이프 출력에 시스템 코드 페이지(CP949)를 쓰므로 `TestCore.exe` 출력은 UTF-8로 바꿔서 수집한다(오라클은 UTF-8로 출력한다).

```powershell
[Console]::OutputEncoding = [Text.Encoding]::UTF8
TestCore.exe --help
```
