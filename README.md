# TestRust

Controller·Worker 분산 부하 테스트와 CSV 통계 수집은 [분산 테스트 사용법](DISTRIBUTED.md)을 참고하세요. 설정 예제: [일반](sample.ini), [Controller](controller.sample.ini), [Worker](worker.sample.ini). 빌드·배포·전환·롤백 절차는 [운영 절차](docs/operations.md)에 있습니다.

Claude Code에서 부하 테스트 실행·서버 모니터링·결과 수집을 자동화하려면 [부하 테스트 스킬](#claude-code-부하-테스트-스킬-load-test)을 참고하세요.

S3 Compatible 스토리지에 접근하고 기능·성능을 검증하기 위한 CLI 도구. 기존 .NET 기반 TESTCore를 Rust로 옮긴 것으로, `v1.0.0`에서 이식을 마쳤다(.NET 기준 구현은 TESTCore 태그 `dotnet-final`). 이식 당시 계획은 [RUST_MIGRATION.md](RUST_MIGRATION.md)에 있다.

## 용도

-   CLI로 S3 Compatible 스토리지에 접근하기 위한 솔루션
-   KSAN 확장 API 지원 (tagindex / tagsearch, admin 모드)
-   부하 테스트 및 기능 검증 테스트 제공
-   ifsMover 테스트 지원

## 빌드 및 실행

```bash
git clone https://github.com/infinistor/TestRust.git
cd TestRust
cargo build --release
```

-   요구 사항: Rust stable. Windows에서는 Visual Studio Build Tools의 C++ 워크로드(MSVC 타깃 `x86_64-pc-windows-msvc`)가 필요하며, cargo는 PowerShell에서 실행한다(Git Bash의 `/usr/bin/link`가 MSVC `link.exe`를 가려 링크가 실패한다).
-   Release 빌드 산출물: `target/release/TestRust`(Windows는 `TestRust.exe`). 런타임 설치는 필요 없다.
-   Windows 실행 파일(`.exe`)은 Linux에서 쓸 수 없다. Linux용 정적 바이너리(musl)는 Docker Desktop이 있으면 `pwsh ./build-linux.ps1`로 빌드·압축하고 테스트 장비에 배포한다(기본 대상 `root@192.168.11.156:/root/workspace/TestRust`, `-SkipDeploy`면 빌드만, 대상은 `-Targets`로 지정. [운영 절차](docs/operations.md#linux-정적-musl-바이너리) 참고). `rust:alpine` 컨테이너에서 직접 만들 때는 다음과 같다.
    ```bash
    cargo build --release --target x86_64-unknown-linux-musl
    ```

```bash
TestRust --list-buckets
TestRust --put-object -b test-bucket -k test.txt -f ./test.txt
TestRust --test-prepare --thread 10 --count 1000 --size 1M
```

-   각 명령의 세부 사용법은 해당 명령과 `-?`, `-h`, `--help`를 함께 지정하면 출력된다.
    ```bash
    TestRust --put-object --help
    ```
-   옵션 없이 `--help`만 지정하면 아래 [전체 옵션](#전체-옵션) 목록이 출력된다.

## 설정 파일

기본적으로 실행 디렉터리의 `config.ini`를 읽는다. `-c, --config=<경로>`로 변경할 수 있으며, **설정 파일을 읽지 못하면 실행이 중단된다.** `sample.ini`를 복사해서 사용한다.

### 사용자 섹션

| 섹션 | 키 | 설명 |
| --- | --- | --- |
| `[Main User]` | `URL`, `AccessKey`, `SecretKey`, `RegionName` | 기본 대상 사용자 |
| `[Alt User]` | `URL`, `AccessKey`, `SecretKey`, `RegionName` | 비교/복제 테스트의 상대 사용자 |

`--user=<섹션 이름>`을 지정하면 해당 이름의 섹션을 Main User 대신 사용한다. 따라서 사용자 섹션은 임의의 이름으로 여러 개 정의할 수 있다.

### 기능별 섹션

| 섹션 | 키 |
| --- | --- |
| `[Default]` | `BucketName`, `ThreadPrefix`, `ObjectPrefix`, `FilePath`, `TargetPath`, `FileSize`, `PartSize`, `Retry`, `IsAdmin` |
| `[UpDown]` | `ReadRatio`, `WriteRatio`, `DeleteRatio`, `ThreadCount`, `FileCount`, `DivisionCount`, `Times`, `BucketType`, `ETagCheck`, `UseChunkEncoding` |
| `[ListObj]` | `Prefix`, `Prepare`, `Times`, `FileCount`, `ThreadCount` |
| `[Compare]` | `SourceBucket`, `TargetBucket`, `DeleteMarker`, `ETagCheck`, `ChecksumCheck`, `VersionCheck`, `MetadataCheck`, `ReplicationCheck`, `TagCheck` |
| `[Mover]` | `URL`, `User`, `SourceBucket`, `TargetBucket`, `FileCount`, `MaxFileSize` |
| `[UsedSize]` | `BucketPrefix`, `FileSize`, `PartSize` |
| `[Duplicate]` | `ObjectPath`, `LoopCount`, `ObjectCount` |
| `[MultiSystem]` | `MultiGatewayURL`, `OldSystemURL`, `NewSystemURL`, `FileCount`, `ThreadCount`, `BucketType` |
| `[AccessIps]` | `S3URLs`, `Volume`, `MainUser`, `SubUser`, `BucketPrefix`, `TestListPath`, `AllFailed`, `JenkinsTestName`, `Password` |
| `[DB]` | `Host`, `Port`, `User`, `Password`, `Database` |
| `[Portal]` | `URL`, `ApiKey` |
| `[Jenkins]` | `Host`, `Port`, `JenkinsPort`, `User`, `Password`, `Database` |

`FileSize`, `PartSize`는 `1K`, `10M`, `1G` 형식의 단위 표기를 사용한다.

`[UpDown] BucketType` 값

| 값 | 동작 |
| --- | --- |
| 0 | 설정 없음 |
| 1 | 단일 버킷 업로드 |
| 2 | 스레드당 버킷 |
| 3 | 날짜당 폴더 |
| 4 | Prefix |
| 5 | 현재 시각 기준 생성 |

### 명령행 우선 적용 옵션

아래 옵션을 지정하면 설정 파일 값을 덮어쓴다.

| 옵션 | 덮어쓰는 설정 |
| --- | --- |
| `--admin` | `[Default] IsAdmin` |
| `-b, --bucket` | `[Default] BucketName` |
| `--thread-prefix` | `[Default] ThreadPrefix` |
| `--prefix` | `[Default] ObjectPrefix` |
| `--size` | `[Default] FileSize` |
| `-p, --path` | `[Default] FilePath` |
| `--target-path` | `[Default] TargetPath` |
| `--bucket-type` | `[UpDown] BucketType` |
| `--thread` | `[UpDown] ThreadCount` |
| `--count` | `[UpDown] FileCount` |
| `--times` | `[UpDown] Times` |
| `-r, --read` / `-w, --write` / `-d, --delete` | `[UpDown] ReadRatio` / `WriteRatio` / `DeleteRatio` |
| `--md5sum` | `[UpDown] ETagCheck` |
| `--use-chunk-encoding` | `[UpDown] UseChunkEncoding` |
| `-s, --save` | 결과 저장 경로 |
| `--url` / `--access-key` / `--secret-key` | Main User의 접속 정보 |
| `--user` | Main User로 사용할 섹션 이름 |

## 명령 분류

| 분류 | 설명 |
| --- | --- |
| S3 API | `--put-object`, `--get-object`, `--list-objects-v2` 등 단건 API 호출 |
| KSAN 확장 | `--put-bucket-tagindex`, `--get-bucket-tagindex`, `--delete-bucket-tagindex`, `--list-bucket-tagsearch`, `--admin` |
| 부하 테스트 | `--test-prepare`, `--test-put`, `--test-get`, `--test-delete`, `--test-mix`, `--test-all` 등 |
| 기능 테스트 | `--test-compare`, `--test-replication`, `--test-used-size`, `--test-access-ips`, `--test-duplicate` 등 |
| 로컬 파일시스템 | `--test-local-*` (S3 없이 로컬 I/O 성능 측정) |
| 다중 시스템 | `--test-multi-system-*` |
| 정리 | `--clear`, `--bucket-clear`, `--current-clear`, `--noncurrent-clear`, `--marker-clear` |
| 외부 연동 | `--test-mover`(ifsMover), `--pause` / `--resume`(S3backend 제어) |
| 분산 | `--worker`, `--controller` ([DISTRIBUTED.md](DISTRIBUTED.md)) |

## Claude Code 부하 테스트 스킬 (`/load-test`)

Claude Code에서 `/load-test`로 Linux 클라이언트의 TestRust 부하 테스트를 실행하고, 서버·클라이언트 자원 사용량을 모니터링한 뒤 결과를 수집한다. 스킬은 저장소의 `.claude/skills/load-test/`에 있으며 pull 받으면 바로 사용할 수 있다. 절차 세부는 [SKILL.md](.claude/skills/load-test/SKILL.md)를 참고한다.

```text
/load-test servers=p-ksan-node-1,p-ksan-node-2 clients=192.168.31.191 type=put size=1M thread=50 prefix=FILE bucket=bench times=300
/load-test config=tests/ksan-mix.conf thread=100
```

-   필수 입력: `servers`, `clients`(ssh 별칭·IP·user@host, 쉼표 구분), `type`(prepare/put/get/delete/mix 등), `size`, `thread`(클라이언트 1대당), `prefix`, `bucket`
-   종류별 필수: prepare·get은 `count`, put·get·mix는 `times`(초). mix 비율은 `read`/`write`/`delete`
-   설정 파일 형식과 기본값은 [run.conf.sample](.claude/skills/load-test/templates/run.conf.sample)을 참고한다. 명령행 값이 설정 파일보다 우선한다.
-   S3 접속 정보는 클라이언트의 `testrust_config`(기본 `/root/workspace/TestRust/config.ini`)에서 읽는다.

수집 항목

| 대상 | 항목 |
| --- | --- |
| 서버 | CPU used·iowait, load 1/5/15, 메모리 used, swap used |
| 클라이언트 | 위 항목 + TestRust 프로세스 CPU·RSS·스레드 수, TestRust 결과(`--save` JSON)와 로그 |

-   실행 중에는 iowait·메모리 사용률, 테스트 전 대비 swap 증가량, 로그 ERROR, 비정상 종료를 감지해 경보를 표시한다.
-   결과는 수집 호스트(기본: 첫 번째 클라이언트)의 `/root/testrust-results/<RunId>/`에 저장하고 `summary.md`·`summary.json`을 만든다. PC의 `.testruns/<RunId>/`에도 사본이 남는다.
-   스크립트는 직접 실행할 수도 있다: `bash .claude/skills/load-test/scripts/run.sh check|start|status|wait|collect|abort`

사전 조건

-   PC에서 모든 서버·클라이언트로 ssh 키 인증(비밀번호 없이, known_hosts 등록 완료) 접속이 가능해야 한다.
-   PC에 Git Bash(또는 bash)와 `python`이 필요하다.
-   클라이언트에 TestRust 실행 파일과 설정 파일이 있어야 한다(기본 `/root/workspace/TestRust/TestRust`, `/root/workspace/TestRust/config.ini`. `pwsh ./build-linux.ps1`의 기본 배포 경로와 같다). 경로가 다르면 `testrust_cmd`, `testrust_config`로 지정한다.
-   클라이언트가 2대 이상이면 객체 이름 충돌을 막기 위해 thread prefix 뒤에 `-<호스트>`가 붙는다. prepare와 get은 같은 클라이언트 목록으로 실행해야 한다.

## 전체 옵션

아래 목록은 `TestRust --help` 출력을 그대로 옮긴 것이다. 옵션을 추가·변경한 경우 다음 명령으로 다시 생성해 갱신한다. 옵션 정의는 `crates/cli/src/options/definitions.rs`에 있다.

```bash
TestRust --help
```

```
      --worker               분산 테스트 Worker HTTP 서버 실행
      --controller           설정된 Worker에서 테스트 실행 및 CSV/JSON 결과 수집
  -?, -h, --help             Usage.
  -v, --version              Show version information.
      --abort-multipart-upload
                             멀티파트업로드 중단 및 삭제
      --complete-multipart-upload
                             멀티파트 업로드 종료. 파일 합치기
      --copy-object          객체 복사
      --create-bucket        버킷 생성
      --create-multipart-upload
                             멀티파트 업로드 생성
      --delete-bucket        버킷 삭제
      --delete-bucket-analytics
                             버킷의 분석 설정 삭제
      --delete-bucket-cors   CORS 설정 삭제
      --delete-bucket-encryption
                             버킷의 암호화 설정 삭제
      --delete-bucket-inventory
                             버킷의 인벤토리 설정 삭제
      --delete-bucket-lifecycle
                             버킷의 수명주기 설정 삭제
      --delete-bucket-metrics
                             버킷의 메트릭 설정 삭제
      --delete-bucket-ownership-controls
                             버킷의 소유권 설정 삭제
      --delete-bucket-policy 버킷의 정책 삭제
      --delete-bucket-replication
                             버킷의 복제 설정 삭제
      --delete-bucket-tagging
                             버킷의 태그 삭제
      --delete-bucket-website
                             버킷의 웹사이트 설정 삭제
      --delete-object        객체 삭제
      --delete-object-tagging
                             객체의 태그 삭제
      --delete-objects       다수의 객체 삭제
      --delete-public-access-block
                             버킷의 Public Access Block 설정 삭제
      --get-bucket-acl       버킷의 권한 정보 조회
      --get-bucket-analytics 버킷의 분석 설정 조회
      --get-bucket-cors      버킷의 CORS 설정 조회
      --get-bucket-encryption
                             버킷의 암호화 설정 조회
      --get-bucket-inventory 버킷의 인벤토리 설정 조회
      --get-bucket-lifecycle 버킷의 수명주기 설정 조회
      --get-bucket-logging   버킷의 로그 설정 조회
      --get-bucket-metrics   버킷의 메트릭 설정 조회
      --get-bucket-location  버킷의 리전 정보 조회
      --get-bucket-notification
                             버킷의 알림 설정 조회
      --get-bucket-ownership-controls
                             버킷의 소유권 설정 조회
      --get-bucket-policy    버킷의 정책 조회
      --get-bucket-policy-status
                             버킷의 정책 상태 조회
      --get-bucket-replication
                             버킷의 복제 설정 조회
      --get-bucket-tagging   버킷의 태그 조회
      --get-bucket-versioning
                             버킷의 버전설정 조회
      --get-bucket-website   버킷의 웹사이트 설정 조회
      --get-object           객체 다운로드
      --get-object-acl       객체의 권한 설정 조회
      --get-object-legal-hold
                             객체의 보존(legal-hold) 설정 조회
      --get-object-lock      객체의 잠금(lock) 설정 조회
      --get-object-retention 객체의 보유(Retention) 설정 조회
      --get-object-tagging   객체의 태그 조회
      --get-public-access-block
                             객체의 Public Access Block 설정 조회
      --get-presigned-url    객체의 Presigned URL 생성(GET)
      --head-bucket          버킷의 정보 조회
      --head-object          객체의 정보 조회
      --restore-object       객체 복원
      --list-bucket-analytics
                             버킷의 분석 설정 목록 조회
      --list-bucket-inventory
                             버킷 인벤토리 목록 조회
      --list-bucket-metrics  버킷 메트릭 목록 조회
      --list-buckets         버킷 목록 조회
      --list-directory-buckets
                             버킷 목록 조회
      --list-multipart-uploads
                             멀티파트 업로드 목록 조회
      --list-object-versions 객체 버전 목록 조회
      --list-objects         객체 목록 조회
      --list-objects-v2      객체 목록 조회(V2)
      --list-parts           파츠 목록 조회
      --put-bucket-acl       버킷의 권한 설정
      --put-bucket-analytics 버킷의 분석 설정
      --put-bucket-cors      버킷의 CORS 설정
      --put-bucket-encryption
                             버킷의 기본 암호화 설정
      --put-bucket-inventory 버킷의 인벤토리 설정
      --put-bucket-lifecycle 버킷내의 객체 수명주기 설정
      --put-bucket-logging   버킷의 로그 설정
      --put-bucket-metrics   버킷의 메트릭 설정
      --put-bucket-notification
                             버킷의 알람 설정
      --put-bucket-ownership 버킷의 소유권 설정
      --put-bucket-policy    버킷의 정책 설정
      --put-bucket-replication
                             버킷의 복제기능 설정
      --put-bucket-tagging   버킷의 태그 설정
      --put-bucket-versioning
                             버킷의 버저닝 설정
      --put-bucket-website   버킷의 웹사이트 설정
      --put-object           객체 업로드
      --put-objects          폴더 업로드
      --put-object-acl       객체의 권한 설정
      --put-object-legal-hold
                             객체의 보존(legal-hold) 설정
      --put-object-lock      객체의 잠금(lock) 설정
      --put-object-retention 객체의 보유(Retention) 설정
      --put-object-tagging   객체의 태그 설정
      --put-public-access-block
                             객체의 Public Access Block 설정
      --upload-part          파츠 업로드
      --upload-part-copy     파츠 복제
      --storage-move         오브젝트의 스토리지 클래스 변경
      --set-object-lock      객체의 Legal Hold 설정(ON)
      --del-object-lock      객체의 Legal Hold 해제(OFF)
      --upload               AWS 상위레벨 API를 사용하여 업로드
      --download             AWS 상위레벨 API를 사용하여 다운로드
      --delete-bucket-tagindex
                             버킷의 태그 설정 삭제
      --get-bucket-tagindex  버킷의 태그 설정 조회
      --list-bucket-tagsearch
                             버킷에 존재하는 오브젝트의 태그 정보를 바탕으로 목록 조회
      --put-bucket-tagindex  버킷의 태그 설정 설정
      --pause=VALUE          Value=ServiceType. S3backend 서비스 일시정지
      --resume=VALUE         Value=ServiceType. S3backend 서비스 재개
      --use-chunk-encoding=VALUE
                             Value=true/false. Chunk Encoding 사용 여부
  -c, --config=VALUE         Value=설정파일 경로.
  -s, --save=VALUE           Value=결과 저장 경로.
      --admin                관리자 모드 활성화
  -b, --bucket=VALUE         Value=버킷 이름
      --access-key=VALUE     value=Access Key
      --secret-key=VALUE     value=Secret Key
      --source=VALUE         Value=원본 이름
      --target=VALUE         Value=대상 이름
  -k, --key=VALUE            Value=객체 이름
      --source-key=VALUE     Value=원본 객체 이름
  -f, --file=VALUE           Value=객체의 경로
  -p, --path=VALUE           Value=객체의 경로
      --upload-id=VALUE      Value=Upload Id.
      --part-size=VALUE      Value=Part Size(long type, ex> 50M)
      --start-byte=VALUE     Value=Start Point(long type, ex> 50M)
      --end-byte=VALUE       Value=end Point(long type, ex> 50M)
      --range-list=VALUE     Value=Range List. ex> 8K,2M...
      --version-id=VALUE     Value=Version Id.
      --storage-class=VALUE  Value=STANDARD(default)/GLACIER
      --versioning=VALUE     Value=Enabled/Suspended/Off.
      --part-number=VALUE    Value=int.
      --acl=VALUE            Value=/private/public-read/public-read-write/
                               authenticated-read/aws-exec-read/bucket-owner-
                               read/bucket-owner-full-control/log-delivery-write
      --prefix=VALUE         Value=Prefix.
      --suffix=VALUE         Value=Suffix.
      --delimiter=VALUE      Value=Delimiter.
      --marker=VALUE         Value=Marker.
      --continuation-token=VALUE
                             Value=ContinuationToken.
      --days=VALUE           Value=int.
      --years=VALUE          Value=int.
      --date=VALUE           Value=Date.
      --body=VALUE           Value=Content.
      --tag=VALUE            Value=Tag.
      --tag-set=VALUE        Value=Tags file.
      --id=VALUE             Value=Id.
      --max-keys=VALUE       Value=MaxKeys. 최대 목록 갯수.
      --print=VALUE          Value=true/false. 목록 조회옵션. 결과 출력 여부를 결정(default =
                               true)
      --url=VALUE            Value=URL. MainUser의 URL 값을 강제로 변경.
      --user=VALUE           Value=User Name. 해당 유저를 MainUser로 변경.
      --check                Check Mode
      --ownership=VALUE      Value=BucketOwnerEnforced/BucketOwnerPreferred/
                               ObjectWriter
      --lock-enable          버킷에 lock 모드 설정
      --lock-mode=VALUE      Value=LockMode. COMPLIANCE/GOVERNANCE. 버킷에 lock 모드
                               설정
      --encryption-key=VALUE Value=EncryptionKey.
      --set-sse-s3=VALUE     Value=true/false. 버킷에 sse-s3 설정 여부
      --manual-upload        create-multipart-upload + upload-part + complete-
                               multipart-upload
      --clear=VALUE          Value=true/false. 해당 유저의 모든 객체, True일 경우 버킷도 삭제.
      --start=VALUE          Value= Start Count. Prepare의 시작 번호
      --thread-prefix=VALUE   Value=스레드 접두어
      --thread=VALUE         Value= Thread Count.
      --count=VALUE          Value= File Count.
      --size=VALUE           Value=File Size. ex> 1k, 10M...
      --times=VALUE          Value=Run Times. 초 단위로 테스트 수행시간 지정.
      --flag=VALUE           Value=true/false.
      --bucket-type=VALUE    Value= int. Bucket Type. [설정없음 = 0, 단일버킷업로드, 스레드당
                               버킷, 날짜당 폴더, Prefix, 현재시각 기준 생성]
      --service-type=VALUE   Value=ServiceType. Replication, Lifecycle, Logging
      --address=VALUE        Value=Ip
      --port=VALUE           Value=Port
      --all                  목록을 불러올때 모든 목록을 불러오도록 설정
      --bulk                 1000개씩 묶어서 삭제할지 여부(--test-delete, --test-delete-
                               version option)
      --bypass               강제 삭제 모드
      --quiet                삭제 실패한 목록만 전달 받는 모드
      --multipart            테스트를 멀티파트로 업로드
      --not-empty            업로드할 오브젝트의 내용물을 매번 생성하도록 지정.
      --debug                디버그용 로그 출력.
      --checksum             체크섬 모드 활성화
      --checksum-type=VALUE  체크섬 타입: CRC32, CRC32C, CRC64NVME, SHA1, SHA256,
                               None
      --md5sum               MD5 체크섬 계산 및 전송
  -r, --read=VALUE           Read=value. 읽기 비율
  -w, --write=VALUE          Write=value. 쓰기 비율
  -d, --delete=VALUE         Delete=value. 삭제 비율
      --bucket-clear=VALUE    Value=버킷 이름. 버킷에 존재하는 모든 객체를 삭제. 버킷 보존.
      --current-clear=VALUE   Value=버킷 이름. 버킷에 존재하는 모든 현재 버전 객체를 삭제. 버킷 보존.
      --noncurrent-clear=VALUE
                              Value=버킷 이름. 버킷에 존재하는 모든 이전 버전 객체를 삭제. 버킷 보존.
      --marker-clear=VALUE    Value=버킷 이름. 버킷에 존재하는 모든 Delete Marker 삭제. 버킷 보존.
      --test-access-ips      ip 접근제어 테스트
      --test-replication     s3 복제 테스트
      --test-list-object     ListObject 테스트
      --test-range-copy      원본 객체를 Range로 읽어 다운로드 한뒤 멀티파트 업로드
      --test-upload          AWS 상위레벨 API를 사용하여 업로드 테스트
      --test-download        AWS 상위레벨 API를 사용하여 다운로드 테스트
      --test-prepare         일정 갯수만큼 업로드. 이미 파일이 업로드 되어있다면 업로드 하지않음.
      --test-prepare-dir     일정 갯수만큼 폴더(디렉터리 마커) 생성. 이미 존재하면 생성하지 않음.
      --test-new-put         Prepare와 동일. 매 PutObject마다 새 S3Client 생성.
      --test-put             PutObject 테스트
      --test-head            Head 테스트
      --test-get             GetObject 테스트. 테스트 전에 Prepare, put 등으로 테스트 환경을
                               구성하고 진행하지 않으면 실패.
      --test-get-v2          Sequential GetObject 테스트. 테스트 전에 Prepare, put 등으로
                               테스트 환경을 구성하고 진행하지 않으면 실패.
      --test-new-get         test-get-v2와 동일. 매 GetObject마다 새 S3Client 생성.
      --test-get-v3          ListObject => GetObject 테스트. 테스트 전에 Prepare, put
                               등으로 테스트 환경을 구성하고 진행하지 않으면 실패.
      --test-get-all         Sequential GetObject 테스트. 테스트 전에 Prepare, put 등으로
                               테스트 환경을 구성하고 진행하지 않으면 실패.
      --test-delete          DeleteObject 테스트
      --test-delete-v2       Prepare로 업로드한 오브젝트를 순차적으로 삭제하고 모두 삭제하면 종료하는 테스트.
                               테스트 전에 Prepare, put 등으로 테스트 환경을 구성하고 진행하지 않으면 실패.
      --test-new-del         test-delete-v2와 동일. 매 DeleteObject마다 새 S3Client 생성.
      --test-delete-version  DeleteObjectVersion 테스트
      --test-delete-directory
                             ListObject를 이용하여 디렉토리를 삭제하는 테스트
      --test-mix             PutObject, GetObject를 설정한 비율로 테스트 설정한 시간만큼 동작.
                               GetObject는 업로드한 파일 목록 내에서 무작위로 실행.
      --test-mix-v2          Read, Write를 설정한 갯수만큼 스레드를 생성하여 테스트. Prepare로 테스트
                               환경을 구성하고 진행해야 함. GetObject는 업로드한 파일 목록 내에서 무작위로
                               실행.
      --test-new-mix         test-all과 동일. 매 Put/Get/Delete마다 새 S3Client 생성.
      --test-put-get         PutObject, GetObject를 순차로 테스트.
      --test-all             PutObject, GetObject, DeleteObject를 순차로 실행하는 테스트.
      --test-full            모든 테스트를 순차로 실행하는 테스트.
      --test-io              특정 폴더의 내용을 모두 업로드한뒤 다운로드하여 md5sum으로 비교하는 테스트.
      --test-mover           ifsMover용 테스트.
      --test-put-tag         PutObject시 Tag를 포함하여 업로드
      --test-compare=VALUE   Value=true(Alt User)/false(Main User Only). 원본과 대상
                               버킷의 모든 객체가 동일한지 비교.(default = true)
      --test-find-tag=VALUE  test-put-tag에서 업로드한 Tag를 조회하는 테스트.
      --test-multi-delete    단일 버킷, DeleteObject만 하는 테스트
      --test-used-size       버킷의 UsedSize가 일치하는지 확인하는 테스트
      --test-duplicate       중복 업로드 테스트
      --test-multi-upload    동시 업로드 테스트
      --test-directory-download
                             폴더 다운로드 테스트
      --test-file-list=VALUE value=File List Path. 파일목록을 읽어와서 멀티 다운로드 테스트
      --test-range-read      단일 오브젝트를 Range 단위로 순차 읽기
      --test-multipart-upload
                             멀티파트업로드 테스트
      --test-multipart-upload-v2
                             멀티파트업로드 및 다운로드 테스트
      --test-multi-system-list
                             다중 시스템 업로드 테스트
      --test-multi-system-upload
                             다중 시스템 업로드 테스트
      --test-multi-system-up-down
                             다중 시스템 업로드, 다운로드 테스트
      --test-multi-system-all
                             다중 시스템 업로드, 다운로드, 삭제 테스트
      --test-compare-lifecycle
                             만료기한 확인
      --test-aws             AWS Multi test
      --target-path=VALUE    로컬 테스트 타겟 경로
      --test-local-prepare   로컬 파일시스템 Prepare 테스트
      --test-local-put       로컬 파일시스템 PutOnly 테스트
      --test-local-get       로컬 파일시스템 랜덤 GetOnly 테스트
      --test-local-get-v2    로컬 파일시스템 순차 GetOnly 테스트
      --test-local-put-get   로컬 파일시스템 PutGet 테스트
      --test-local-delete    로컬 파일시스템 Delete 테스트
      --test-local-multipart-prepare
                             로컬 파일시스템 멀티파트 Prepare 테스트
      --test-local-multipart-put
                             로컬 파일시스템 멀티파트 PutOnly 테스트
      --test-local-multipart-get
                             로컬 파일시스템 멀티파트 랜덤 GetOnly 테스트
      --test-local-multipart-get-v2
                             로컬 파일시스템 멀티파트 순차 GetOnly 테스트
      --test-local-multipart-put-get
                             로컬 파일시스템 멀티파트 PutGet 테스트
```

## TESTCore와의 관계

TestRust는 TESTCore(.NET)와 같은 계약을 지키도록 만들었다. 실행 파일 이름(`TestRust`)만 다르고, 아래는 같다.

-   CLI 옵션 이름과 `config.ini` 형식
-   콘솔 출력, CSV·JSON 결과 파일
-   Controller·Worker HTTP 계약
-   통계 측정 구간, 체크섬·서명 결과

두 구현의 출력은 `tests/parity/`의 비교 테스트로 확인한다([tests/parity/README.md](tests/parity/README.md)). 모듈별 설계와 원본 동작 규칙은 다음 문서에 있다.

-   [CLI·디스패처](docs/design/cli-dispatch.md)
-   [S3 클라이언트](docs/design/s3-client.md)
-   [테스트 시나리오](docs/design/scenarios.md)
-   [분산 실행](docs/design/distributed.md)

### TESTCore에서 함께 고친 원본 버그

원본에 있던 버그는 사용자 결정으로 TESTCore와 이식본을 함께 고쳤다. 기준 출력은 이 커밋으로 만든 빌드에서 만든다.

-   `c83e35f`: 시나리오 테스트의 목록 marker, 다운로드 경로, 접근 IP 해제, 다중 시스템 목록 검증(CompareTest·FindTagTest, MultiDownloadTest, AccessIpsTest, MultiSystemTest `ListCore`)
-   `3c4b0ea`: RangeReadCopy 파트 업로드 실패(크기가 0보다 큰 오브젝트가 항상 복사에 실패하던 문제)
-   `ec427f2`: 분산 실행의 결과 저장 순서(`WorkerJob` 마무리), `--not-empty` 무작위 업로드 파일, 결과 파일 생성 실패 시 정리

### 알려진 차이

-   CompareTest 메타데이터 비교: .NET은 서버가 보낸 `x-amz-meta-*` 헤더 이름의 대소문자를 그대로 두지만 Rust(hyper)는 소문자로 바꾼다. 대문자가 섞인 메타데이터 헤더는 로그의 이름과 비교 순서가 다를 수 있다(S3는 소문자로 보낸다).
-   분산 CSV: 실수의 `1E+15` 이상 지수 표기는 다루지 않는다(통계 값이 그만큼 커지지 않는다).
-   분산 Worker: 닫힌 S3 포트로 제출 직후 중단하면, 시작 속도에 따라 .NET은 Cancelled, Rust는 Failed로 끝날 수 있다. 두 경로 모두 원본 코드에 있다.

## 알려진 이슈 (원본 동작을 그대로 옮긴 것)

-   `[Jenkins]` 섹션의 DB 포트는 현재 `Port` 키로 읽히고 `JenkinsPort` 키가 Jenkins 포트가 아닌 DB 포트로 전달된다(원본 `ReadJenkinsConfig`, Rust는 `crates/config/src/config.rs`의 `jenkins_config`). Jenkins 연동 사용 시 값 확인이 필요하다.
-   `sample.ini`의 `[Third User]` 섹션은 현재 코드에서 읽지 않는다.

## 참고 문서

-   [DISTRIBUTED.md](DISTRIBUTED.md): Controller·Worker 분산 부하 테스트
-   [docs/operations.md](docs/operations.md): 빌드·배포, TESTCore에서의 전환, 성능 비교, 롤백
-   [docs/perf/](docs/perf/2026-10-08-ksan.md): TESTCore와의 성능 비교 결과(사내 KSAN)
-   [RUST_MIGRATION.md](RUST_MIGRATION.md): 이식 당시의 전환 계획과 단계(기록)
