---
name: load-test
description: TestRust로 S3(KSAN 등) 부하 테스트(put/get/delete/mix/prepare 등)를 Linux 클라이언트에서 실행하고, 서버 CPU·메모리와 클라이언트 TestRust 사용량을 모니터링한 뒤 결과를 클라이언트에 수집·요약한다.
argument-hint: "[config=<파일>] servers=<h1,h2> clients=<h1,h2> type=<put|get|delete|mix|prepare> size=<1M> thread=<N> prefix=<P> bucket=<B> [count=<N>] [times=<초>]"
disable-model-invocation: true
allowed-tools: Bash(bash .claude/skills/load-test/scripts/run.sh:*), Bash(python .claude/skills/load-test/scripts/summarize.py:*)
---

# load-test: 부하 테스트 실행 + 모니터링 + 결과 수집

모든 원격 작업은 `scripts/run.sh`가 PC에서 ssh(키 인증, BatchMode)로 수행한다. 원격 명령을 직접 조합하지 말고 이 스크립트를 사용한다. 저장소 루트에서 실행한다.

```
RUN=.claude/skills/load-test/scripts/run.sh
bash $RUN check   [config=FILE] key=value ...
bash $RUN start   [config=FILE] key=value ...   → RUN_ID 출력
bash $RUN status  <RUN_ID>
bash $RUN wait    <RUN_ID>                      → 경보(ALERT)·완료(DONE)·주기 요약 출력, 끝나면 종료
bash $RUN collect <RUN_ID> [--force]            → SUMMARY=<경로>
bash $RUN abort   <RUN_ID>
```

`config=FILE`은 `--config FILE`로 바꿔서 전달한다. 설정 형식과 기본값은 `templates/run.conf.sample`을 참고한다.

## 입력: $ARGUMENTS

| 키 | 설명 | 필수 |
|---|---|---|
| servers | 모니터링할 서버 목록 (ssh 별칭, IP, user@host. 여러 개는 쉼표로 구분) | O |
| clients | TestRust를 실행할 클라이언트 목록 | O |
| type | prepare, put, get, delete, mix, all 등 (`--test-<type>`) | O |
| size | 파일 크기 (4K, 1M, 1G) | O |
| thread | 클라이언트 1대당 스레드 수 | O |
| prefix | ObjectPrefix | O |
| bucket | 버킷 이름 | O |
| count | 스레드당 파일 수 | prepare, get |
| times | 실행 시간(초) | put, get, mix |
| read, write, delete | mix 비율 | 선택 |
| extra | 그 밖에 TestRust에 그대로 넘길 옵션 | 선택 |

인자를 해석하는 방법:
- `put 1M 50스레드 300초`처럼 자연어로 들어와도 위 키로 변환한다.
- 필수 값이 빠졌으면 실행하기 전에 AskUserQuestion으로 **한 번에 모아서** 묻는다. 서버와 클라이언트 주소는 추측하지 않는다.
- S3 접속 정보는 클라이언트의 `testrust_config`(기본 `/root/workspace/TestRust/config.ini`)에서 읽는다. 자격 증명을 인자로 받거나 출력하지 않는다.

## 절차

1. **점검**: `check`를 실행한다. `CHECK: FAIL`이면 원인(ssh 실패, TestRust 실행 파일 또는 설정 파일 없음)을 보고하고 중단한다. 출력의 "예시 명령"을 사용자에게 한 줄로 보여 준다.
2. **시작**: 같은 인자로 `start`를 실행하고 `RUN_ID`를 기록한다. 샘플러가 먼저 시작해 테스트 전 기준값을 남긴 뒤 클라이언트에서 TestRust가 동시에 실행된다.
3. **감시**: `wait <RUN_ID>`를 Monitor 도구로 실행해 출력을 스트리밍한다. Monitor를 쓸 수 없으면 Bash `run_in_background`로 실행한다. 이 명령은 포그라운드로 실행하지 않는다. 테스트가 10분 제한을 넘길 수 있기 때문이다.
   - `ALERT` 줄이 나오면 사용자에게 바로 짧게 알린다. 예: iowait나 메모리 기준 초과, 비정상 종료, ssh 끊김.
   - 테스트를 중단할지는 사용자가 결정한다. 사용자가 요청할 때만 `abort`를 실행한다. 클라이언트가 모두 비정상 종료한 경우는 예외로, 바로 수집 단계로 넘어간다.
   - 사용자가 진행 상황을 물으면 `status`로 답한다.
4. **수집**: `wait`가 끝나면 `collect <RUN_ID>`를 실행한다. 샘플러를 중지하고, 모든 호스트의 파일을 PC의 `.testruns/<RUN_ID>/`로 가져와 요약한 뒤, 수집 호스트(기본: 첫 번째 클라이언트)의 `result_dir/<RUN_ID>/`에 저장한다.
5. **보고**: `summary.md`를 읽고 아래 내용을 한국어로 간결하게 보고한다.
   - 전체 처리량(ops/s, MiB/s)과 실패 수
   - 서버별 CPU used/wait, load 1/5/15, 메모리, swap (테스트 구간 평균/최대, 기준값과 비교)
   - 클라이언트 TestRust CPU, RSS, 스레드 수
   - 눈에 띄는 점. 예: 실패가 0이 아님, 클라이언트 CPU 포화로 클라이언트가 병목, 서버 iowait가 높음, swap 발생, 클라이언트 간 처리량 편차
   - 결과 저장 위치 (`collect_host:result_dir/RUN_ID`)
6. Outline 정리, DB 적재 등 후속 작업은 사용자가 요청할 때만 한다.

## 결과 디렉터리 구조

```
<RUN_ID>/
  run.conf, commands.txt        실행 설정과 클라이언트별 실제 명령
  summary.md, summary.json      요약
  hosts/<호스트>/
    metrics-server.csv          서버: cpu_used/wait, load1/5/15, mem/swap used
    metrics-client.csv          클라이언트: 위 항목 + TestRust proc_cpu/rss/threads
    testrust.log, result.json   클라이언트: TestRust 로그와 --save 결과
    started_at, finished_at, exitcode
```

## 주의

- 클라이언트가 2대 이상이면 기본값(`client_prefix=auto`)으로 thread_prefix 뒤에 `-<호스트>`를 붙인다. 그래서 같은 클라이언트 목록으로 prepare를 실행한 뒤 get을 실행해야 같은 객체를 읽는다.
- 처리량 합산은 클라이언트별 `성공 수 / 실행 시간`을 더한 값이다. 클라이언트 간 시작 시각 차이(수 초)는 보정하지 않는다.
- 같은 호스트가 서버와 클라이언트 양쪽에 있으면 두 지표 파일이 모두 생성된다.
