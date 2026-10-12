#!/usr/bin/env bash
# TESTCore(.NET)와 TestRust(Rust)를 같은 KSAN·같은 부하로 번갈아 실행해 처리량·평균 지연·최대 메모리를 비교한다.
#
#   ./compare.sh --net-ini testcore.ini --rs-ini TestRust.ini [--rounds 3] [--threads 32] [--times 60]
#                [--size 1M] [--files 32] [--net ./testcore/TESTCore] [--rs ./TestRust] [--out results]
#                [--bucket 이름 [--prefix testrust-perf/]]
#
# 각 INI는 접속 정보([Main User])와 버킷 이름([Default] BucketName)만 쓴다. 부하 값은 실행용 사본에서 덮어쓴다.
# 버킷은 기본으로 도구별 `{BucketName}-perf-net`·`-perf-rs`를 만들고, 끝나면 비운 뒤 지운다.
# 버킷을 만들 권한이 없으면 `--bucket`으로 기존 버킷 하나를 두 도구가 함께 쓴다. 이때 객체는 `--prefix` 아래
# (`ThreadPrefix = {prefix}TH`)에만 만들고, 끝나면 그 접두사 아래 객체만 지운다(버킷과 다른 객체는 그대로 둔다).
# 결과: {out}/summary.md(표·±10% 판정), {out}/runs.csv(원시 값), {out}/logs/*.log. 자격 증명은 남기지 않는다.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
net_ini="" rs_ini="" rounds=3 threads=32 times=60 size=1M files=32 shared_bucket="" prefix="testrust-perf/"
net_exe="$here/testcore/TESTCore" rs_exe="$here/TestRust" out="$here/results" tolerance=10
while [ $# -gt 0 ]; do
	case "$1" in
		--net-ini) net_ini=$2; shift 2 ;;
		--rs-ini) rs_ini=$2; shift 2 ;;
		--rounds) rounds=$2; shift 2 ;;
		--threads) threads=$2; shift 2 ;;
		--times) times=$2; shift 2 ;;
		--size) size=$2; shift 2 ;;
		--files) files=$2; shift 2 ;;
		--net) net_exe=$2; shift 2 ;;
		--rs) rs_exe=$2; shift 2 ;;
		--out) out=$2; shift 2 ;;
		--bucket) shared_bucket=$2; shift 2 ;;
		--prefix) prefix=$2; shift 2 ;;
		*) echo "알 수 없는 인자: $1" >&2; exit 2 ;;
	esac
done
[ -f "$net_ini" ] && [ -f "$rs_ini" ] || { echo "--net-ini, --rs-ini 파일이 필요합니다." >&2; exit 2; }
[ -x "$net_exe" ] && [ -x "$rs_exe" ] || { echo "실행 파일을 찾을 수 없습니다: $net_exe, $rs_exe" >&2; exit 2; }

mkdir -p "$out/logs" "$out/save"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# INI 섹션의 키 값을 바꾸거나, 없으면 섹션 끝(섹션이 없으면 파일 끝)에 넣는다.
ini_set() { # file section key value
	awk -v s="[$2]" -v k="$3" -v v="$4" '
		/^\[/ {
			if (in_s && !done) { print k " = " v; done = 1 }
			in_s = ($0 == s); if (in_s) found = 1
		}
		in_s && $0 ~ "^[ \t]*" k "[ \t]*=" { if (!done) { print k " = " v; done = 1 }; next }
		{ print }
		END {
			if (in_s && !done) { print k " = " v; done = 1 }
			if (!found) { print ""; print s; print k " = " v }
		}
	' "$1" > "$1.tmp" && mv "$1.tmp" "$1"
}

ini_get() { # file section key
	awk -v s="[$2]" -v k="$3" '/^\[/ { in_s = ($0 == s) } in_s && $0 ~ "^[ \t]*" k "[ \t]*=" { sub(/^[^=]*=[ \t]*/, ""); sub(/[ \t\r]*$/, ""); print; exit }' "$1"
}

# 실행용 INI: 원본을 복사하고 부하 값을 덮어쓴다.
make_ini() { # src dst suffix
	sed 's/\r$//' "$1" > "$2"
	local bucket
	if [ -n "$shared_bucket" ]; then
		bucket=$shared_bucket
		ini_set "$2" Default ThreadPrefix "${prefix}TH"
	else
		bucket="$(ini_get "$1" Default BucketName)-perf-$3"
	fi
	ini_set "$2" Default BucketName "$bucket"
	ini_set "$2" Default FileSize "$size"
	ini_set "$2" Default FilePath "$work/data-$3"
	ini_set "$2" UpDown ThreadCount "$threads"
	ini_set "$2" UpDown FileCount "$files"
	ini_set "$2" UpDown Times "$times"
	ini_set "$2" UpDown BucketType 4
	ini_set "$2" UpDown ReadRatio 1
	ini_set "$2" UpDown WriteRatio 1
	ini_set "$2" UpDown DeleteRatio 0
	ini_set "$2" UpDown ETagCheck false
	echo "$bucket"
}
net_bucket=$(make_ini "$net_ini" "$work/net.ini" net)
rs_bucket=$(make_ini "$rs_ini" "$work/rs.ini" rs)
endpoint=$(ini_get "$rs_ini" "Main User" URL)

# `times` 출력 파일에서 자식 프로세스 누적 CPU 초(user sys)를 읽는다.
# `times`는 그 셸이 거둔 자식만 세므로 run_tool 안에서 직접(명령 치환 없이) 파일로 받아야 한다.
children_cpu() { # times-file
	awk 'NR == 2 {
		split($1, u, "m"); split($2, s, "m"); sub("s", "", u[2]); sub("s", "", s[2])
		printf "%.3f %.3f", u[1] * 60 + u[2], s[1] * 60 + s[2]
	}' "$1"
}

# 프로세스를 실행하고 종료 코드·경과 초·최대 RSS(KiB)·CPU 초(user sys)를 돌려준다. 저장 JSON은 save/{tag}/ 아래.
# CPU 초는 실행 전후 자식 누적 시간의 차이다(감시용 awk·sleep 몇 ms가 섞이지만 두 도구에 똑같이 들어간다).
run_tool() { # tool tag menu
	local tool=$1 tag=$2 menu=$3 exe ini
	if [ "$tool" = net ]; then exe=$net_exe ini=$work/net.ini; else exe=$rs_exe ini=$work/rs.ini; fi
	mkdir -p "$out/save/$tag"
	local start end pid hwm=0 code cpu0 cpu1
	times > "$work/times0"
	start=$(date +%s.%N)
	"$exe" -c "$ini" "$menu" -s "$out/save/$tag" > "$out/logs/$tag.log" 2>&1 &
	pid=$!
	while kill -0 "$pid" 2>/dev/null; do
		local now
		now=$(awk '/^VmHWM:/ { print $2 }' "/proc/$pid/status" 2>/dev/null || echo 0)
		[ -n "$now" ] && [ "$now" -gt "$hwm" ] && hwm=$now
		sleep 0.5
	done
	wait "$pid" && code=0 || code=$?
	end=$(date +%s.%N)
	times > "$work/times1"
	cpu0=$(children_cpu "$work/times0")
	cpu1=$(children_cpu "$work/times1")
	echo "$code $(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.3f", b - a }') $hwm $(awk -v a="$cpu0" -v b="$cpu1" 'BEGIN {
		split(a, x, " "); split(b, y, " "); printf "%.3f %.3f", y[1] - x[1], y[2] - x[2]
	}')"
}

# 저장 JSON의 숫자 필드.
json_num() { # file key
	sed -n "s/^[[:space:]]*\"$2\":[[:space:]]*\(-\{0,1\}[0-9][0-9.]*\).*/\1/p" "$1" | head -1
}

echo "tool,scenario,round,exit,elapsed_s,max_rss_kib,read,read_failed,write,write_failed,ops_per_s,avg_latency_ms,cpu_user_s,cpu_sys_s,cpu_ms_per_op" > "$out/runs.csv"
record() { # tool scenario round result
	local tool=$1 scenario=$2 round=$3 code elapsed hwm cu cs json r rf w wf ops lat cpo
	read -r code elapsed hwm cu cs <<< "$4"
	json=$(ls -t "$out/save/$tool-$scenario-$round"/*.json 2>/dev/null | head -1 || true)
	if [ -n "$json" ]; then
		r=$(json_num "$json" read); rf=$(json_num "$json" readFailed)
		w=$(json_num "$json" write); wf=$(json_num "$json" writeFailed)
	else
		r=0 rf=0 w=0 wf=0
	fi
	# 처리량: 성공 건수 ÷ 설정 시간(두 도구가 같은 정의). 평균 지연: 스레드 수 ÷ 처리량(리틀 법칙).
	ops=$(awk -v r="$r" -v w="$w" -v t="$times" 'BEGIN { printf "%.3f", (r + w) / t }')
	lat=$(awk -v o="$ops" -v n="$threads" 'BEGIN { if (o > 0) printf "%.3f", n / o * 1000; else print "" }')
	# 요청당 CPU: (user + sys) ÷ 성공 건수.
	cpo=$(awk -v u="$cu" -v s="$cs" -v r="$r" -v w="$w" 'BEGIN { if (r + w > 0) printf "%.3f", (u + s) / (r + w) * 1000; else print "" }')
	echo "$tool,$scenario,$round,$code,$elapsed,$hwm,$r,$rf,$w,$wf,$ops,$lat,$cu,$cs,$cpo" >> "$out/runs.csv"
	echo "  $tool $scenario #$round: exit=$code ops/s=$ops avg_ms=$lat rss_kib=$hwm cpu=${cu}u/${cs}s cpu_ms/op=$cpo (fail r=$rf w=$wf)"
}

echo "대상: $endpoint, 버킷: $net_bucket / $rs_bucket, 스레드 $threads, ${times}초, 크기 $size, 회차 $rounds"
echo "Prepare(Get 대상 객체 준비)"
for tool in net rs; do
	record "$tool" prepare 0 "$(run_tool "$tool" "$tool-prepare-0" --test-prepare)"
done
for round in $(seq 1 "$rounds"); do
	if [ $((round % 2)) -eq 1 ]; then order="net rs"; else order="rs net"; fi
	for scenario in put get mix; do
		for tool in $order; do
			record "$tool" "$scenario" "$round" "$(run_tool "$tool" "$tool-$scenario-$round" "--test-$scenario")"
		done
	done
done
if [ -n "$shared_bucket" ]; then
	echo "정리(${shared_bucket}의 ${prefix} 아래 객체만 삭제, 버킷 보존)"
	"$rs_exe" -c "$work/rs.ini" "--bucket-clear=$shared_bucket" "--prefix=$prefix" > "$out/logs/rs-clear.log" 2>&1 || true
else
	echo "정리(버킷 비우기·삭제)"
	"$net_exe" -c "$work/net.ini" "--bucket-clear=$net_bucket" --flag=true > "$out/logs/net-clear.log" 2>&1 || true
	"$rs_exe" -c "$work/rs.ini" "--bucket-clear=$rs_bucket" --flag=true > "$out/logs/rs-clear.log" 2>&1 || true
fi

# 요약: 시나리오별 시스템 평균·Rust/.NET 비율·±tolerance% 판정과 회차별 전체 값.
# CPU 사용률은 장비 전체 코어 대비(코어 수 = CPU 초 ÷ 경과 초), 메모리 사용률은 장비 전체 메모리 대비 최대 RSS다.
ncpu=$(nproc)
mem_kib=$(awk '/^MemTotal:/ { print $2 }' /proc/meminfo)
cpu_model=$(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo)
awk -F, -v tol="$tolerance" -v endpoint="$endpoint" -v threads="$threads" -v times="$times" -v size="$size" \
	-v rounds="$rounds" -v files="$files" -v ncpu="$ncpu" -v mem_kib="$mem_kib" -v cpu_model="$cpu_model" \
	-v buckets="$(if [ -n "$shared_bucket" ]; then echo "$shared_bucket(공용, 접두사 $prefix)"; else echo "$net_bucket / $rs_bucket"; fi)" '
	function name(t) { return t == "net" ? "**.NET**" : "**Rust**" }
	function pct(x, y) { return y > 0 ? sprintf("%d", x / y * 100) : "-" }
	NR == 1 || $2 == "prepare" { next }
	{
		t = $1; s = $2; r = $3; k = t SUBSEP s; kr = k SUBSEP r
		cores = $5 > 0 ? ($13 + $14) / $5 : 0
		n[k]++; ops[k] += $11; lat[k] += $12; cor[k] += cores; cpo[k] += $15; cu[k] += $13; cs[k] += $14
		rss[k] = ($6 > rss[k] ? $6 : rss[k]); rsum[k] += $6
		fail[k] += $8 + $10; if ($4 != 0) bad[k]++
		if (min[k] == "" || $11 < min[k]) min[k] = $11; if ($11 > max[k]) max[k] = $11
		R_ops[kr] = $11; R_lat[kr] = $12; R_cor[kr] = cores; R_cpo[kr] = $15; R_cu[kr] = $13; R_cs[kr] = $14
		R_rss[kr] = $6; R_fail[kr] = $8 + $10; R_code[kr] = $4; R_el[kr] = $5
	}
	END {
		print "# TESTCore(.NET) vs TestRust(Rust) 성능 비교\n"
		printf "- 대상: %s, 버킷: %s\n", endpoint, buckets
		printf "- 클라이언트: %s, %s 코어, 메모리 %.1f GiB\n", cpu_model, ncpu, mem_kib / 1048576
		printf "- 부하: 스레드 %s, %s초, 파일 크기 %s, 스레드당 파일 %s개, 회차 %s(회차마다 실행 순서를 바꿔 번갈아 실행)\n", threads, times, size, files, rounds
		print "- 처리량: 성공 건수 ÷ 설정 시간. 평균 지연: 스레드 ÷ 처리량(리틀 법칙)."
		print "- CPU 코어: (user + sys) ÷ 경과 초. CPU 사용률: CPU 코어 ÷ 장비 코어 수. CPU ms/op: (user + sys) ÷ 성공 건수."
		print "- 메모리: 최대 RSS(VmHWM). 메모리 사용률: 최대 RSS ÷ 장비 메모리."
		printf "- 판정: 처리량·평균 지연 비율이 ±%s%% 안이고 실패·비정상 종료가 없으면 통과(CPU·메모리는 기록만).\n", tol
		print "- 표의 값은 소수점 아래를 버린 정수다. Rust/.NET 행은 비율을 백분율(%)로 나타낸다(100 = 같음). 원시 값은 runs.csv에 있다.\n"

		split("put get mix", list, " ")
		print "## 요약(회차 평균)\n"
		print "| 시나리오 | 시스템 | 처리량 ops/s (최소~최대) | 평균 지연 ms | CPU 코어 | CPU 사용률 % | CPU ms/op | 최대 RSS MiB | 메모리 사용률 % | 실패 | 판정 |"
		print "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |"
		for (i = 1; i <= 3; i++) {
			s = list[i]; a = "net" SUBSEP s; b = "rs" SUBSEP s
			if (!n[a] || !n[b]) continue
			for (j = 1; j <= 2; j++) {
				t = j == 1 ? "net" : "rs"; k = t SUBSEP s
				A_ops[t] = ops[k] / n[k]; A_lat[t] = lat[k] / n[k]; A_cor[t] = cor[k] / n[k]; A_cpo[t] = cpo[k] / n[k]
				printf "| %s | %s | %d (%d~%d) | %d | %d | %d | %d | %d | %d | %d | |\n", s, name(t), A_ops[t], min[k], max[k], A_lat[t], A_cor[t], A_cor[t] / ncpu * 100, A_cpo[t], rss[k] / 1024, rss[k] / mem_kib * 100, fail[k]
			}
			ratio = A_ops["net"] > 0 ? A_ops["rs"] / A_ops["net"] : 0; lratio = A_lat["net"] > 0 ? A_lat["rs"] / A_lat["net"] : 0
			ok = (ratio >= 1 - tol / 100 && ratio <= 1 + tol / 100 && lratio >= 1 - tol / 100 && lratio <= 1 + tol / 100 && !fail[a] && !fail[b] && !bad[a] && !bad[b])
			printf "| %s | Rust/.NET %% | %d | %d | %s | %s | %s | %s | %s | | %s |\n", s, ratio * 100, lratio * 100, pct(A_cor["rs"], A_cor["net"]), pct(A_cor["rs"], A_cor["net"]), pct(A_cpo["rs"], A_cpo["net"]), pct(rss[b], rss[a]), pct(rss[b], rss[a]), ok ? "통과" : "범위 밖"
		}

		print "\n## 회차별 값\n"
		print "실행 순서 1은 그 회차에서 먼저 실행한 시스템이다(홀수 회차는 .NET, 짝수 회차는 Rust가 먼저)."
		for (i = 1; i <= 3; i++) {
			s = list[i]
			if (!n["net" SUBSEP s] || !n["rs" SUBSEP s]) continue
			printf "\n### %s\n\n", s
			print "| 시스템 | 회차 | 실행 순서 | 처리량 ops/s | 평균 지연 ms | CPU 코어 | CPU 사용률 % | CPU ms/op | user s | sys s | 최대 RSS MiB | 메모리 사용률 % | 실패 | 종료 코드 |"
			print "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
			for (j = 1; j <= 2; j++) {
				t = j == 1 ? "net" : "rs"; k = t SUBSEP s
				for (r = 1; r <= rounds; r++) {
					kr = k SUBSEP r
					if (!(kr in R_ops)) continue
					order = (r % 2 == 1) == (t == "net") ? 1 : 2
					printf "| %s | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d |\n", name(t), r, order, R_ops[kr], R_lat[kr], R_cor[kr], R_cor[kr] / ncpu * 100, R_cpo[kr], R_cu[kr], R_cs[kr], R_rss[kr] / 1024, R_rss[kr] / mem_kib * 100, R_fail[kr], R_code[kr]
				}
				printf "| %s | 평균 | | %d | %d | %d | %d | %d | %d | %d | %d | %d | %d | |\n", name(t), ops[k] / n[k], lat[k] / n[k], cor[k] / n[k], cor[k] / n[k] / ncpu * 100, cpo[k] / n[k], cu[k] / n[k], cs[k] / n[k], rsum[k] / n[k] / 1024, rsum[k] / n[k] / mem_kib * 100, fail[k]
			}
		}
	}' "$out/runs.csv" > "$out/summary.md"
cat "$out/summary.md"
