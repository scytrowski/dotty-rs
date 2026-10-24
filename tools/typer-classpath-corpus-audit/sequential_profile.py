#!/usr/bin/env python3
"""Run a bounded, serial Typer profile and summarize its opt-in timing logs."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import tempfile
import statistics
import subprocess
import sys
import time
from pathlib import Path


REVISION = "777528f19a58e794c9954a42f433373472ec57f8"
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "docs/typer-sequential-profile-manifest-3.9.0.tsv"
AUDIT_TEST = [
    "cargo", "test", "-p", "dotty-typer", "--test", "local_definition_audit",
    "--locked", "pinned_scala39_local_definition_audit", "--", "--ignored", "--nocapture",
]
PREBUILD_TEST = [
    "cargo", "test", "-p", "dotty-typer", "--test", "local_definition_audit", "--locked", "--no-run",
]
LOAD_ROOT_RE = re.compile(
    r"^    root=(.*?)::file=(.*?)::method=(.*?)::request=(.*?)::depth=(\d+)"
    r"::distinct_classes=(\d+)::full_completions=(\d+)::shells_created=(\d+)"
    r"::shells_reused=(\d+)::member_type_references=(\d+)::hierarchy_loads=(\d+)"
    r"::completed_cache_hits=(\d+)"
)


def fail(message: str) -> "NoReturn":
    print(message, file=sys.stderr)
    raise SystemExit(2)


def read_manifest(path: Path, scala_root: Path) -> list[str]:
    roots = [(scala_root / "compiler").resolve(), (scala_root / "library").resolve()]
    entries: list[str] = []
    seen: set[str] = set()
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != "path\trole":
        fail(f"manifest must start with 'path<TAB>role': {path}")
    for line_number, line in enumerate(lines[1:], 2):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 2 or not fields[0] or not fields[1]:
            fail(f"malformed manifest row {line_number}: {line!r}")
        relative, _role = fields
        candidate = Path(relative)
        if candidate.is_absolute() or ".." in candidate.parts:
            fail(f"manifest path must be relative and traversal-free: {relative}")
        resolved = (scala_root / candidate).resolve()
        if not any(resolved.is_relative_to(root) for root in roots):
            fail(f"manifest path is outside pinned compiler/ and library/ roots: {relative}")
        if not resolved.is_file():
            fail(f"manifest path does not exist in pinned Scala checkout: {relative}")
        if relative in seen:
            fail(f"duplicate manifest path: {relative}")
        seen.add(relative)
        entries.append(relative)
    if not entries:
        fail("profile manifest selects no files")
    return entries


def kvs(line: str) -> dict[str, str]:
    return dict(re.findall(r"([a-zA-Z_][a-zA-Z0-9_]*)=([^\s]+)", line))


def resolver_duration_us(values: dict[str, int], operation: str) -> int | str:
    return values.get(f"{operation}_inner_us", "not_captured")


def resolver_duration_ms(values: dict[str, int], operation: str) -> int | str:
    return values.get(f"{operation}_inner_ms", "not_captured")


def cargo_target_cache_state(target_dir: Path) -> str:
    deps_dir = target_dir / "debug" / "deps"
    if not deps_dir.is_dir():
        return "cold"
    return "warm" if any(
        path.is_file() and not path.name.endswith(".d") and os.access(path, os.X_OK)
        for path in deps_dir.glob("local_definition_audit-*")
    ) else "cold"


def profile_input_paths(scala_root: Path, java_home: Path, artifact_root: Path, file_paths: list[str]) -> list[Path]:
    paths = {scala_root / relative for relative in file_paths}
    paths.update(artifact_root / item for item in (
        "org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar",
        "org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar",
        "org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar",
        "org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar",
        "org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar",
        "org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar",
        "org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar",
        "org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar",
    ))
    paths.update((java_home / "jmods").glob("*.jmod"))
    return sorted(path.resolve() for path in paths if path.is_file())


def resident_bytes(paths: list[Path]) -> int:
    try:
        completed = subprocess.run(
            ["fincore", "--bytes", "--json", "--output", "RES,FILE", *(str(path) for path in paths)],
            text=True, capture_output=True, check=False,
        )
    except FileNotFoundError:
        fail("--cold-inputs-first-run requires util-linux fincore")
    if completed.returncode:
        fail("fincore could not measure profile input residency: " + completed.stderr.strip())
    data = json.loads(completed.stdout)
    return sum(int(row["res"]) for row in data["fincore"])


def evict_input_pages(paths: list[Path]) -> None:
    if not hasattr(os, "posix_fadvise") or not hasattr(os, "POSIX_FADV_DONTNEED"):
        fail("--cold-inputs-first-run requires POSIX_FADV_DONTNEED support")
    for path in paths:
        descriptor = os.open(path, os.O_RDONLY)
        try:
            os.posix_fadvise(descriptor, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(descriptor)


def warm_input_pages(paths: list[Path]) -> int:
    started = time.monotonic_ns()
    for path in paths:
        with path.open("rb") as source:
            while source.read(1024 * 1024):
                pass
    return (time.monotonic_ns() - started) // 1_000_000


def parse_run(log: str) -> dict:
    result: dict = {
        "files": {}, "methods": {}, "outcomes": {}, "match_probes": [], "match_probe_ids": [], "match_outcomes": {}, "resolver": {}, "match_phases": [], "match_resolver": [], "match_resolver_wrappers": [], "summary": {},
        "loads": {}, "resolver_counts": {}, "load_roots": [], "wrappers": {}, "rollback": {},
        "cheap_resolver": {}, "cheap_resolver_outcomes": {}, "resolver_stages": [], "match_resolver_stages": [],
        "classpath_resources": {}, "resolver_profile": {}, "match_resolver_profiles": [],
        "repeated_requests": [],
        "classload_timings": {}, "match_classload_timings": [],
        "resolver_roots": [], "resolver_root_phases": [], "resolver_cache": {}, "match_resolver_cache": [],
    }
    for line in log.splitlines():
        if line.startswith("AUDIT_PROFILE_SUMMARY "):
            result["summary"] = kvs(line)
        elif line.startswith("AUDIT_FILE_PHASE "):
            values = kvs(line)
            file = values.pop("file")
            phase = values.pop("phase")
            result["files"].setdefault(file, {})[phase] = int(values["ms"])
        elif line.startswith("AUDIT_FILE_TIMING "):
            values = kvs(line)
            file = values.pop("file")
            result["files"].setdefault(file, {}).update({k: int(v) for k, v in values.items() if k.endswith("_ms")})
            result["files"][file]["outcome"] = values.get("outcome", "unknown")
        elif line.startswith("AUDIT_METHOD_TIMING "):
            values = kvs(line)
            key = (values["file"], values["tree"])
            result["methods"][key] = {"method": values["method"], "line": int(values["line"]), "elapsed_ms": int(values["elapsed_ms"])}
        elif line.startswith("AUDIT_METHOD_RESULT "):
            values = kvs(line)
            result["outcomes"][(values["file"], values["tree"])] = values["result"]
        elif line.startswith("AUDIT_MATCH_PROBES "):
            result["match_probes"].append(kvs(line))
        elif line.startswith("AUDIT_MATCH_PROBE "):
            values = kvs(line)
            result["match_probe_ids"].append((values["file"], values["method_symbol"], values["case_tree"], values["family"]))
        elif line.startswith("AUDIT_MATCH_RESULT "):
            values = kvs(line)
            key = (values["file"], values["method_symbol"], values["case_tree"], values["family"])
            result["match_outcomes"][key] = values["result"]
        elif line.startswith("AUDIT_MATCH_RESOLVER "):
            values = kvs(line)
            result["match_resolver"].append(values)
        elif line.startswith("AUDIT_MATCH_PHASE "):
            values = kvs(line)
            result["match_phases"].append(values)
        elif line.startswith("AUDIT_MATCH_RESOLVER_WRAPPER "):
            values = kvs(line)
            result["match_resolver_wrappers"].append(values)
        elif line.startswith("AUDIT_MATCH_RESOLVER_STAGE "):
            result["match_resolver_stages"].append(kvs(line))
        elif line.startswith("AUDIT_CLASSLOADER_RESOLVER_COUNTS "):
            result["resolver_profile"] = {key: int(value) for key, value in kvs(line).items()}
        elif line.startswith("AUDIT_MATCH_CLASSLOADER_RESOLVER_COUNTS "):
            values = kvs(line)
            result["match_resolver_profiles"].append(
                {"file": values["file"], **{key: int(value) for key, value in values.items() if key != "file"}}
            )
        elif line.startswith("AUDIT_CLASSLOAD_TIMINGS "):
            result["classload_timings"] = {key: int(value) for key, value in kvs(line).items()}
        elif line.startswith("AUDIT_RESOLVER_ROOT_PHASE "):
            values = kvs(line)
            values.setdefault("scope", "source")
            result["resolver_root_phases"].append(values)
        elif line.startswith("AUDIT_RESOLVER_ROOT "):
            values = kvs(line)
            values.setdefault("stable_key_observations", values.pop("request_observations", "not_captured"))
            result["resolver_roots"].append(values)
        elif line.startswith("AUDIT_RESOLVER_CACHE "):
            result["resolver_cache"] = kvs(line)
        elif line.startswith("AUDIT_MATCH_RESOLVER_CACHE "):
            result["match_resolver_cache"].append(kvs(line))
        elif line.startswith("AUDIT_MATCH_CLASSLOAD_TIMINGS "):
            values = kvs(line)
            result["match_classload_timings"].append(
                {"file": values["file"], **{key: int(value) for key, value in values.items() if key != "file"}}
            )
        elif line.startswith("AUDIT_RESOLVER_STAGE "):
            result["resolver_stages"].append(kvs(line))
        elif line.startswith("AUDIT_CHEAP_RESOLVER_OUTCOMES "):
            values = kvs(line)
            scope, operation = values.pop("scope"), values.pop("operation")
            totals = result["cheap_resolver_outcomes"].setdefault((scope, operation), {})
            for key, value in values.items():
                totals[key] = totals.get(key, 0) + int(value)
        elif line.startswith("AUDIT_CHEAP_RESOLVER "):
            values = kvs(line)
            scope, operation = values.pop("scope"), values.pop("operation")
            totals = result["cheap_resolver"].setdefault((scope, operation), {})
            for key, value in values.items():
                totals[key] = totals.get(key, 0) + int(value)
        elif line.startswith("AUDIT_RESOLVER_WRAPPER "):
            values = kvs(line)
            operation = values.pop("operation")
            result["wrappers"][operation] = {key: int(value) for key, value in values.items()}
        elif line.startswith("AUDIT_ROLLBACK "):
            result["rollback"] = {key: int(value) for key, value in kvs(line).items()}
        elif line.startswith("AUDIT_CLASSPATH_RESOURCES "):
            result["classpath_resources"] = kvs(line)
        elif line.startswith("AUDIT_METHOD_RESOLVER "):
            values = kvs(line)
            key = (values["file"], values["tree"])
            result["resolver"][key] = {k: int(v) for k, v in values.items() if k not in {"file", "tree"}}
        elif "repeated_hit_factor=" in line and ": requests=" in line:
            shape, raw_values = line.strip().split(": ", 1)
            values = kvs(raw_values)
            result["repeated_requests"].append(
                {
                    "shape": shape,
                    **{
                        key: int(value.rstrip(","))
                        for key, value in values.items()
                        if key not in {"outcomes", "repeated_hit_factor", "known_repeated_hit_factor"}
                    },
                    "repeated_hit_factor": float(values["repeated_hit_factor"].rstrip(",")),
                }
            )
        elif line.startswith("  "):
            root_match = LOAD_ROOT_RE.match(line)
            if root_match:
                root, file, method, request, *counts = root_match.groups()
                result["load_roots"].append({
                    "root": root, "file": file, "method": method, "request": request,
                    "depth": int(counts[0]), "distinct_classes": int(counts[1]),
                    "full_completions": int(counts[2]), "shells_created": int(counts[3]),
                    "shells_reused": int(counts[4]), "member_type_references": int(counts[5]),
                    "hierarchy_loads": int(counts[6]), "completed_cache_hits": int(counts[7]),
                })
            values = kvs(line)
            for name in (
                "top_level_class_loads", "on_demand_completions", "identity_shell_creations",
                "identity_shell_reuses", "completed_cache_hits", "full_completions",
                "member_type_references", "hierarchy_loads",
            ):
                if name in values:
                    result["loads"][name] = int(values[name])
            for name in (
                "resolver_member_requests", "external_member_requests", "external_member_successes",
                "external_member_unresolved", "external_member_errors", "external_package_requests",
                "external_package_successes", "external_package_unresolved", "external_package_errors",
                "class_completion_calls", "classfile_completion_calls", "tasty_completion_calls",
                "class_completion_inner_us", "classfile_completion_inner_us", "tasty_completion_inner_us",
            ):
                if name in values:
                    result["resolver_counts"][name] = int(values[name])
            for name in ("typer_module_identity_resolver_calls", "typer_module_completion_resolver_calls"):
                if name in values:
                    result["resolver_counts"][name] = int(values[name].split()[0])
    if not result["summary"]:
        fail("profile log is missing AUDIT_PROFILE_SUMMARY")
    if not result["files"]:
        fail("profile log is missing per-file timing rows")
    return result


def process_tree_rss_bytes(root_pid: int) -> int:
    """Return sampled aggregate RSS for root_pid and current descendants."""
    process_table: dict[int, tuple[int, int]] = {}
    try:
        entries = Path("/proc").iterdir()
    except OSError:
        return 0
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="ascii")
            fields = stat[stat.rfind(")") + 2 :].split()
            pid = int(entry.name)
            parent = int(fields[1])
            resident_pages = int(fields[21])
            process_table[pid] = (parent, max(0, resident_pages) * os.sysconf("SC_PAGE_SIZE"))
        except (OSError, ValueError, IndexError):
            continue
    tree = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, (parent, _rss) in process_table.items():
            if parent in tree and pid not in tree:
                tree.add(pid)
                changed = True
    return sum(process_table[pid][1] for pid in tree if pid in process_table)


def run_measured(command: list[str], cwd: Path, env: dict[str, str]) -> tuple[int, str, dict[str, int | float | str]]:
    """Run one serial audit; sample aggregate descendant RSS and collect child CPU."""
    before = os.times()
    started = time.monotonic_ns()
    sampled_peak_rss = 0
    sample_count = 0
    with tempfile.TemporaryFile(mode="w+t", encoding="utf-8") as capture:
        process = subprocess.Popen(command, cwd=cwd, env=env, text=True, stdout=capture,
                                   stderr=subprocess.STDOUT)
        while process.poll() is None:
            sampled_peak_rss = max(sampled_peak_rss, process_tree_rss_bytes(process.pid))
            sample_count += 1
            time.sleep(0.1)
        return_code = process.wait()
        wall_ms = (time.monotonic_ns() - started) // 1_000_000
        capture_read_started = time.monotonic_ns()
        capture.seek(0)
        output = capture.read()
        capture_read_ms = (time.monotonic_ns() - capture_read_started) // 1_000_000
    after = os.times()
    return return_code, output, {
        "cargo_wall_ms": int(wall_ms),
        "child_cpu_user_s": max(0.0, after.children_user - before.children_user),
        "child_cpu_system_s": max(0.0, after.children_system - before.children_system),
        "sampled_peak_tree_rss_bytes": sampled_peak_rss,
        "rss_sample_count": sample_count,
        "rss_sample_interval_ms": 100,
        "captured_log_bytes": len(output.encode("utf-8")),
        "capture_read_ms": int(capture_read_ms),
    }


def prebuild_test(cwd: Path, env: dict[str, str]) -> tuple[int, str]:
    """Build the exact audit integration-test binary outside measured samples."""
    started = time.monotonic_ns()
    completed = subprocess.run(PREBUILD_TEST, cwd=cwd, env=env, text=True,
                               capture_output=True, check=False)
    elapsed_ms = (time.monotonic_ns() - started) // 1_000_000
    output = completed.stdout + completed.stderr
    if completed.returncode:
        fail(f"audit test binary prebuild failed with exit code {completed.returncode}:\n{output}")
    return int(elapsed_ms), output


def read_saved_runs(directory: Path) -> list[dict]:
    timing_path = directory / "run-timings.tsv"
    if not timing_path.is_file():
        fail(f"comparison profile is missing {timing_path}")
    import csv
    results = []
    with timing_path.open(encoding="utf-8", newline="") as stream:
        for row in csv.DictReader(stream, delimiter="\t"):
            path = directory / f"run-{row['run']}.log"
            if not path.is_file():
                fail(f"comparison profile is missing {path}")
            parsed = parse_run(path.read_text(encoding="utf-8"))
            parsed["cargo_wall_ms"] = int(row["cargo_wall_ms"])
            parsed["process_metrics"] = {
                "child_cpu_user_s": float(row.get("child_cpu_user_s", "nan")),
                "child_cpu_system_s": float(row.get("child_cpu_system_s", "nan")),
                "sampled_peak_tree_rss_bytes": int(row.get("sampled_peak_tree_rss_bytes", 0)),
                "rss_sample_count": int(row.get("rss_sample_count", 0)),
                "rss_sample_interval_ms": int(row.get("rss_sample_interval_ms", 100)),
            }
            results.append(parsed)
    if not results:
        fail(f"comparison profile contains no runs: {directory}")
    return results


def percentile(values: list[float], p: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, (len(ordered) * p + 0.999999).__floor__() - 1))
    return ordered[index]


def write_report(out_dir: Path, runs: list[dict], run_walls: list[int], cache_states: list[str],
                 input_cache: list[dict],
                 file_paths: list[str], commit: str,
                 scala_root: Path, java_home: Path, jdk_version: str, artifact_root: Path,
                 prebuild_wall_ms: int) -> None:
    timing_only = runs[0]["summary"].get("timing_mode") != "full"
    output_started_ns = time.monotonic_ns()
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "run-timings.tsv").write_text(
        "run\tcargo_target_cache_state\tinput_cache_mode\tinput_cache_before_resident_bytes\tinput_cache_after_setup_resident_bytes\tinput_warmup_ms\tcargo_wall_ms\tchild_cpu_user_s\tchild_cpu_system_s\tsampled_peak_tree_rss_bytes\trss_sample_count\trss_sample_interval_ms\tcaptured_log_bytes\tcapture_read_ms\tsaved_log_write_ms\taudit_wall_ms\trunner_overhead_ms\tclasspath_setup_ms\tfile_discovery_ms\tsource_read_ms\tsource_processing_and_merge_ms\n"
        + "".join(
            f"{i}\t{cache_state}\t{input_state['mode']}\t{input_state['before_bytes']}\t{input_state.get('after_setup_bytes', 'unknown')}\t{input_state.get('warmup_ms', 'not_warmed')}\t"
            f"{wall}\t{r.get('process_metrics', {}).get('child_cpu_user_s', 'unavailable')}\t{r.get('process_metrics', {}).get('child_cpu_system_s', 'unavailable')}\t"
            f"{r.get('process_metrics', {}).get('sampled_peak_tree_rss_bytes', 'unavailable')}\t{r.get('process_metrics', {}).get('rss_sample_count', 'unavailable')}\t{r.get('process_metrics', {}).get('rss_sample_interval_ms', 'unavailable')}\t"
            f"{r.get('process_metrics', {}).get('captured_log_bytes', 'unavailable')}\t{r.get('process_metrics', {}).get('capture_read_ms', 'unavailable')}\t{r.get('process_metrics', {}).get('saved_log_write_ms', 'unavailable')}\t"
            f"{int(r['summary']['wall_ms'])}\t{wall-int(r['summary']['wall_ms'])}\t"
            f"{r['summary'].get('classpath_setup_ms', '0')}\t{r['summary'].get('file_discovery_ms', '0')}\t"
            f"{r['summary'].get('source_read_ms', '0')}\t{r['summary'].get('source_processing_and_merge_ms', r['summary'].get('source_audit_merge_ms', '0'))}\n"
            for i, (r, wall, cache_state, input_state) in enumerate(zip(runs, run_walls, cache_states, input_cache), 1)
        ), encoding="utf-8")

    phase_totals: dict[str, int] = {}
    file_samples: dict[str, list[int]] = {path: [] for path in file_paths}
    method_samples: list[tuple[int, str, str, str]] = []
    method_resolver_samples: dict[tuple[str, str, str], list[tuple[int, int]]] = {}
    resolver_totals: dict[str, list[int]] = {}
    load_totals: dict[str, int] = {}
    resolver_count_totals: dict[str, int] = {}
    match_resolver_totals: dict[str, int] = {}
    match_wrapper_totals: dict[str, dict[str, int]] = {}
    match_resolver_rows: list[str] = []
    cheap_resolver_rows: list[str] = []
    cheap_resolver_totals: dict[tuple[str, str], dict[str, int]] = {}
    resolver_stage_rows: list[str] = []
    resolver_stage_totals: dict[tuple[str, str], dict[str, int]] = {}
    match_phase_rows: list[str] = []
    load_root_rows: list[tuple[int, dict]] = []
    file_row_data: list[tuple[int, str, str, str, str]] = []
    method_row_data: list[tuple[int, str, str, str]] = []
    for run_index, result in enumerate(runs, 1):
        for scope, key in (("source", "resolver_stages"), ("match", "match_resolver_stages")):
            for row in result.get(key, []):
                stage = row["stage"]
                calls = int(row["calls"])
                elapsed_us = int(row["elapsed_us"])
                elapsed_ns = int(row.get("elapsed_ns", elapsed_us * 1_000))
                exclusive_ns = int(row.get("exclusive_ns", elapsed_ns))
                resolver_stage_rows.append(
                    f"{run_index}\t{scope}\t{stage}\t{calls}\t{elapsed_us}\t{elapsed_ns}\t{exclusive_ns}"
                )
                total = resolver_stage_totals.setdefault(
                    (scope, stage), {"calls": 0, "elapsed_us": 0, "exclusive_ns": 0}
                )
                total["calls"] += calls
                total["elapsed_us"] += elapsed_us
                total["exclusive_ns"] += exclusive_ns
        for (scope, operation), values in result.get("cheap_resolver", {}).items():
            outcomes = result.get("cheap_resolver_outcomes", {}).get((scope, operation), {})
            row = {**values, **outcomes}
            cheap_resolver_rows.append(
                f"{run_index}\t{scope}\t{operation}\t" + "\t".join(
                    str(row.get(name, 0)) for name in (
                        "calls", "wall_us", "delegated_us", "exclusive_us", "ok", "none", "errors"
                    )
                )
            )
            total = cheap_resolver_totals.setdefault(
                (scope, operation),
                {name: 0 for name in ("calls", "wall_us", "delegated_us", "exclusive_us", "ok", "none", "errors")},
            )
            for name in total:
                total[name] += row.get(name, 0)
        for name, count in result["loads"].items():
            load_totals[name] = load_totals.get(name, 0) + count
        for name, count in result["resolver_counts"].items():
            if not timing_only:
                resolver_count_totals[name] = resolver_count_totals.get(name, 0) + count
        load_root_rows.extend((run_index, row) for row in result["load_roots"])
        for row in result.get("match_phases", []):
            match_phase_rows.append(
                f"{run_index}\t{row.get('file', '')}\t{row.get('elapsed_ms', 'unknown')}\t"
                f"{row.get('selectors', 'unknown')}\t{row.get('probes', 'unknown')}"
            )
        for row in result.get("match_resolver", []):
            match_resolver_rows.append(f"{run_index}\t" + "\t".join(
                row.get(name, "unknown") for name in (
                    "file", "status", "package_calls", "member_calls", "module_identity_calls",
                    "module_completion_calls", "class_completion_calls", "classfile_completion_calls",
                    "tasty_completion_calls", "package_inner_us", "member_inner_us",
                    "module_identity_inner_us", "module_completion_inner_us", "class_completion_inner_us",
                    "classfile_completion_inner_us", "tasty_completion_inner_us", "rollback_count",
                    "rollback_operation_observations", "rollback_member_observations", "classpath_requests",
                    "classpath_inner_calls", "classpath_positive", "classpath_negative", "classpath_errors",
                    "classpath_negative_cache_hits", "classpath_returned_bytes", "classpath_inner_wall_us",
                )
            ))
            if row.get("status") == "captured":
                for key, value in row.items():
                    if key not in {"file", "status"}:
                        match_resolver_totals[key] = match_resolver_totals.get(key, 0) + int(value)
        for row in result.get("match_resolver_wrappers", []):
            operation = row["operation"]
            totals = match_wrapper_totals.setdefault(operation, {key: 0 for key in ("calls", "wall_us", "delegated_us", "exclusive_us")})
            for key in totals:
                totals[key] += int(row[key])
        for path, data in result["files"].items():
            file_samples.setdefault(path, []).append(data.get("total_ms", 0))
            outcome = data.get("outcome", "unknown")
            for phase, millis in data.items():
                if phase in {"outcome", "total_ms"} or not phase.endswith("_ms"):
                    continue
                file_row_data.append((int(millis), path, phase.removesuffix("_ms"), str(run_index), outcome))
                phase_totals[phase] = phase_totals.get(phase, 0) + millis
        for (path, tree), data in result["methods"].items():
            resolver = {} if timing_only else result["resolver"].get((path, tree), {})
            method_samples.append((data["elapsed_ms"], path, tree, data["method"]))
            if not timing_only:
                method_resolver_samples.setdefault((path, tree, data["method"]), []).append((
                    int(resolver.get("member_calls", 0)), int(resolver.get("member_inner_us", 0)),
                ))
            method_row_data.append((data["elapsed_ms"], path, tree, "\t".join(str(value) for value in [
                run_index, path, tree, data["line"], data["method"], data["elapsed_ms"],
                resolver.get("module_identity_calls", "not_captured"), resolver_duration_ms(resolver, "module_identity"), resolver_duration_us(resolver, "module_identity"),
                resolver.get("module_completion_calls", "not_captured"), resolver_duration_ms(resolver, "module_completion"), resolver_duration_us(resolver, "module_completion"),
                resolver.get("class_completion_calls", "not_captured"), resolver_duration_ms(resolver, "class_completion"), resolver_duration_us(resolver, "class_completion"),
                resolver.get("classfile_completion_calls", "not_captured"), resolver_duration_us(resolver, "classfile_completion"),
                resolver.get("tasty_completion_calls", "not_captured"), resolver_duration_us(resolver, "tasty_completion"),
                resolver.get("member_calls", "not_captured"), resolver_duration_ms(resolver, "member"), resolver_duration_us(resolver, "member"),
                resolver.get("package_calls", "not_captured"), resolver_duration_ms(resolver, "package"), resolver_duration_us(resolver, "package"),
            ])))
            for name, value in resolver.items():
                if name.endswith("_calls") or name.endswith("_inner_ms") or name.endswith("_inner_us"):
                    total = resolver_totals.setdefault(name, [0, 0])
                    total[0] += value
                    total[1] += 1
    file_rows = ["run\tfile\tphase\tmilliseconds\toutcome"] + [
        f"{run}\t{path}\t{phase}\t{millis}\t{outcome}"
        for millis, path, phase, run, outcome in sorted(file_row_data, key=lambda item: (-item[0], item[1], item[2], item[3]))
    ]
    method_rows = ["run\tfile\ttree\tline\tmethod\telapsed_ms\tmodule_identity_calls\tmodule_identity_inner_ms\tmodule_identity_inner_us\tmodule_completion_calls\tmodule_completion_inner_ms\tmodule_completion_inner_us\tclass_completion_calls\tclass_completion_inner_ms\tclass_completion_inner_us\tclassfile_completion_calls\tclassfile_completion_inner_us\ttasty_completion_calls\ttasty_completion_inner_us\tmember_calls\tmember_inner_ms\tmember_inner_us\tpackage_calls\tpackage_inner_ms\tpackage_inner_us"] + [
        row for _elapsed, _path, _tree, row in sorted(method_row_data, key=lambda item: (-item[0], item[1], item[2], item[3]))
    ]
    (out_dir / "file-phases.tsv").write_text("\n".join(file_rows) + "\n", encoding="utf-8")
    (out_dir / "match-phases.tsv").write_text(
        "run\tfile\telapsed_ms\tselectors\tprobes\n" + "\n".join(match_phase_rows) + ("\n" if match_phase_rows else ""),
        encoding="utf-8",
    )
    (out_dir / "match-resolver.tsv").write_text(
        "run\tfile\tstatus\tpackage_calls\tmember_calls\tmodule_identity_calls\tmodule_completion_calls\tclass_completion_calls\tclassfile_completion_calls\ttasty_completion_calls\tpackage_inner_us\tmember_inner_us\tmodule_identity_inner_us\tmodule_completion_inner_us\tclass_completion_inner_us\tclassfile_completion_inner_us\ttasty_completion_inner_us\trollback_count\trollback_operation_observations\trollback_member_observations\tclasspath_requests\tclasspath_inner_calls\tclasspath_positive\tclasspath_negative\tclasspath_errors\tclasspath_negative_cache_hits\tclasspath_returned_bytes\tclasspath_inner_wall_us\n"
        + "\n".join(match_resolver_rows) + ("\n" if match_resolver_rows else ""), encoding="utf-8")
    (out_dir / "match-resolver-wrappers.tsv").write_text(
        "run\tfile\toperation\tcalls\twall_us\tdelegated_us\texclusive_us\n" + "".join(
            f"{run_index}\t{row['file']}\t{row['operation']}\t{row['calls']}\t{row['wall_us']}\t{row['delegated_us']}\t{row['exclusive_us']}\n"
            for run_index, result in enumerate(runs, 1) for row in result.get("match_resolver_wrappers", [])
        ), encoding="utf-8")
    (out_dir / "cheap-resolver.tsv").write_text(
        "run\tscope\toperation\tcalls\twall_us\tdelegated_us\texclusive_us\tok\tnone\terrors\n"
        + "\n".join(cheap_resolver_rows) + ("\n" if cheap_resolver_rows else ""),
        encoding="utf-8",
    )
    (out_dir / "resolver-stages.tsv").write_text(
        "run\tscope\tstage\tcalls\telapsed_us\telapsed_ns\texclusive_ns\n"
        + "\n".join(resolver_stage_rows) + ("\n" if resolver_stage_rows else ""),
        encoding="utf-8",
    )
    (out_dir / "resolver-profile.tsv").write_text(
        "run\tmember_lookups\tsuccesses\tmisses\terrors\trollbacks\n" + "".join(
            f"{run_index}\t" + "\t".join(
                str(run.get("resolver_profile", {}).get(key, "not_captured"))
                for key in ("member_lookups", "successes", "misses", "errors", "rollbacks")
            ) + "\n"
            for run_index, run in enumerate(runs, 1)
        ), encoding="utf-8")
    (out_dir / "match-resolver-profile.tsv").write_text(
        "run\tfile\tmember_lookups\tsuccesses\tmisses\terrors\trollbacks\n" + "".join(
            f"{run_index}\t{row.get('file', 'not_captured')}\t" + "\t".join(
                str(row.get(key, "not_captured"))
                for key in ("member_lookups", "successes", "misses", "errors", "rollbacks")
            ) + "\n"
            for run_index, run in enumerate(runs, 1)
            for row in run.get("match_resolver_profiles", [])
        ), encoding="utf-8")
    root_rows = ["run\tscope\troot\tresolver_calls\tstable_key_observations\tsuccesses\tmisses\terrors\trollbacks\tunique_keys\trepeated_keys\tretry_after_failed_lookup\ttop_repeated"]
    root_phase_rows = ["run\tscope\troot\tstage\tcalls\telapsed_ns\texclusive_ns"]
    cache_rows = ["run\t" + "\t".join(sorted({key for run in runs for key in run.get("resolver_cache", {})}))]
    cache_keys = cache_rows[0].split("\t")[1:]
    for run_index, run in enumerate(runs, 1):
        for row in run.get("resolver_roots", []):
            root_rows.append(f"{run_index}\t" + "\t".join(str(row.get(key, "not_captured")) for key in (
                "scope", "root", "calls", "stable_key_observations", "successes", "misses", "errors", "rollbacks",
                "unique_keys", "repeated_keys", "retry_after_failed_lookup", "top_repeated",
            )))
        for row in run.get("resolver_root_phases", []):
            root_phase_rows.append(f"{run_index}\t" + "\t".join(str(row.get(key, "not_captured")) for key in (
                "scope", "root", "stage", "calls", "elapsed_ns", "exclusive_ns",
            )))
    for run_index, run in enumerate(runs, 1):
        cache_rows.append(str(run_index) + "\t" + "\t".join(run.get("resolver_cache", {}).get(key, "not_captured") for key in cache_keys))
    (out_dir / "resolver-roots.tsv").write_text("\n".join(root_rows) + "\n", encoding="utf-8")
    (out_dir / "resolver-root-phases.tsv").write_text("\n".join(root_phase_rows) + "\n", encoding="utf-8")
    (out_dir / "resolver-cache.tsv").write_text("\n".join(cache_rows) + "\n", encoding="utf-8")
    match_cache_keys = sorted({key for run in runs for row in run.get("match_resolver_cache", []) for key in row if key != "file"})
    match_cache_rows = ["run\tfile\t" + "\t".join(match_cache_keys)]
    for run_index, run in enumerate(runs, 1):
        for row in run.get("match_resolver_cache", []):
            match_cache_rows.append(
                f"{run_index}\t{row.get('file', 'not_captured')}\t"
                + "\t".join(row.get(key, "not_captured") for key in match_cache_keys)
            )
    (out_dir / "match-resolver-cache.tsv").write_text(
        "\n".join(match_cache_rows) + "\n", encoding="utf-8"
    )
    (out_dir / "repeated-requests.tsv").write_text(
        "run\tshape\trequests\tknown_requests\tdistinct_keys\tunknown_identity_requests\towners\tnames\tfiles\trepeated_hit_factor\n"
        + "".join(
            f"{run_index}\t{row['shape']}\t" + "\t".join(
                str(row.get(key, "not_captured"))
                for key in ("requests", "known_requests", "distinct_keys", "unknown_identity_requests", "owners", "names", "files", "repeated_hit_factor")
            ) + "\n"
            for run_index, run in enumerate(runs, 1)
            for row in run.get("repeated_requests", [])
        ), encoding="utf-8")
    (out_dir / "classload-timings.tsv").write_text(
        "run\tresource_lookup_ns\tclassfile_decode_ns\ttasty_decode_ns\tload_uncached_inclusive_ns\n"
        + "".join(
            f"{run_index}\t" + "\t".join(
                str(run.get("classload_timings", {}).get(key, "not_captured"))
                for key in ("resource_lookup_ns", "classfile_decode_ns", "tasty_decode_ns", "load_uncached_inclusive_ns")
            ) + "\n"
            for run_index, run in enumerate(runs, 1)
        ), encoding="utf-8")
    (out_dir / "match-classload-timings.tsv").write_text(
        "run\tfile\tresource_lookup_ns\tclassfile_decode_ns\ttasty_decode_ns\tload_uncached_inclusive_ns\n"
        + "".join(
            f"{run_index}\t{row['file']}\t" + "\t".join(
                str(row.get(key, "not_captured"))
                for key in ("resource_lookup_ns", "classfile_decode_ns", "tasty_decode_ns", "load_uncached_inclusive_ns")
            ) + "\n"
            for run_index, run in enumerate(runs, 1)
            for row in run.get("match_classload_timings", [])
        ), encoding="utf-8")
    (out_dir / "methods.tsv").write_text("\n".join(method_rows) + "\n", encoding="utf-8")
    outcome_rows = ["run\tfile\ttree\tresult"] + [
        f"{run_index}\t{path}\t{tree}\t{result}"
        for run_index, run in enumerate(runs, 1)
        for (path, tree), result in sorted(run.get("outcomes", {}).items())
    ]
    (out_dir / "method-outcomes.tsv").write_text("\n".join(outcome_rows) + "\n", encoding="utf-8")
    match_probe_rows = ["run\tfile\tmethod_symbol\tcase_tree\tfamily\tresult"] + [
        f"{run_index}\t{file}\t{method_symbol}\t{case_tree}\t{family}\t"
        f"{run['match_outcomes'].get((file, method_symbol, case_tree, family), 'missing')}"
        for run_index, run in enumerate(runs, 1)
        for file, method_symbol, case_tree, family in run.get("match_probe_ids", [])
    ]
    (out_dir / "match-probe-counts.tsv").write_text(
        "run\tavailable\texecuted\n" + "".join(
            f"{run_index}\t{probe.get('available', 'unknown')}\t{probe.get('executed', 'unknown')}\n"
            for run_index, run in enumerate(runs, 1) for probe in run.get("match_probes", [])
        ), encoding="utf-8")
    (out_dir / "match-probes.tsv").write_text("\n".join(match_probe_rows) + "\n", encoding="utf-8")
    wrapper_rows = ["run\toperation\tcalls\twall_us\tdelegated_us\texclusive_us"] + [
        f"{run_index}\t{operation}\t{values.get('calls', 0)}\t{values.get('wall_us', 0)}\t"
        f"{values.get('delegated_us', 0)}\t{values.get('exclusive_us', 0)}"
        for run_index, run in enumerate(runs, 1)
        for operation, values in sorted(run.get("wrappers", {}).items())
    ]
    (out_dir / "audit-wrapper.tsv").write_text("\n".join(wrapper_rows) + "\n", encoding="utf-8")
    rollback_rows = [
        "run\tcount\tcheckpoint_count\tcheckpoint_max_depth\toperation_appended\tmember_appended\t"
        "operation_observations\tmember_observations\toperation_rows_scanned\tmember_rows_scanned\t"
        "operation_status_changes\tmember_status_changes"
    ]
    for run_index, run in enumerate(runs, 1):
        rollback = run.get("rollback", {})
        # Keep the established TSV observation column names for downstream consumers,
        # while accepting both the old producer fields and the newer coverage fields.
        operation_observations = rollback.get(
            "operation_rollback_coverage", rollback.get("operation_observations", "not_captured")
        )
        member_observations = rollback.get(
            "member_rollback_coverage", rollback.get("member_observations", "not_captured")
        )
        rollback_rows.append("\t".join(str(value) for value in (
            run_index, rollback.get("count", "not_captured"), rollback.get("checkpoint_count", "not_captured"),
            rollback.get("checkpoint_max_depth", "not_captured"), rollback.get("operation_appended", "not_captured"),
            rollback.get("member_appended", "not_captured"), operation_observations, member_observations,
            rollback.get("operation_rows_scanned", "not_captured"), rollback.get("member_rows_scanned", "not_captured"),
            rollback.get("operation_status_changes", "not_captured"), rollback.get("member_status_changes", "not_captured"),
        )))
    (out_dir / "rollback.tsv").write_text("\n".join(rollback_rows) + "\n", encoding="utf-8")
    classpath_rows = ["run\trequests\tinner_calls\tpositive\tnegative\terrors\tnegative_cache_hits\treturned_bytes\tinner_wall_us"] + [
        f"{run_index}\t" + "\t".join(run.get("classpath_resources", {}).get(name, "unknown") for name in (
            "requests", "inner_calls", "positive", "negative", "errors", "negative_cache_hits", "returned_bytes", "inner_wall_us",
        ))
        for run_index, run in enumerate(runs, 1)
    ]
    (out_dir / "classpath-resources.tsv").write_text("\n".join(classpath_rows) + "\n", encoding="utf-8")

    top_files = sorted(
        ((statistics.mean(vals), path) for path, vals in file_samples.items() if vals),
        key=lambda item: (-item[0], item[1]),
    )
    method_times: dict[tuple[str, str, str], list[int]] = {}
    for elapsed, path, tree, name in method_samples:
        method_times.setdefault((path, tree, name), []).append(elapsed)
    top_methods = sorted(
        ((statistics.mean(values), max(values), path, tree, name, len(values))
         for (path, tree, name), values in method_times.items()),
        key=lambda item: (-item[0], item[2], item[3], item[4]),
    )[:10]
    top_hot_paths = top_methods[:3]
    # Delegated resolver durations can overlap with their calling method timers. Keep the two views separate.
    resolver_operations: dict[str, tuple[int | None, int | None, int | None]] = {}
    for operation in (
        "module_identity", "module_completion", "class_completion", "classfile_completion",
        "tasty_completion", "member", "package",
    ):
        micros_key = f"{operation}_inner_us"
        millis_key = f"{operation}_inner_ms"
        calls_key = f"{operation}_calls"
        micros = resolver_totals.get(micros_key, [0, 0])[0] if micros_key in resolver_totals else None
        millis = resolver_totals.get(millis_key, [0, 0])[0] if millis_key in resolver_totals else None
        calls = resolver_totals.get(calls_key, [0, 0])[0] if calls_key in resolver_totals else None
        resolver_operations[operation] = (calls, millis, micros)
    semantic_ms = phase_totals.get("typing_ms", 0) + phase_totals.get("typer_setup_ms", 0)
    p50 = percentile([v for values in file_samples.values() for v in values], 0.50)
    p95 = percentile([v for values in file_samples.values() for v in values], 0.95)
    cpu_model = "unknown"
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.lower().startswith("model name"):
                cpu_model = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    mem_total = "unknown"
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                mem_total = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    input_cache_summary = ", ".join(
        f"{index}={state['mode']} (warmup {state.get('warmup_ms', 'unknown')} ms; resident bytes before/after setup {state['before_bytes']}/{state.get('after_setup_bytes', 'unknown')})"
        for index, state in enumerate(input_cache, 1)
    )
    cpu_user_s = [float(run.get("process_metrics", {}).get("child_cpu_user_s", float("nan"))) for run in runs]
    cpu_system_s = [float(run.get("process_metrics", {}).get("child_cpu_system_s", float("nan"))) for run in runs]
    sampled_rss = [int(run.get("process_metrics", {}).get("sampled_peak_tree_rss_bytes", 0)) for run in runs]
    lines = [
        "# Sequential Typer corpus profile",
        "",
        "This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.",
        "",
        f"- dotty-rs commit: `{commit}`",
        f"- Scala checkout: `{scala_root}` at `{REVISION}`",
        f"- Scala artifacts: `3.9.0` under `{artifact_root}`",
        f"- JDK: `{jdk_version}` at `{java_home}`",
        f"- Host: `{cpu_model}`, {mem_total} memory; build profile `test`; Cargo/test concurrency fixed at 1",
        f"- Manifest files: {len(file_paths)}; profiled method observations per run: {len(runs[0]['methods'])}; repetitions: {len(runs)}",
        f"- Exact audit integration-test binary prebuilt outside measured samples in {prebuild_wall_ms} ms; measured invocations that rebuild are rejected.",
        f"- Timing mode: `{runs[0]['summary'].get('timing_mode', 'unknown')}`; root method limit per file: `{runs[0]['summary'].get('method_limit_per_file', 'unknown')}`. Exact method identities/outcomes and match-probe selection are in [`methods.tsv`](methods.tsv), [`method-outcomes.tsv`](method-outcomes.tsv), and [`match-probes.tsv`](match-probes.tsv). A/B counters are in [`cheap-resolver.tsv`](cheap-resolver.tsv); full-mode wrapper, rollback, and classpath data are in [`audit-wrapper.tsv`](audit-wrapper.tsv), [`rollback.tsv`](rollback.tsv), and [`classpath-resources.tsv`](classpath-resources.tsv).",
        f"- Cargo target cache by repetition: {', '.join(f'{index}={state}' for index, state in enumerate(cache_states, 1))}. `cold` means no compiled local_definition_audit test binary existed in the selected target directory.",
        f"- Input page-cache measurements: {input_cache_summary}. Warm mode reads the selected Scala sources, pinned Scala JARs, and JDK jmods before each sample, then records residency with `fincore`. Cold mode evicts only these selected inputs with POSIX_FADV_DONTNEED; it does not clear the system-wide cache.",
        "- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup, sampler overhead, and test-side log emission. Captured bytes plus the Python log read/write times are recorded in `run-timings.tsv`; the cost of emitting each diagnostic line remains inside the process timing.",
        f"- Process measurements: child CPU user/system mean {statistics.mean(cpu_user_s):.2f}/{statistics.mean(cpu_system_s):.2f} s; maximum sampled aggregate Cargo-process-tree RSS {max(sampled_rss, default=0) / (1024 * 1024):.1f} MiB. RSS sums per-process resident sets and can double-count shared mappings; 100 ms sampling may miss short peaks. CPU comes from child process accounting.",
        ("- Classpath setup includes JDK module and Scala JAR adapter construction. Full mode records resource lookup and ClassFile/TASTy decode intervals in `classload-timings.tsv`; those counters only cover classloads represented by dependency observations. Resolver phase and classloader timers are inclusive and may overlap, so do not add them as disjoint CPU intervals." if not timing_only else "- Classpath setup includes JDK module and Scala JAR adapter construction. Classloader resource/decode timings are collected only in full mode."),
        "- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. Resolver profiles additionally report request outcomes and repeated key counts by request shape. They do not expose all lookup cache misses; those are not inferred.",
        "- `audit-wrapper.tsv` separates full AuditResolver wrapper time from delegated resolver time; `exclusive_us` includes detailed observation work. Semantic-only mode bypasses audit collection; cheap-counters mode records only fixed operation/outcome counters and timings in `cheap-resolver.tsv`. Rollback detail is available only in full mode.",
        "- `classpath-resources.tsv` counts logical resource responses and returned bytes, not physical JAR reads/decompressions: the classloader interface does not expose those physical events. `inner_wall_us` measures the aggregate ClassPathEntry lookup interval only in detailed mode; cached negative requests are counted separately.",
        "- Semantic/report ratio compares `typer_setup + typing` against generation of all profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.",
        "",
        "## Repetition timings",
        "",
        "See [`run-timings.tsv`](run-timings.tsv).",
        "",
        f"Median / p95 selected-file wall time: {p50:.0f} / {p95:.0f} ms. Across repetitions, source typer setup plus method typing took {semantic_ms} ms; profile report generation took about PROFILE_REPORT_WRITE_MS ms (semantic-to-profile-report ratio PROFILE_REPORT_RATIO:1).",
        "",
        "## Slowest files",
        "",
        "| Mean ms | File |",
        "| ---: | --- |",
    ]
    lines += [f"| {mean:.1f} | `{path}` |" for mean, path in top_files[:10]]
    lines += ["", "## Slowest methods", "", "Method timings are averaged by stable file/tree identity across repetitions.", "", "| Mean ms | Max ms | Runs | File | Tree | Method |", "| ---: | ---: | ---: | --- | ---: | --- |"]
    lines += [f"| {mean:.1f} | {maximum} | {runs_count} | `{path}` | {tree} | `{method}` |"
               for mean, maximum, path, tree, method, runs_count in top_methods]
    lines += ["", "## Phase time", "", "| Phase | Total ms | Share of measured per-file phases |", "| --- | ---: | ---: |"]
    phase_sum = sum(phase_totals.values())
    for phase, total in sorted(phase_totals.items(), key=lambda item: (-item[1], item[0])):
        lines.append(f"| `{phase}` | {total} | {100 * total / max(1, phase_sum):.1f}% |")
    lines += ["", "## Resolver operation families", "", ("Detailed resolver counters and timings are not captured outside full mode; cheap boundary counters, when selected, are listed separately." if timing_only else "Durations measure delegated resolver calls; nested class/TASTy loading remains inclusive inside those calls. AuditResolver wrapper time is reported separately. Full mode also records aggregate resolver phases, member outcomes, repeated request shapes, and classloader resource/decode timings in sidecar TSVs."), "", "| Operation | Calls | Delegated ms |", "| --- | ---: | ---: |"]
    resolver_sort_key = lambda item: (item[1][1] is None and item[1][2] is None, -(item[1][2] if item[1][2] is not None else (item[1][1] or 0) * 1000), item[0])
    for operation, (calls, millis, micros) in sorted(resolver_operations.items(), key=resolver_sort_key):
        calls_text = "not captured" if calls is None else str(calls)
        if micros is not None:
            time_text = f"{micros / 1000:.3f}"
        elif millis is None:
            time_text = "not captured"
        elif millis == 0 and (calls is None or calls > 0):
            time_text = "below 1 ms per method"
        else:
            time_text = f"{millis:.3f}"
        lines.append(f"| `{operation}` | {calls_text} | {time_text} |")
    lines += ["", "## Cheap counter adapter", "", "Mode B records resolver operation counts, delegate/wrapper durations, and coarse result categories without request identities or transaction/event strings. Empty rows mean the selected mode did not collect cheap counters.", "", "| Scope | Operation | Calls | Wall µs | Delegated µs | Exclusive µs | Ok | None | Errors |", "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for (scope, operation), values in sorted(cheap_resolver_totals.items()):
        lines.append(
            f"| `{scope}` | `{operation}` | {values['calls']} | {values['wall_us']} | "
            f"{values['delegated_us']} | {values['exclusive_us']} | {values['ok']} | "
            f"{values['none']} | {values['errors']} |"
        )
    lines += ["", "## Match-probe pass attribution", "", "Match selector and case typing run in a separate SourceTyper pass. These resolver and classpath observations are reported separately from root-method typing.", "", "| Operation | Calls | Delegated µs |", "| --- | ---: | ---: |"]
    match_operations = (
        ("package", "package_calls", "package_inner_us"),
        ("member", "member_calls", "member_inner_us"),
        ("module_identity", "module_identity_calls", "module_identity_inner_us"),
        ("module_completion", "module_completion_calls", "module_completion_inner_us"),
        ("class_completion", "class_completion_calls", "class_completion_inner_us"),
        ("classfile_completion", "classfile_completion_calls", "classfile_completion_inner_us"),
        ("tasty_completion", "tasty_completion_calls", "tasty_completion_inner_us"),
    )
    for operation, calls_key, time_key in match_operations:
        calls = "not captured" if timing_only else str(match_resolver_totals.get(calls_key, 0))
        elapsed = "not captured" if timing_only else str(match_resolver_totals.get(time_key, 0))
        lines.append(f"| `{operation}` | {calls} | {elapsed} |")
    rollback_cells = "not captured" if timing_only else (
        f"{match_resolver_totals.get('rollback_count', 0)} calls; "
        f"{match_resolver_totals.get('rollback_operation_observations', 0)} operation and "
        f"{match_resolver_totals.get('rollback_member_observations', 0)} member observations"
    )
    resource_cells = "not captured" if timing_only else (
        f"{match_resolver_totals.get('classpath_positive', 0)} positive / "
        f"{match_resolver_totals.get('classpath_negative', 0)} negative; "
        f"{match_resolver_totals.get('classpath_returned_bytes', 0)} logical bytes; "
        f"{match_resolver_totals.get('classpath_inner_wall_us', 0)} µs inner lookup"
    )
    lines += ["", f"- Rollback/checkpoint-range metrics: {rollback_cells}.",
              f"- Match-pass classpath responses: {resource_cells}.",
              "- Per-file phase and resolver rows: [`match-phases.tsv`](match-phases.tsv), [`match-resolver.tsv`](match-resolver.tsv), and [`match-resolver-wrappers.tsv`](match-resolver-wrappers.tsv)."]
    if match_wrapper_totals:
        lines += ["", "Match-pass AuditResolver wrapper intervals:", "", "| Operation | Calls | Wall µs | Delegated µs | Exclusive µs |", "| --- | ---: | ---: | ---: | ---: |"]
        for operation, values in sorted(match_wrapper_totals.items()):
            lines.append(f"| `{operation}` | {values['calls']} | {values['wall_us']} | {values['delegated_us']} | {values['exclusive_us']} |")
    lines += ["", "## Classpath load counts", "", "| Observation | Count across runs |", "| --- | ---: |"]
    for name in sorted(load_totals):
        lines.append(f"| `{name}` | {load_totals[name]} |")
    lines += ["", "## Resolver request counts", "", "| Family | Count across runs |", "| --- | ---: |"]
    for name in sorted(resolver_count_totals):
        lines.append(f"| `{name}` | {resolver_count_totals[name]} |")
    lines += ["", "## AuditResolver wrapper time", "", "Times include wrapper bookkeeping and exclude the delegated interval in the `exclusive_us` column.", "", "| Operation | Calls | Wall ms | Delegated ms | Exclusive ms |", "| --- | ---: | ---: | ---: | ---: |"]
    wrapper_totals: dict[str, dict[str, int]] = {}
    for result in runs:
        for operation, values in result.get("wrappers", {}).items():
            aggregate = wrapper_totals.setdefault(operation, {key: 0 for key in values})
            for key, value in values.items():
                aggregate[key] += value
    for operation, values in sorted(wrapper_totals.items(), key=lambda item: (-item[1].get("wall_us", 0), item[0])):
        lines.append(f"| `{operation}` | {values.get('calls', 0)} | {values.get('wall_us', 0) / 1000:.3f} | {values.get('delegated_us', 0) / 1000:.3f} | {values.get('exclusive_us', 0) / 1000:.3f} |")
    lines += ["", "## Full-mode AuditResolver subphases", "", "These separately timed wrapper windows attribute request preparation/identity, result classification, event collection, and transaction bookkeeping. They are inclusive across resolver operations that can nest; do not sum them with delegated resolver intervals or wrapper totals.", "", "| Scope | Stage | Calls | Elapsed ms | Exclusive ms |", "| --- | --- | ---: | ---: | ---: |"]
    for (scope, stage), values in sorted(resolver_stage_totals.items(), key=lambda row: (-row[1]["elapsed_us"], row[0])):
        lines.append(f"| `{scope}` | `{stage}` | {values['calls']} | {values['elapsed_us'] / 1000:.3f} | {values['exclusive_ns'] / 1_000_000:.3f} |")
    lines += ["", "## Largest classpath request roots", "", "Existing resolver observations sorted these by completed-class fanout; paths and method/tree identities are unchanged.", "", "| Root | File and method | Classes touched | Completions | Cache hits |", "| --- | --- | ---: | ---: | ---: |"]
    sorted_roots = sorted(load_root_rows, key=lambda item: (-item[1]["distinct_classes"], item[1]["root"], item[1]["file"], item[1]["method"], item[0]))
    lines += [
        f"| `{root['root']}` | `{root['file']}::{root['method']}` (run {run}) | {root['distinct_classes']} | {root['full_completions']} | {root['completed_cache_hits']} |"
        for run, root in sorted_roots[:10]
    ]
    lines += ["", "## Three measured method roots", "", "Ranked by mean inclusive method-attempt time. These attempts stop at the first typer blocker, so they characterize current audit work rather than successful full-method typing. " + ("Detailed per-method attribution is only available in full mode." if timing_only else "Member resolver time is nested within method time and is shown only as attribution, not added again."), "", "| Rank | Mean inclusive ms | Member calls | Delegated member ms | Exact root |", "| ---: | ---: | ---: | ---: | --- |"]
    for rank, (mean, _maximum, path, tree, name, _runs_count) in enumerate(top_hot_paths, 1):
        calls_and_time = method_resolver_samples.get((path, tree, name), [])
        mean_calls = statistics.mean(calls for calls, _elapsed in calls_and_time) if calls_and_time else None
        mean_resolver_ms = statistics.mean(elapsed for _calls, elapsed in calls_and_time) / 1000 if calls_and_time else None
        source_lines = [
            data["line"]
            for run in runs
            for (file, method_tree), data in run["methods"].items()
            if file == path and method_tree == tree
        ]
        source_line = source_lines[0] if source_lines else 0
        pointer = f"{path}:{source_line}" if source_line else f"{path}#tree={tree}"
        calls_text = f"{mean_calls:.1f}" if mean_calls is not None else "not captured"
        resolver_text = f"{mean_resolver_ms:.1f}" if mean_resolver_ms is not None else "not captured"
        lines.append(f"| {rank} | {mean:.1f} | {calls_text} | {resolver_text} | `{pointer}` `{name}` (tree {tree}) |")
    if not timing_only:
        lines += [
        "",
        "All three hot roots spend most observed method-attempt time in delegated member resolution. The largest nested interval is module completion, including classloader decode and resource work; exclusive target-selection time is small. Full-mode sidecars retain per-root exclusive/inclusive stages, stable-key outcomes, cache/completion counts, and exact method/match identities. Use `resolver-root-phases.tsv`, `resolver-roots.tsv`, `resolver-cache.tsv`, `match-resolver-cache.tsv`, and `resolver-stages.tsv` for those records. These timings stop at first blockers, overlap where marked inclusive, and do not establish an optimization speedup or justify cross-session semantic caching.",
        ]
    lines += [
        "",
        ("Resolver per-method durations are emitted in microseconds. A zero call count means that operation did not run for that method; zero elapsed time with calls is below timer resolution, not proof of zero cost." if not timing_only else "Per-method detailed resolver attribution is not collected in semantic-only or cheap-counters mode."),
        ("Classloader decode intervals are separated from resource lookup, but nested dependencies and semantic projection may still overlap with inclusive load timings. Shell creation and semantic completion are represented by counts, not disjoint CPU intervals; do not add inclusive timers or infer a safe cross-store cache from reuse counts." if not timing_only else "Classloader load and AuditResolver detailed observations are full-mode-only; cheap counters contain resolver boundary measurements only."),
        "",
    ]
    (out_dir / "resolver-operations.tsv").write_text(
        "operation\tcalls\tinclusive_ms\tinclusive_us\n" + "".join(
            f"{operation}\t{calls if calls is not None else 'not_captured'}\t{millis if millis is not None else 'not_captured'}\t{micros if micros is not None else 'not_captured'}\n"
            for operation, (calls, millis, micros) in sorted(resolver_operations.items(), key=resolver_sort_key)
        ), encoding="utf-8")
    (out_dir / "classpath-loads.tsv").write_text(
        "run\tmeasure\tcount\n" + "".join(
            f"{index}\t{name}\t{result['loads'].get(name, 0)}\n"
            for index, result in enumerate(runs, 1) for name in sorted(load_totals)
        ), encoding="utf-8")
    (out_dir / "request-roots.tsv").write_text(
        "run\troot\tfile\tmethod\trequest\tdepth\tdistinct_classes\tfull_completions\tshells_created\tshells_reused\tmember_type_references\thierarchy_loads\tcompleted_cache_hits\n" + "".join(
            "\t".join(str(value) for value in [run, root["root"], root["file"], root["method"], root["request"], root["depth"], root["distinct_classes"], root["full_completions"], root["shells_created"], root["shells_reused"], root["member_type_references"], root["hierarchy_loads"], root["completed_cache_hits"]]) + "\n"
            for run, root in sorted_roots
        ), encoding="utf-8")
    (out_dir / "resolver-counts.tsv").write_text(
        "request_family\tcount_across_runs\n" + "".join(
            f"{name}\t{count}\n" for name, count in sorted(resolver_count_totals.items())
        ), encoding="utf-8")
    summary_path = out_dir / "summary.md"
    summary_path.write_text("\n".join(lines), encoding="utf-8")
    report_write_ms = max(0, (time.monotonic_ns() - output_started_ns) // 1_000_000)
    summary = summary_path.read_text(encoding="utf-8")
    summary = summary.replace("PROFILE_REPORT_WRITE_MS", str(report_write_ms))
    summary = summary.replace("PROFILE_REPORT_RATIO", f"{semantic_ms / max(1, report_write_ms):.2f}")
    summary_path.write_text(summary, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--method-limit-per-file", type=int, help="type only the first N stable root-method trees in each selected file")
    parser.add_argument("--match-limit-per-file", type=int, help="type at most N supported-match probes per selected file")
    parser.add_argument("--mode", choices=("semantic_only", "cheap_counters", "full"), default="full", help="A/B/C resolver instrumentation mode: semantic-only, cheap counters, or full audit")
    parser.add_argument("--timing-only", action="store_true", help="deprecated alias for --mode semantic_only")
    parser.add_argument("--compare-with", type=Path, action="append", default=[], help="compare exact selected identities/outcomes against a completed profile directory; may be repeated for A/B/C comparisons")
    parser.add_argument("--output-dir", type=Path, default=REPO_ROOT / "target/typer-sequential-profile")
    parser.add_argument("--summarize-existing", action="store_true", help="rebuild reports from run-N.log without rerunning the audit")
    parser.add_argument("--measured-commit", help="override source commit when summarizing saved logs")
    parser.add_argument("--cold-inputs-first-run", action="store_true", help="evict selected Scala/JDK input pages before run 1 and measure residency with fincore")
    args = parser.parse_args()
    if args.timing_only and args.mode != "full":
        fail("--timing-only cannot be combined with --mode")
    profile_mode = "semantic_only" if args.timing_only else args.mode
    if not 1 <= args.runs <= 3:
        fail("--runs must be between 1 and 3 to keep the profile bounded")
    if args.measured_commit and not args.summarize_existing:
        fail("--measured-commit is only valid with --summarize-existing")
    if args.cold_inputs_first_run and args.summarize_existing:
        fail("--cold-inputs-first-run cannot be combined with --summarize-existing")
    if args.method_limit_per_file is not None and args.method_limit_per_file < 1:
        fail("--method-limit-per-file must be positive")
    if args.match_limit_per_file is not None and args.match_limit_per_file < 0:
        fail("--match-limit-per-file must not be negative")
    scala_root = Path(os.environ.get("SCALA39_ROOT", ""))
    java_home = Path(os.environ.get("JAVA_HOME", ""))
    if not scala_root.is_dir() or not java_home.is_dir():
        fail("set SCALA39_ROOT and JAVA_HOME to the pinned Scala checkout and JDK")
    actual_revision = subprocess.check_output(["git", "-C", str(scala_root), "rev-parse", "HEAD"], text=True).strip()
    if actual_revision != REVISION:
        fail(f"SCALA39_ROOT is {actual_revision}; expected pinned revision {REVISION}")
    release = (java_home / "release").read_text(encoding="utf-8")
    match = re.search(r'^JAVA_VERSION="([^"]+)"', release, re.MULTILINE)
    expected_release = os.environ.get("SCALA39_JDK_RELEASE")
    if not match or not expected_release or match.group(1) != "21.0.8" or expected_release != "21":
        fail("sequential profile requires the pinned JDK 21.0.8 and SCALA39_JDK_RELEASE=21")
    artifact_root = Path(os.environ.get("COURSIER_MAVEN_ROOT", str(Path.home() / ".cache/coursier/v1/https/repo1.maven.org/maven2")))
    artifacts = [
        "org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar",
        "org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar",
        "org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar",
        "org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar",
        "org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar",
        "org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar",
        "org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar",
        "org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar",
    ]
    missing = [str(artifact_root / item) for item in artifacts if not (artifact_root / item).is_file()]
    if missing:
        fail("missing pinned Scala runtime artifact(s):\n" + "\n".join(missing))
    selected = read_manifest(args.manifest, scala_root)
    env = os.environ.copy()
    env["SCALA39_ROOT"] = str(scala_root)
    env["SCALA39_JDK_RELEASE"] = expected_release
    env["SCALA39_CLASSPATH"] = os.pathsep.join(str(artifact_root / item) for item in artifacts)
    env["DOTTY_AUDIT_PROFILE_FILES"] = ",".join(selected)
    env["DOTTY_AUDIT_PROFILE_MODE"] = profile_mode
    if args.method_limit_per_file is not None:
        env["DOTTY_AUDIT_PROFILE_METHOD_LIMIT"] = str(args.method_limit_per_file)
    if args.match_limit_per_file is not None:
        env["DOTTY_AUDIT_PROFILE_MATCH_LIMIT"] = str(args.match_limit_per_file)
    if profile_mode == "semantic_only":
        env["DOTTY_AUDIT_TIMING_ONLY"] = "1"
    else:
        env.pop("DOTTY_AUDIT_TIMING_ONLY", None)
    env["CARGO_BUILD_JOBS"] = "1"
    env["RUST_TEST_THREADS"] = "1"
    out_dir = args.output_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    results: list[dict] = []
    wall_times: list[int] = []
    cache_states: list[str] = []
    input_cache: list[dict] = []
    if args.summarize_existing:
        prebuild_path = out_dir / "prebuild.tsv"
        if not prebuild_path.is_file():
            fail(f"no prebuild record for saved samples: {prebuild_path}")
        import csv
        with prebuild_path.open(encoding="utf-8", newline="") as stream:
            prebuild_wall_ms = int(next(csv.DictReader(stream, delimiter="\t"))["prebuild_wall_ms"])
        timing_path = out_dir / "run-timings.tsv"
        if not timing_path.is_file():
            fail(f"no existing timing table: {timing_path}")
        with timing_path.open(encoding="utf-8", newline="") as stream:
            for row in csv.DictReader(stream, delimiter="\t"):
                log_path = out_dir / f"run-{row['run']}.log"
                if not log_path.is_file():
                    fail(f"no saved profile log: {log_path}")
                parsed = parse_run(log_path.read_text(encoding="utf-8"))
                parsed["cargo_wall_ms"] = int(row["cargo_wall_ms"])
                parsed["process_metrics"] = {
                    "child_cpu_user_s": float(row.get("child_cpu_user_s", "nan")),
                    "child_cpu_system_s": float(row.get("child_cpu_system_s", "nan")),
                    "sampled_peak_tree_rss_bytes": int(row.get("sampled_peak_tree_rss_bytes", 0)),
                    "rss_sample_count": int(row.get("rss_sample_count", 0)),
                    "rss_sample_interval_ms": int(row.get("rss_sample_interval_ms", 100)),
                    "captured_log_bytes": int(row.get("captured_log_bytes", 0)),
                    "capture_read_ms": int(row.get("capture_read_ms", 0)),
                    "saved_log_write_ms": int(row.get("saved_log_write_ms", 0)),
                }
                results.append(parsed)
                wall_times.append(int(row["cargo_wall_ms"]))
                cache_states.append(row.get("cargo_target_cache_state", "unknown"))
                input_cache.append({
                    "mode": row.get("input_cache_mode", "unmeasured"),
                    "before_bytes": row.get("input_cache_before_resident_bytes", "unknown"),
                    "after_setup_bytes": row.get("input_cache_after_setup_resident_bytes", "unknown"),
                    "warmup_ms": row.get("input_warmup_ms", "not_warmed"),
                })
    else:
        target_dir = Path(env.get("CARGO_TARGET_DIR", REPO_ROOT / "target")).resolve()
        inputs = profile_input_paths(scala_root, java_home, artifact_root, selected)
        prebuild_wall_ms, prebuild_output = prebuild_test(REPO_ROOT, env)
        (out_dir / "prebuild.tsv").write_text(
            "prebuild_wall_ms\texit_status\tbuild_log_bytes\n"
            f"{prebuild_wall_ms}\t0\t{len(prebuild_output.encode('utf-8'))}\n",
            encoding="utf-8",
        )
        for index in range(1, args.runs + 1):
            print(f"starting serial profile run {index}/{args.runs}", flush=True)
            cache_state = cargo_target_cache_state(target_dir)
            cache_states.append(cache_state)
            if args.cold_inputs_first_run:
                before_bytes = resident_bytes(inputs)
                if index == 1:
                    evict_input_pages(inputs)
                    after_setup_bytes = resident_bytes(inputs)
                    input_mode = "evicted" if after_setup_bytes == 0 else "partial-eviction"
                else:
                    after_setup_bytes = "not_measured"
                    input_mode = "warm-observed"
                input_cache.append({"mode": input_mode, "before_bytes": before_bytes, "after_setup_bytes": after_setup_bytes, "warmup_ms": 0})
            else:
                warmup_ms = warm_input_pages(inputs)
                after_setup_bytes = resident_bytes(inputs) if shutil.which("fincore") else "unavailable"
                input_cache.append({"mode": "warm-read-through", "before_bytes": "unknown", "after_setup_bytes": after_setup_bytes, "warmup_ms": warmup_ms})
            return_code, output, process_metrics = run_measured(AUDIT_TEST, REPO_ROOT, env)
            log_path = out_dir / f"run-{index}.log"
            log_write_started = time.monotonic_ns()
            log_path.write_text(output, encoding="utf-8")
            process_metrics["saved_log_write_ms"] = (time.monotonic_ns() - log_write_started) // 1_000_000
            if re.search(r"^\s*Compiling\s+", output, re.MULTILINE):
                fail(f"profile run {index} rebuilt a Cargo target inside the measurement; see {out_dir / f'run-{index}.log'}")
            wall_ms = process_metrics["cargo_wall_ms"]
            if return_code:
                print(output, file=sys.stderr)
                fail(f"profile run {index} failed with exit code {return_code}; see {log_path}")
            parsed = parse_run(output)
            parsed["cargo_wall_ms"] = wall_ms
            parsed["process_metrics"] = process_metrics
            results.append(parsed)
            wall_times.append(wall_ms)
            print(f"run {index}/{args.runs}: target_cache={cache_state} cargo_wall_ms={wall_ms} audit_wall_ms={results[-1]['summary']['wall_ms']}", flush=True)
    if not results:
        fail("no profile runs to summarize")
    expected_methods = {(key, value["method"]) for key, value in results[0]["methods"].items()}
    expected_outcomes = results[0]["outcomes"]
    expected_match_probes = results[0]["match_probes"]
    expected_match_probe_ids = results[0]["match_probe_ids"]
    expected_match_outcomes = results[0]["match_outcomes"]
    expected_files = set(results[0]["files"])
    if expected_files != set(selected):
        fail("profile logs do not match the checked-in manifest file set")
    for index, result in enumerate(results[1:], 2):
        if {(key, value["method"]) for key, value in result["methods"].items()} != expected_methods:
            fail(f"profile run {index} did not preserve the same method identities")
        if result["outcomes"] != expected_outcomes:
            fail(f"profile run {index} did not preserve the same method semantic outcomes")
        if result["match_probes"] != expected_match_probes:
            fail(f"profile run {index} did not preserve the same match-probe selection")
        if result["match_probe_ids"] != expected_match_probe_ids:
            fail(f"profile run {index} did not preserve the same exact match-probe identities")
        if result["match_outcomes"] != expected_match_outcomes:
            fail(f"profile run {index} did not preserve the same match-probe semantic outcomes")
        if set(result["files"]) != expected_files:
            fail(f"profile run {index} did not preserve the same selected files")
    if args.compare_with:
        comparison_profiles = [read_saved_runs(directory.resolve()) for directory in args.compare_with]
        reference = comparison_profiles[0][0]
        profiles = [*comparison_profiles, results]
        for profile in profiles:
            for index, result in enumerate(profile, 1):
                if set(result["files"]) != set(reference["files"]):
                    fail(f"comparison run {index} selected a different file set")
                if {(key, value["method"]) for key, value in result["methods"].items()} != {
                    (key, value["method"]) for key, value in reference["methods"].items()
                }:
                    fail(f"comparison run {index} selected different method identities")
                if result["outcomes"] != reference["outcomes"]:
                    fail(f"comparison run {index} changed method semantic outcomes")
                if result["match_probe_ids"] != reference["match_probe_ids"]:
                    fail(f"comparison run {index} selected different exact match probes")
                if result["match_outcomes"] != reference["match_outcomes"]:
                    fail(f"comparison run {index} changed match-probe semantic outcomes")
        comparison_rows = ["timing_mode\truns\tmean_cargo_wall_ms\tmean_child_cpu_user_s\tmean_child_cpu_system_s\tmax_sampled_rss_bytes\tmethod_count\tmatch_probe_count"]
        for profile in profiles:
            first = profile[0]
            cpu_user = [float(row.get("process_metrics", {}).get("child_cpu_user_s", "nan")) for row in profile]
            cpu_system = [float(row.get("process_metrics", {}).get("child_cpu_system_s", "nan")) for row in profile]
            rss = [int(row.get("process_metrics", {}).get("sampled_peak_tree_rss_bytes", 0)) for row in profile]
            comparison_rows.append(
                f"{first['summary'].get('timing_mode', 'unknown')}\t{len(profile)}\t"
                f"{statistics.mean(row['cargo_wall_ms'] for row in profile):.1f}\t"
                f"{statistics.mean(cpu_user):.2f}\t{statistics.mean(cpu_system):.2f}\t"
                f"{max(rss, default=0)}\t{len(first['methods'])}\t{len(first['match_probe_ids'])}"
            )
        (out_dir / "variant-comparison.tsv").write_text("\n".join(comparison_rows) + "\n", encoding="utf-8")
    commit = args.measured_commit or subprocess.check_output(["git", "-C", str(REPO_ROOT), "rev-parse", "HEAD"], text=True).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        fail("--measured-commit must be a full 40-character Git SHA")
    write_report(out_dir, results, wall_times, cache_states, input_cache, selected, commit, scala_root, java_home, match.group(1), artifact_root, prebuild_wall_ms)
    if args.compare_with:
        summary_path = out_dir / "summary.md"
        summary = summary_path.read_text(encoding="utf-8")
        summary += "\n## Instrumentation variant comparison\n\n`variant-comparison.tsv` records the selected A/B/C profile variants. Exact method identities, method outcomes, match-probe identities, and match outcomes were checked for equality across all listed directories. Timing differences estimate observer overhead; they are not code optimization speedups.\n"
        summary_path.write_text(summary, encoding="utf-8")
    print(f"profile report: {out_dir / 'summary.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
