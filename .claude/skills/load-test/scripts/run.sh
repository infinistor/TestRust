#!/bin/bash
# TestRust 부하 테스트 실행·모니터링·결과 수집 오케스트레이터 (PC의 Git Bash에서 실행)
#
# 사용법:
#   run.sh check   [--config FILE] [key=value ...]   접속·실행 환경 점검
#   run.sh start   [--config FILE] [key=value ...]   샘플러와 테스트 시작, RunId 출력
#   run.sh status  [RunId|latest]                    현재 상태 1회 출력
#   run.sh wait    [RunId|latest]                    종료까지 감시하며 경보·주기 요약 출력
#   run.sh collect [RunId|latest] [--force]          샘플러 중지, 결과 회수·요약, 수집 호스트에 저장
#   run.sh abort   [RunId|latest]                    클라이언트 테스트 중단(SIGINT → SIGTERM)
#
# 옵션은 key=value, --key=value, --key value 형식을 모두 받는다. 명령행 값이 설정 파일보다 우선한다.

set -u
export MSYS_NO_PATHCONV=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RUNS_DIR="${TESTRUNS_DIR:-$(pwd)/.testruns}"
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=15 -o ServerAliveCountMax=3)

KEYS="servers clients type size thread prefix thread_prefix client_prefix bucket count times read write delete
testrust_cmd testrust_config testrust_workdir extra interval remote_dir result_dir collect_host keep_remote
poll heartbeat alert_iowait alert_mem alert_swap_mb"

declare -A C=(
	[client_prefix]=auto
	[thread_prefix]=TH
	[testrust_cmd]="/root/workspace/TestRust/TestRust"
	[testrust_config]=/root/workspace/TestRust/config.ini
	[interval]=2
	[remote_dir]=/tmp/testrust-skill
	[result_dir]=/root/testrust-results
	[keep_remote]=0
	[poll]=15
	[heartbeat]=60
	[alert_iowait]=30
	[alert_mem]=90
	[alert_swap_mb]=512
)

die() { echo "ERROR: $*" >&2; exit 1; }
now() { date +%H:%M:%S; }
rssh() { local h="$1"; shift; ssh "${SSH_OPTS[@]}" "$h" "$@"; }
host_dir() { echo "${1//[^A-Za-z0-9._-]/_}"; }
trim() { local s="$1"; s="${s#"${s%%[![:space:]]*}"}"; echo "${s%"${s##*[![:space:]]}"}"; }

set_kv() {
	local k="${1//-/_}" v="$2"
	[[ " $(echo $KEYS) " == *" $k "* ]] || die "알 수 없는 옵션: $1 (허용: $(echo $KEYS))"
	C[$k]="$v"
}

load_conf() {
	[ -f "$1" ] || die "설정 파일이 없습니다: $1"
	local line k v
	while IFS= read -r line || [ -n "$line" ]; do
		line="${line%$'\r'}"
		[[ "$line" =~ ^[[:space:]]*(#|;|$) ]] && continue
		[[ "$line" =~ ^[[:space:]]*\[ ]] && continue
		[[ "$line" == *=* ]] || continue
		k="$(trim "${line%%=*}")"; v="$(trim "${line#*=}")"
		set_kv "$k" "$v"
	done < "$1"
}

parse_args() {
	local -a rest=()
	while [ $# -gt 0 ]; do
		case "$1" in
			--config|-c) load_conf "$2"; shift 2 ;;
			--config=*) load_conf "${1#*=}"; shift ;;
			*) rest+=("$1"); shift ;;
		esac
	done
	set -- "${rest[@]+"${rest[@]}"}"
	while [ $# -gt 0 ]; do
		case "$1" in
			--*=*) local a="${1#--}"; set_kv "${a%%=*}" "${a#*=}"; shift ;;
			--*) [ $# -ge 2 ] || die "$1 값이 없습니다"; set_kv "${1#--}" "$2"; shift 2 ;;
			*=*) set_kv "${1%%=*}" "${1#*=}"; shift ;;
			*) die "해석할 수 없는 인자: $1" ;;
		esac
	done
	C[type]="${C[type]:-}"; C[type]="${C[type]#--test-}"
	C[testrust_workdir]="${C[testrust_workdir]:-$(dirname "${C[testrust_config]}")}"
}

split_hosts() { echo "${1//,/ }"; }

validate() {
	local missing=()
	for k in servers clients type size thread prefix bucket; do [ -n "${C[$k]:-}" ] || missing+=("$k"); done
	case "${C[type]:-}" in
		prepare) [ -n "${C[count]:-}" ] || missing+=("count") ;;
		get) [ -n "${C[count]:-}" ] || missing+=("count"); [ -n "${C[times]:-}" ] || missing+=("times") ;;
		put|mix) [ -n "${C[times]:-}" ] || missing+=("times") ;;
	esac
	[ ${#missing[@]} -eq 0 ] || die "필수 값 누락: ${missing[*]}"
	[[ "${C[thread]}" =~ ^[0-9]+$ ]] || die "thread는 정수여야 합니다: ${C[thread]}"
	[[ "${C[size]}" =~ ^[0-9]+[KkMmGg]?[Bb]?$ ]] || die "size 형식 오류(예: 4K, 1M, 1G): ${C[size]}"
	for k in count times read write delete; do
		[ -z "${C[$k]:-}" ] || [[ "${C[$k]}" =~ ^[0-9]+$ ]] || die "$k는 정수여야 합니다: ${C[$k]}"
	done
	C[collect_host]="${C[collect_host]:-$(split_hosts "${C[clients]}" | awk '{print $1}')}"
}

all_hosts() { { for h in $(split_hosts "${C[servers]}") $(split_hosts "${C[clients]}"); do echo "$h"; done; } | awk '!seen[$0]++'; }
is_client() { [[ " $(split_hosts "${C[clients]}") " == *" $1 "* ]]; }
is_server() { [[ " $(split_hosts "${C[servers]}") " == *" $1 "* ]]; }

# ---------- RunId 상태 ----------
resolve_run() {
	local id="${1:-latest}"
	if [ "$id" = latest ]; then
		id="$(ls -1 "$RUNS_DIR" 2>/dev/null | grep -v '^\.' | sort | tail -n1)"
		[ -n "$id" ] || die "실행 기록이 없습니다: $RUNS_DIR"
	fi
	RUN_ID="$id"; LOCAL_DIR="$RUNS_DIR/$RUN_ID"
	[ -f "$LOCAL_DIR/run.conf" ] || die "RunId를 찾을 수 없습니다: $RUN_ID"
	load_conf "$LOCAL_DIR/run.conf"
	RDIR="${C[remote_dir]}/$RUN_ID"
}

save_run_conf() {
	{
		echo "# RunId: $RUN_ID"
		for k in $KEYS; do [ -n "${C[$k]:-}" ] && echo "$k = ${C[$k]}"; done
	} > "$LOCAL_DIR/run.conf"
}

build_cmd() {
	local host="$1" idx="$2" tp="${C[thread_prefix]}"
	local nclients; nclients=$(split_hosts "${C[clients]}" | wc -w)
	if [ "${C[client_prefix]}" = auto ] && [ "$nclients" -gt 1 ]; then tp="${tp}-$(host_dir "$host")"; fi
	local cmd="${C[testrust_cmd]} --test-${C[type]} -c $(printf %q "${C[testrust_config]}")"
	cmd+=" --size ${C[size]} --thread ${C[thread]} -b $(printf %q "${C[bucket]}")"
	cmd+=" --prefix $(printf %q "${C[prefix]}") --thread-prefix $(printf %q "$tp")"
	[ -n "${C[count]:-}" ] && cmd+=" --count ${C[count]}"
	[ -n "${C[times]:-}" ] && cmd+=" --times ${C[times]}"
	[ -n "${C[read]:-}" ] && cmd+=" -r ${C[read]}"
	[ -n "${C[write]:-}" ] && cmd+=" -w ${C[write]}"
	[ -n "${C[delete]:-}" ] && cmd+=" -d ${C[delete]}"
	cmd+=" --save $RDIR/result.json"
	[ -n "${C[extra]:-}" ] && cmd+=" ${C[extra]}"
	echo "$cmd"
}

# ---------- check ----------
cmd_check() {
	parse_args "$@"; validate
	local ok=1 h out
	for h in $(all_hosts); do
		local role; role="$( { is_server "$h" && echo server; is_client "$h" && echo client; } | paste -sd+ )"
		if ! out="$(rssh "$h" "hostname; nproc; free -m | awk '/^Mem:/{print \$2}'; df -Pm ${C[remote_dir]%/*} | awk 'NR==2{print \$4}'" 2>&1)"; then
			echo "[FAIL] $h ($role): ssh 실패 - $(echo "$out" | tail -n1)"; ok=0; continue
		fi
		mapfile -t f <<< "$out"
		echo "[ OK ] $h ($role): hostname=${f[0]} cpu=${f[1]} mem=${f[2]}MB free(${C[remote_dir]%/*})=${f[3]}MB"
		if is_client "$h"; then
			local exe; exe="$(echo "${C[testrust_cmd]}" | awk '{print $1}')"
			out="$(rssh "$h" "command -v $exe >/dev/null && echo exe=ok || echo exe=missing;
				[ -f '${C[testrust_config]}' ] && echo config=ok || echo config=missing;
				[ -d '${C[testrust_workdir]}' ] && echo workdir=ok || echo workdir=missing" 2>&1)"
			echo "       testrust: $(echo $out)"
			[[ "$out" == *missing* ]] && ok=0
		fi
	done
	echo "수집 호스트: ${C[collect_host]}:${C[result_dir]}"
	echo "예시 명령(첫 클라이언트): $(RDIR="${C[remote_dir]}/<RunId>"; build_cmd "$(split_hosts "${C[clients]}" | awk '{print $1}')" 0)"
	[ $ok -eq 1 ] && echo "CHECK: OK" || { echo "CHECK: FAIL"; exit 1; }
}

# ---------- start ----------
cmd_start() {
	parse_args "$@"; validate
	RUN_ID="$(date +%Y%m%d-%H%M%S)-${C[type]}-${C[size]}"
	LOCAL_DIR="$RUNS_DIR/$RUN_ID"; RDIR="${C[remote_dir]}/$RUN_ID"
	mkdir -p "$LOCAL_DIR"; save_run_conf

	local max=21600
	[ -n "${C[times]:-}" ] && max=$(( C[times] + 3600 ))
	local h
	for h in $(all_hosts); do
		rssh "$h" "mkdir -p $RDIR && cat > $RDIR/sampler.sh && chmod +x $RDIR/sampler.sh" < "$SCRIPT_DIR/remote/sampler.sh" \
			|| die "$h: 원격 디렉터리 준비 실패"
		if is_server "$h"; then
			rssh "$h" "setsid nohup bash $RDIR/sampler.sh $RDIR/metrics-server.csv ${C[interval]} $max >/dev/null 2>&1 </dev/null &"
		fi
		if is_client "$h"; then
			rssh "$h" "setsid nohup bash $RDIR/sampler.sh $RDIR/metrics-client.csv ${C[interval]} $max $RDIR/testrust.pid >/dev/null 2>&1 </dev/null &"
		fi
	done
	echo "[$(now)] 샘플러 시작: $(all_hosts | tr '\n' ' ')"

	# 테스트 전 기준값(baseline) 확보
	sleep $(( C[interval] * 3 ))

	local idx=0 pids=()
	for h in $(split_hosts "${C[clients]}"); do
		local cmd; cmd="$(build_cmd "$h" $idx)"
		echo "$h: $cmd" >> "$LOCAL_DIR/commands.txt"
		(
			cat <<EOF | rssh "$h" "cat > $RDIR/launch.sh"
#!/bin/bash
cd $(printf %q "${C[testrust_workdir]}") || exit 1
date -u +%Y-%m-%dT%H:%M:%SZ > $RDIR/started_at
$cmd </dev/null > $RDIR/testrust.log 2>&1 &
echo \$! > $RDIR/testrust.pid
wait \$!
echo \$? > $RDIR/exitcode
date -u +%Y-%m-%dT%H:%M:%SZ > $RDIR/finished_at
EOF
			rssh "$h" "setsid nohup bash $RDIR/launch.sh >/dev/null 2>&1 </dev/null &"
		) &
		pids+=($!); idx=$((idx + 1))
	done
	for p in "${pids[@]}"; do wait "$p"; done

	echo "[$(now)] 테스트 시작: type=${C[type]} size=${C[size]} thread=${C[thread]} count=${C[count]:--} times=${C[times]:--} clients=${C[clients]}"
	echo "RUN_ID=$RUN_ID"
	echo "LOCAL_DIR=$LOCAL_DIR"
}

# ---------- 상태 조회 ----------
# 호스트별: 첫 샘플(기준값), 마지막 서버/클라이언트 지표, 테스트 상태
probe() {
	local h="$1"
	rssh "$h" "cd $RDIR 2>/dev/null || exit 0
		[ -f metrics-server.csv ] && echo \"BS|\$(sed -n 2p metrics-server.csv)\"
		[ -f metrics-client.csv ] && echo \"BC|\$(sed -n 2p metrics-client.csv)\"
		[ -f metrics-server.csv ] && echo \"S|\$(tail -n1 metrics-server.csv)\"
		[ -f metrics-client.csv ] && echo \"C|\$(tail -n1 metrics-client.csv)\"
		if [ -f launch.sh ]; then
			alive=0; [ -f testrust.pid ] && kill -0 \$(cat testrust.pid) 2>/dev/null && alive=1
			echo \"T|\$alive|\$(cat exitcode 2>/dev/null)|\$(grep -a -c -E '(^| )ERROR ' testrust.log 2>/dev/null)|\$(grep -a -v -E '^[[:space:]-]*\$' testrust.log 2>/dev/null | tail -n1 | tr -d '\r' | cut -c1-200)\"
		fi" 2>/dev/null
}

fmt_metrics() {
	# ts,epoch,cpu,wait,l1,l5,l15,mtot,mused,mpct,stot,sused,alive,pcpu,prss,pthr
	echo "$1" | awk -F, '{
		s = sprintf("cpu %s%% wait %s%% load %s/%s/%s mem %s%% swap %sMB", $3, $4, $5, $6, $7, $10, $12)
		if ($13 == 1) s = s sprintf(" | TestRust cpu %s%% rss %sMB thr %s", $14, $15, $16)
		print s }'
}

cmd_status() {
	resolve_run "${1:-latest}"
	echo "RunId: $RUN_ID  (type=${C[type]} size=${C[size]} thread=${C[thread]} count=${C[count]:--} times=${C[times]:--})"
	local h line
	for h in $(all_hosts); do
		local out; out="$(probe "$h")"
		[ -n "$out" ] || { echo "  $h: 응답 없음"; continue; }
		while IFS= read -r line; do
			case "$line" in
				B?\|*) ;;
				*\|ts_utc*) echo "  $h: 첫 샘플 대기 중" ;;
				S\|*) echo "  [server] $h: $(fmt_metrics "${line#S|}")" ;;
				C\|*) echo "  [client] $h: $(fmt_metrics "${line#C|}")" ;;
				T\|*) IFS='|' read -r _ alive code errs last <<< "$line"
					if [ "$alive" = 1 ]; then echo "  [test  ] $h: 실행 중, 로그 ERROR ${errs:-0}건 | $last"
					else echo "  [test  ] $h: 종료(exit=${code:-?}), 로그 ERROR ${errs:-0}건 | $last"; fi ;;
			esac
		done <<< "$out"
	done
}

# ---------- wait ----------
cmd_wait() {
	resolve_run "${1:-latest}"
	local deadline=$(( $(date +%s) + ${C[times]:-21600} + 1800 ))
	local last_hb=0
	declare -A alerted=() swap_base=()
	echo "[$(now)] 감시 시작: $RUN_ID (poll=${C[poll]}s, heartbeat=${C[heartbeat]}s)"
	while :; do
		local running=0 hb_lines=() h line
		for h in $(all_hosts); do
			local out; out="$(probe "$h")"
			if [ -z "$out" ]; then
				[ -z "${alerted[$h:ssh]:-}" ] && echo "[$(now)] ALERT $h: 응답 없음(ssh 실패)" && alerted[$h:ssh]=1
				is_client "$h" && running=$((running + 1))
				continue
			fi
			unset "alerted[$h:ssh]"
			while IFS= read -r line; do
				case "$line" in
					BS\|?*|BC\|?*)
						# swap 경보는 테스트 전 첫 샘플 대비 증가량으로 판단한다
						local brole=server; [[ "$line" == BC* ]] && brole=client
						[ -n "${swap_base[$h:$brole]:-}" ] || swap_base[$h:$brole]="$(echo "${line#B?|}" | cut -d, -f12)" ;;
					S\|*|C\|*)
						local role=server; [[ "$line" == C* ]] && role=client
						local m="${line#?|}"
						[[ "$m" == ts_utc* ]] && continue
						hb_lines+=("  [$role] $h: $(fmt_metrics "$m")")
						IFS=, read -r _ _ _ iow _ _ _ _ _ mpct _ sused _ <<< "$m"
						if awk -v a="${iow:-0}" -v b="${C[alert_iowait]}" 'BEGIN{exit !(a>=b)}'; then
							[ -z "${alerted[$h:$role:wait]:-}" ] && echo "[$(now)] ALERT $role $h: iowait ${iow}% >= ${C[alert_iowait]}%" && alerted[$h:$role:wait]=1
						else unset "alerted[$h:$role:wait]"; fi
						if awk -v a="${mpct:-0}" -v b="${C[alert_mem]}" 'BEGIN{exit !(a>=b)}'; then
							[ -z "${alerted[$h:$role:mem]:-}" ] && echo "[$(now)] ALERT $role $h: 메모리 ${mpct}% >= ${C[alert_mem]}%" && alerted[$h:$role:mem]=1
						else unset "alerted[$h:$role:mem]"; fi
						local sbase="${swap_base[$h:$role]:-${sused:-0}}"
						if [ $(( ${sused:-0} - sbase )) -ge "${C[alert_swap_mb]}" ] 2>/dev/null; then
							[ -z "${alerted[$h:$role:swap]:-}" ] && echo "[$(now)] ALERT $role $h: swap ${sbase}MB → ${sused}MB (+$(( sused - sbase ))MB >= ${C[alert_swap_mb]}MB)" && alerted[$h:$role:swap]=1
						fi ;;
					T\|*)
						IFS='|' read -r _ alive code errs last <<< "$line"
						if [ "${errs:-0}" -gt 0 ] && [ -z "${alerted[$h:err]:-}" ]; then
							echo "[$(now)] ALERT $h: TestRust 로그에 ERROR 발생 | $last"; alerted[$h:err]=1
						fi
						if [ "$alive" = 1 ] || [ -z "$code" ]; then
							running=$((running + 1)); hb_lines+=("  [test  ] $h: 로그 ERROR ${errs:-0}건 | $last")
						elif [ -z "${alerted[$h:done]:-}" ]; then
							if [ "$code" = 0 ]; then echo "[$(now)] DONE $h: 테스트 종료(exit=0, 로그 ERROR ${errs:-0}건)"
							else echo "[$(now)] ALERT $h: 테스트 비정상 종료(exit=$code) | $last"; fi
							alerted[$h:done]=1
						fi ;;
				esac
			done <<< "$out"
		done

		if [ $(( $(date +%s) - last_hb )) -ge "${C[heartbeat]}" ] && [ "$running" -gt 0 ]; then
			echo "[$(now)] 진행 중 (실행 중 클라이언트 $running)"
			printf '%s\n' "${hb_lines[@]}"
			last_hb=$(date +%s)
		fi
		if [ "$running" -eq 0 ]; then echo "[$(now)] 모든 클라이언트 테스트 종료: $RUN_ID"; break; fi
		if [ "$(date +%s)" -ge "$deadline" ]; then echo "[$(now)] ALERT 감시 제한 시간 초과: 테스트가 아직 실행 중일 수 있습니다"; exit 2; fi
		sleep "${C[poll]}"
	done
}

# ---------- abort ----------
cmd_abort() {
	resolve_run "${1:-latest}"
	local h
	for h in $(split_hosts "${C[clients]}"); do
		rssh "$h" "cd $RDIR || exit 0; [ -f testrust.pid ] || exit 0; p=\$(cat testrust.pid)
			kill -INT \$p 2>/dev/null || { echo '이미 종료됨'; exit 0; }
			for i in \$(seq 30); do kill -0 \$p 2>/dev/null || { echo 'SIGINT로 종료'; exit 0; }; sleep 1; done
			kill -TERM \$p 2>/dev/null; echo 'SIGTERM 전송'" | sed "s/^/$h: /"
	done
}

# ---------- collect ----------
cmd_collect() {
	local force=0 id=latest a
	for a in "$@"; do case "$a" in --force) force=1 ;; *) id="$a" ;; esac; done
	resolve_run "$id"
	local h

	if [ $force -eq 0 ]; then
		for h in $(split_hosts "${C[clients]}"); do
			if rssh "$h" "[ -f $RDIR/testrust.pid ] && kill -0 \$(cat $RDIR/testrust.pid) 2>/dev/null"; then
				die "$h에서 테스트가 아직 실행 중입니다. 종료를 기다리거나 abort 후 다시 수집하세요(강제: --force)."
			fi
		done
	fi

	for h in $(all_hosts); do rssh "$h" "touch $RDIR/stop" 2>/dev/null; done
	sleep $(( C[interval] + 1 ))

	mkdir -p "$LOCAL_DIR/hosts"
	for h in $(all_hosts); do
		local d="$LOCAL_DIR/hosts/$(host_dir "$h")"; mkdir -p "$d"
		if rssh "$h" "cd $RDIR && tar czf - --exclude=sampler.sh --exclude=stop ." | tar xzf - -C "$d"; then
			echo "[$(now)] 회수: $h"
		else
			echo "[$(now)] 회수 실패: $h" >&2
		fi
	done

	# MSYS_NO_PATHCONV 상태이므로 Windows Python에는 변환한 경로를 넘긴다
	local py_script="$SCRIPT_DIR/summarize.py" py_dir="$LOCAL_DIR"
	if command -v cygpath >/dev/null; then py_script="$(cygpath -m "$py_script")"; py_dir="$(cygpath -m "$py_dir")"; fi
	python "$py_script" "$py_dir" || echo "요약 생성 실패" >&2

	local dst="${C[result_dir]}/$RUN_ID"
	if tar czf - -C "$LOCAL_DIR" . | rssh "${C[collect_host]}" "mkdir -p $dst && tar xzf - -C $dst"; then
		echo "[$(now)] 저장: ${C[collect_host]}:$dst"
		if [ "${C[keep_remote]}" != 1 ]; then
			for h in $(all_hosts); do rssh "$h" "rm -rf $RDIR" 2>/dev/null; done
		fi
	else
		echo "수집 호스트 저장 실패: ${C[collect_host]}:$dst (원격 임시 파일은 유지)" >&2
	fi
	echo "SUMMARY=$LOCAL_DIR/summary.md"
}

case "${1:-}" in
	check) shift; cmd_check "$@" ;;
	start) shift; cmd_start "$@" ;;
	status) shift; cmd_status "$@" ;;
	wait) shift; cmd_wait "$@" ;;
	collect) shift; cmd_collect "$@" ;;
	abort) shift; cmd_abort "$@" ;;
	*) sed -n '2,13p' "$0"; exit 1 ;;
esac
