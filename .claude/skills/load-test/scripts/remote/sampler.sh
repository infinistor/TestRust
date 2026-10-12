#!/bin/bash
# 원격 호스트에서 시스템(및 선택적으로 프로세스) 사용량을 주기적으로 CSV에 기록한다.
# 사용법: sampler.sh <출력 CSV> <간격(초)> <최대 실행 시간(초)> [PID 파일]
# 같은 디렉터리에 stop 파일이 생기거나 최대 실행 시간이 지나면 종료한다.

OUT="$1"
INTERVAL="${2:-2}"
MAX_SECONDS="${3:-21600}"
PID_FILE="$4"
DIR="$(dirname "$OUT")"
STOP_FILE="$DIR/stop"
HZ="$(getconf CLK_TCK 2>/dev/null || echo 100)"

echo "ts_utc,epoch,cpu_used_pct,cpu_wait_pct,load1,load5,load15,mem_total_mb,mem_used_mb,mem_used_pct,swap_total_mb,swap_used_mb,proc_alive,proc_cpu_pct,proc_rss_mb,proc_threads" > "$OUT"

read_cpu() {
	# user nice system idle iowait irq softirq steal
	read -r _ u n s i w q sq st _ < /proc/stat
	CPU_TOTAL=$((u + n + s + i + w + q + sq + st))
	CPU_IDLE=$i
	CPU_WAIT=$w
}

read_proc() {
	PROC_TICKS=""
	[ -n "$PID_FILE" ] && [ -f "$PID_FILE" ] || return
	PID="$(cat "$PID_FILE" 2>/dev/null)"
	[ -n "$PID" ] && [ -r "/proc/$PID/stat" ] || return
	local stat rest
	stat="$(cat "/proc/$PID/stat" 2>/dev/null)" || return
	rest="${stat##*) }"
	# rest는 3번째 필드(state)부터 시작하므로 utime=12, stime=13번째
	PROC_TICKS="$(echo "$rest" | awk '{print $12 + $13}')"
	PROC_RSS_KB="$(awk '/^VmRSS:/{print $2}' "/proc/$PID/status" 2>/dev/null)"
	PROC_THREADS="$(awk '/^Threads:/{print $2}' "/proc/$PID/status" 2>/dev/null)"
}

read_cpu; PREV_TOTAL=$CPU_TOTAL; PREV_IDLE=$CPU_IDLE; PREV_WAIT=$CPU_WAIT
read_proc; PREV_TICKS=$PROC_TICKS
PREV_EPOCH="$(date +%s.%N)"
START=$(date +%s)

while :; do
	sleep "$INTERVAL"
	[ -f "$STOP_FILE" ] && break
	[ $(( $(date +%s) - START )) -ge "$MAX_SECONDS" ] && break

	EPOCH="$(date +%s.%N)"
	read_cpu
	read_proc
	read -r L1 L5 L15 _ < /proc/loadavg

	DT=$((CPU_TOTAL - PREV_TOTAL)); [ "$DT" -le 0 ] && DT=1
	CPU_LINE="$(awk -v dt="$DT" -v di=$((CPU_IDLE - PREV_IDLE)) -v dw=$((CPU_WAIT - PREV_WAIT)) \
		'BEGIN{printf "%.1f,%.1f", (dt-di-dw)*100/dt, dw*100/dt}')"
	MEM_LINE="$(awk '/^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} /^SwapTotal:/{st=$2} /^SwapFree:/{sf=$2}
		END{printf "%d,%d,%.1f,%d,%d", t/1024, (t-a)/1024, (t-a)*100/t, st/1024, (st-sf)/1024}' /proc/meminfo)"

	if [ -n "$PROC_TICKS" ]; then
		if [ -n "$PREV_TICKS" ]; then
			PROC_CPU="$(awk -v d=$((PROC_TICKS - PREV_TICKS)) -v hz="$HZ" -v t0="$PREV_EPOCH" -v t1="$EPOCH" \
				'BEGIN{s=t1-t0; if (s<=0) s=1; printf "%.1f", d/hz/s*100}')"
		else
			PROC_CPU=""
		fi
		PROC_LINE="1,$PROC_CPU,$(awk -v k="${PROC_RSS_KB:-0}" 'BEGIN{printf "%.1f", k/1024}'),${PROC_THREADS}"
	else
		PROC_LINE="0,,,"
	fi

	echo "$(date -u +%Y-%m-%dT%H:%M:%SZ),${EPOCH%.*},$CPU_LINE,$L1,$L5,$L15,$MEM_LINE,$PROC_LINE" >> "$OUT"

	PREV_TOTAL=$CPU_TOTAL; PREV_IDLE=$CPU_IDLE; PREV_WAIT=$CPU_WAIT
	PREV_TICKS=$PROC_TICKS; PREV_EPOCH=$EPOCH
done
