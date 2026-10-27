#!/usr/bin/env python3
"""Run one issue #1769 command; preserve its output and resource sample."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from datetime import datetime, timezone

root = Path(__file__).resolve().parents[3]
bench = root / ".benchmark-1769"
mode, threads, repetition = sys.argv[1:4]
threads = None if threads == "default" else int(threads)
if mode not in {"libtest", "nextest"}:
    raise SystemExit(f"unknown mode: {mode}")

env = os.environ.copy()
java_home = os.environ.get("BENCH_JAVA_HOME", "/home/vdrake6/.jdks/jdk-21")
(bench / "tmp").mkdir(parents=True, exist_ok=True)
env.update({
    "CARGO_TARGET_DIR": os.environ.get("BENCH_TARGET_DIR", str(bench / "target")),
    "CARGO_INCREMENTAL": "0",
    "CARGO_PROFILE_TEST_DEBUG": "0",
    "CARGO_TERM_COLOR": "never",
    "TMPDIR": str(bench / "tmp"),
    "JAVA_HOME": java_home,
})
env["PATH"] = env["JAVA_HOME"] + "/bin:" + env["PATH"]
env.pop("RUST_TEST_THREADS", None)
env.pop("NEXTEST_TEST_THREADS", None)
env.pop("NEXTEST_RETRIES", None)
if mode == "libtest":
    cmd = ["cargo", "test", "--workspace", "--all-targets", "--locked"]
    if threads is not None:
        env["RUST_TEST_THREADS"] = str(threads)
else:
    env["PATH"] = str(bench / "tool") + os.pathsep + env["PATH"]
    cmd = ["cargo", "nextest", "run", "--workspace", "--all-targets", "--locked", "--retries", "0", "--test-threads", str(threads if threads is not None else "num-cpus")]

run_id = f"{mode}-threads-{threads or 'default'}-rep-{repetition}"
evidence = root / "docs/ci-performance-baseline/issue-1769-evidence"
log_path = evidence / f"{run_id}.log"
result_path = evidence / f"{run_id}.json"
log = log_path.open("wb")
load_start = Path("/proc/loadavg").read_text().split()[:3]
start_time = datetime.now(timezone.utc).isoformat()
start = time.monotonic()
proc = subprocess.Popen(cmd, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
peak_tree = peak_single = 0

def descendants(pid):
    pending, seen = [pid], set()
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        try:
            pending.extend(int(child) for child in Path(f"/proc/{current}/task/{current}/children").read_text().split())
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    return seen

while proc.poll() is None:
    rss = []
    for pid in descendants(proc.pid):
        try:
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                if line.startswith("VmRSS:"):
                    rss.append(int(line.split()[1]))
                    break
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            pass
    peak_tree = max(peak_tree, sum(rss))
    peak_single = max(peak_single, max(rss, default=0))
    time.sleep(0.1)
code = proc.wait()
log.close()
result = {
    "run_id": run_id,
    "mode": mode,
    "threads": threads if threads is not None else "default/num-cpus",
    "command": cmd,
    "started_at_utc": start_time,
    "elapsed_seconds": round(time.monotonic() - start, 3),
    "exit_code": code,
    "peak_process_tree_rss_kib_sampled": peak_tree,
    "peak_single_process_rss_kib_sampled": peak_single,
    "rss_sampling_interval_seconds": 0.1,
    "rss_note": "sampled /proc VmRSS; process-tree sum includes shared pages more than once",
    "loadavg_at_start": load_start,
    "loadavg_at_end": Path("/proc/loadavg").read_text().split()[:3],
    "java_home": env["JAVA_HOME"],
    "java_version": subprocess.check_output([env["JAVA_HOME"] + "/bin/java", "-version"], stderr=subprocess.STDOUT, text=True).splitlines()[0],
    "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
    "log": str(log_path.relative_to(root)),
}
result_path.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result))
raise SystemExit(code)
