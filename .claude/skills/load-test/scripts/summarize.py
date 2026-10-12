"""load-test 실행 결과 요약: <LOCAL_DIR>/hosts/*/ 를 읽어 summary.md, summary.json 생성.

사용법: python summarize.py <LOCAL_DIR>
"""
import csv
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

MIB = 1024 * 1024


def load_conf(path):
    conf = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith(("#", ";")) or "=" not in line:
            continue
        k, v = line.split("=", 1)
        conf[k.strip()] = v.strip()
    return conf


def parse_utc(text):
    try:
        return datetime.strptime(text.strip(), "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp()
    except (ValueError, AttributeError):
        return None


def read_text(path):
    try:
        return path.read_text(encoding="utf-8", errors="replace").strip()
    except OSError:
        return None


def read_metrics(path):
    if not path.exists():
        return []
    rows = []
    with path.open(encoding="utf-8", errors="replace") as f:
        for r in csv.DictReader(f):
            try:
                r["epoch"] = float(r["epoch"])
            except (KeyError, ValueError):
                continue
            rows.append(r)
    return rows


def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def stats(rows, key):
    vals = [x for x in (num(r.get(key)) for r in rows) if x is not None]
    if not vals:
        return None
    return {"avg": round(sum(vals) / len(vals), 1), "max": round(max(vals), 1), "min": round(min(vals), 1)}


def window(rows, start, end):
    if start is None or end is None:
        return rows
    return [r for r in rows if start <= r["epoch"] <= end]


def lowered(d):
    return {k.lower(): v for k, v in d.items()} if isinstance(d, dict) else {}


def summarize_result(res):
    r = lowered(res)
    t = r.get("time") or 0
    size = r.get("filesize") or 0
    out = {"time_s": t, "file_size": size}
    for op in ("read", "write", "head", "delete", "list"):
        ok, fail = r.get(op) or 0, r.get(op + "failed") or 0
        if ok or fail:
            ops = ok / t if t else None
            out[op] = {
                "success": ok,
                "failed": fail,
                "ops_per_s": round(ops, 1) if ops is not None else None,
                "mib_per_s": round(ops * size / MIB, 1) if ops is not None and op in ("read", "write") else None,
            }
    return out


def fmt(v, unit=""):
    return "-" if v is None else f"{v}{unit}"


def fmt_stat(s, unit=""):
    return "-" if not s else f"{s['avg']}{unit} / {s['max']}{unit}"


def main():
    run_dir = Path(sys.argv[1])
    conf = load_conf(run_dir / "run.conf")
    run_id = run_dir.name
    clients = conf.get("clients", "").replace(",", " ").split()
    servers = conf.get("servers", "").replace(",", " ").split()
    hdir = lambda h: run_dir / "hosts" / re.sub(r"[^A-Za-z0-9._-]", "_", h)

    # 클라이언트 테스트 결과
    client_results, starts, ends = {}, [], []
    for h in clients:
        d = hdir(h)
        start, end = parse_utc(read_text(d / "started_at") or ""), parse_utc(read_text(d / "finished_at") or "")
        if start: starts.append(start)
        if end: ends.append(end)
        log = read_text(d / "testrust.log") or ""
        entry = {
            "exit_code": read_text(d / "exitcode"),
            "started_at": read_text(d / "started_at"),
            "finished_at": read_text(d / "finished_at"),
            "log_error_lines": sum(1 for line in log.splitlines() if re.search(r"\b(ERROR|Exception)\b", line)),
            "log_tail": log.splitlines()[-15:],
        }
        res_path = d / "result.json"
        if res_path.exists():
            try:
                entry["result"] = summarize_result(json.loads(res_path.read_text(encoding="utf-8-sig")))
            except (OSError, ValueError) as e:
                entry["result_error"] = str(e)
        else:
            entry["result_error"] = "result.json 없음"
        client_results[h] = entry

    test_start = min(starts) if starts else None
    test_end = max(ends) if ends else None

    # 전체 합산 (클라이언트별 ops/s 합)
    total = {}
    for e in client_results.values():
        for op, v in (e.get("result") or {}).items():
            if not isinstance(v, dict):
                continue
            t = total.setdefault(op, {"success": 0, "failed": 0, "ops_per_s": 0.0, "mib_per_s": 0.0})
            t["success"] += v["success"]
            t["failed"] += v["failed"]
            t["ops_per_s"] = round(t["ops_per_s"] + (v["ops_per_s"] or 0), 1)
            t["mib_per_s"] = round(t["mib_per_s"] + (v["mib_per_s"] or 0), 1)

    # 자원 사용량 (테스트 구간 / 테스트 전 기준값)
    def host_usage(h, fname, with_proc):
        rows = read_metrics(hdir(h) / fname)
        if not rows:
            return None
        base = [r for r in rows if test_start and r["epoch"] < test_start]
        run = window(rows, test_start, test_end)
        u = {
            "samples": len(run),
            "cpu_used_pct": stats(run, "cpu_used_pct"),
            "cpu_wait_pct": stats(run, "cpu_wait_pct"),
            "load1": stats(run, "load1"),
            "load5": stats(run, "load5"),
            "load15": stats(run, "load15"),
            "mem_used_mb": stats(run, "mem_used_mb"),
            "mem_used_pct": stats(run, "mem_used_pct"),
            "swap_used_mb": stats(run, "swap_used_mb"),
            "mem_total_mb": num(rows[-1].get("mem_total_mb")),
            "baseline": {
                "cpu_used_pct": stats(base, "cpu_used_pct"),
                "mem_used_mb": stats(base, "mem_used_mb"),
                "swap_used_mb": stats(base, "swap_used_mb"),
            },
        }
        if with_proc:
            alive = [r for r in run if r.get("proc_alive") == "1"]
            u["proc"] = {
                "cpu_pct": stats(alive, "proc_cpu_pct"),
                "rss_mb": stats(alive, "proc_rss_mb"),
                "threads": stats(alive, "proc_threads"),
            }
        return u

    server_usage = {h: host_usage(h, "metrics-server.csv", False) for h in servers}
    client_usage = {h: host_usage(h, "metrics-client.csv", True) for h in clients}

    summary = {
        "run_id": run_id,
        "params": {k: conf.get(k) for k in ("type", "size", "thread", "count", "times", "bucket", "prefix",
                                             "read", "write", "delete", "servers", "clients")},
        "test_window_utc": [
            datetime.fromtimestamp(test_start, timezone.utc).isoformat() if test_start else None,
            datetime.fromtimestamp(test_end, timezone.utc).isoformat() if test_end else None,
        ],
        "total": total,
        "clients": client_results,
        "client_usage": client_usage,
        "server_usage": server_usage,
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")

    # Markdown
    p = summary["params"]
    md = [f"# 부하 테스트 결과: {run_id}", ""]
    md.append(f"- 종류: **{p['type']}**, 크기 {p['size']}, 스레드 {p['thread']}/클라이언트, "
              f"count {p['count'] or '-'}, times {p['times'] or '-'}s")
    md.append(f"- 버킷 `{p['bucket']}`, prefix `{p['prefix']}`"
              + (f", 비율 R{p['read'] or 0}/W{p['write'] or 0}/D{p['delete'] or 0}" if p['type'] == "mix" else ""))
    md.append(f"- 서버: {p['servers']} / 클라이언트: {p['clients']}")
    md.append(f"- 테스트 구간(UTC): {summary['test_window_utc'][0]} ~ {summary['test_window_utc'][1]}")
    md += ["", "## 처리량 (전체 합산)", "", "| 연산 | 성공 | 실패 | ops/s | MiB/s |", "|---|---:|---:|---:|---:|"]
    for op, v in total.items():
        md.append(f"| {op} | {v['success']} | {v['failed']} | {v['ops_per_s']} | {v['mib_per_s'] if op in ('read', 'write') else '-'} |")
    if not total:
        md.append("| (결과 없음) | | | | |")

    md += ["", "## 클라이언트별 결과", "", "| 클라이언트 | exit | 시간(s) | 연산 | 성공 | 실패 | ops/s | MiB/s | 로그 오류 |",
           "|---|---:|---:|---|---:|---:|---:|---:|---:|"]
    for h, e in client_results.items():
        res = e.get("result") or {}
        ops = [(k, v) for k, v in res.items() if isinstance(v, dict)]
        if not ops:
            md.append(f"| {h} | {fmt(e['exit_code'])} | - | {e.get('result_error', '-')} | | | | | {e['log_error_lines']} |")
        for k, v in ops:
            md.append(f"| {h} | {fmt(e['exit_code'])} | {res.get('time_s')} | {k} | {v['success']} | {v['failed']} | "
                      f"{fmt(v['ops_per_s'])} | {fmt(v['mib_per_s'])} | {e['log_error_lines']} |")

    md += ["", "## 서버 자원 (테스트 구간 평균 / 최대)", "",
           "| 서버 | CPU used | CPU wait | load 1 / 5 / 15 (평균) | 메모리 used | swap used | 기준 CPU | 기준 메모리 |",
           "|---|---:|---:|---|---:|---:|---:|---:|"]
    for h, u in server_usage.items():
        if not u:
            md.append(f"| {h} | 지표 없음 | | | | | | |")
            continue
        load = " / ".join(fmt(u[k]["avg"] if u[k] else None) for k in ("load1", "load5", "load15"))
        md.append(f"| {h} | {fmt_stat(u['cpu_used_pct'], '%')} | {fmt_stat(u['cpu_wait_pct'], '%')} | {load} | "
                  f"{fmt_stat(u['mem_used_mb'], 'MB')} | {fmt_stat(u['swap_used_mb'], 'MB')} | "
                  f"{fmt_stat(u['baseline']['cpu_used_pct'], '%')} | {fmt_stat(u['baseline']['mem_used_mb'], 'MB')} |")

    md += ["", "## 클라이언트 자원 (테스트 구간 평균 / 최대)", "",
           "| 클라이언트 | TestRust CPU | TestRust RSS | 스레드 | 시스템 CPU used | CPU wait | load1 | 메모리 used | swap used |",
           "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for h, u in client_usage.items():
        if not u:
            md.append(f"| {h} | 지표 없음 | | | | | | | |")
            continue
        pr = u["proc"]
        md.append(f"| {h} | {fmt_stat(pr['cpu_pct'], '%')} | {fmt_stat(pr['rss_mb'], 'MB')} | {fmt_stat(pr['threads'])} | "
                  f"{fmt_stat(u['cpu_used_pct'], '%')} | {fmt_stat(u['cpu_wait_pct'], '%')} | {fmt_stat(u['load1'])} | "
                  f"{fmt_stat(u['mem_used_mb'], 'MB')} | {fmt_stat(u['swap_used_mb'], 'MB')} |")
    md += ["", "> TestRust CPU는 코어 1개 = 100% 기준(멀티코어 사용 시 100% 초과). 원시 지표: `hosts/<호스트>/metrics-*.csv`", ""]

    (run_dir / "summary.md").write_text("\n".join(md), encoding="utf-8")
    print(f"summary: {run_dir / 'summary.md'}")


if __name__ == "__main__":
    main()
