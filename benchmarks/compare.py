#!/usr/bin/env python3
"""Compare the local original Python program with optimized Rust binaries."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import re
import statistics
import subprocess
import sys
from datetime import datetime, timezone
from time import perf_counter_ns

ROOT = Path(__file__).resolve().parents[1]
MARK = "\u200b" * 10


def run(argv, env, cwd, **kwargs):
    return subprocess.run(
        argv, env=env, cwd=cwd, capture_output=True, text=True,
        check=True, timeout=120, **kwargs,
    )


def summarize(values):
    ordered = sorted(values)
    return {
        "median": statistics.median(values),
        "mean": statistics.mean(values),
        "p95": ordered[min(len(ordered) - 1, int(0.95 * len(ordered)))],
        "min": min(values), "max": max(values), "samples": values,
    }


def source_hash():
    digest = hashlib.sha256()
    for path in sorted((ROOT / "thefuck/thefuck").rglob("*.py")):
        digest.update(str(path.relative_to(ROOT)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", type=Path, default=ROOT / "target/bench-python/bin/python")
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--engine-samples", type=int, default=15)
    parser.add_argument("--output", type=Path, default=ROOT / "benchmarks/results.json")
    args = parser.parse_args()
    if min(args.samples, args.warmups, args.engine_samples) < 1:
        parser.error("sample counts and warmups must be positive")
    python = str(args.python.absolute())
    rust = str(ROOT / "target/release/notypo")
    work = ROOT / "target/benchmark-work"
    work.mkdir(parents=True, exist_ok=True)
    if (Path.home() / ".thefuck").is_dir():
        raise SystemExit("Legacy ~/.thefuck overrides XDG_CONFIG_HOME; use an isolated account for this benchmark.")
    env = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("THEFUCK_", "TF_", "NOTYPO_"))
        and key not in ("SHELL_LOGGER_SOCKET", "PYTHONPATH", "PYTHONHOME", "PYTHONWARNINGS")
    }
    (work / "history").write_text("git status\n")
    env.update({
        "PYTHONPATH": str(ROOT / "thefuck"), "PYTHONWARNINGS": "ignore",
        "TF_SHELL": "bash", "TF_ALIAS": "fuck", "TF_SHELL_ALIASES": "",
        "THEFUCK_RULES": "DEFAULT_RULES", "THEFUCK_ALTER_HISTORY": "false",
        "THEFUCK_NO_COLORS": "true", "THEFUCK_DEBUG": "false",
        "THEFUCK_REQUIRE_CONFIRMATION": "false",
        "THEFUCK_INSTANT_MODE": "false", "HISTFILE": str(work / "history"),
        "XDG_CONFIG_HOME": str(work / "config"), "XDG_CACHE_HOME": str(work / "cache"),
        "PS1": MARK + "$ ", "COLUMNS": "80", "LINES": "24",
    })
    fixtures = json.loads((ROOT / "benchmarks/fixtures.json").read_text())
    # A repository with no remotes: rerunning git sttus cannot contact a server.
    run(["git", "init", "--quiet"], env, work)
    implementations = {
        "python": [python, str(ROOT / "benchmarks/python_cli.py")], "rust": [rust],
    }
    cases = [{"name": "help_startup", "args": ["--help"], "env": env, "expected": None}]
    for fixture in fixtures:
        log = work / (fixture["name"] + ".log")
        data = (MARK + "$ " + fixture["script"] + "\r\n"
                + fixture["output"].replace("\n", "\r\n")
                + MARK + "$ fuck\r\n").encode()
        log.write_bytes(data.ljust(1024 * 1024, b"\0"))
        captured = dict(env, TF_HISTORY=fixture["script"] + "\nfuck",
                        THEFUCK_INSTANT_MODE="true", THEFUCK_OUTPUT_LOG=str(log))
        cases.append({"name": fixture["name"] + "_captured", "args": ["-y"],
                      "env": captured, "expected": fixture["expected"]})
        # Permission-denied output is simulated only; other reruns are harmless
        # failures and the suggested correction is printed, never evaluated.
        if fixture["name"] != "sudo":
            cases.append({"name": fixture["name"] + "_rerun", "args": ["-y"],
                          "env": dict(env, TF_HISTORY=fixture["script"] + "\nfuck"),
                          "expected": fixture["expected"]})
    fuzzy = next(case for case in cases if case["name"] == "no_command_rerun")
    cases.append(dict(fuzzy, name="no_command_rerun_no_path_cache",
                      env=dict(fuzzy["env"], NOTYPO_NO_CACHE="1")))

    def invoke(case, implementation):
        start = perf_counter_ns()
        process = run(implementations[implementation] + case["args"], case["env"], work)
        elapsed = perf_counter_ns() - start
        if case["expected"] is not None and process.stdout.strip() != case["expected"]:
            raise RuntimeError(f"{implementation} {case['name']}: unexpected correction {process.stdout!r}; {process.stderr}")
        if "[WARN]" in process.stderr:
            raise RuntimeError(f"{implementation} {case['name']}: {process.stderr}")
        return elapsed / 1e6

    print("Checking corrections and warming process/filesystem caches...", flush=True)
    for case in cases:
        for implementation in implementations:
            for _ in range(args.warmups):
                invoke(case, implementation)
        print("  verified " + case["name"], flush=True)
    timings = {case["name"]: {name: [] for name in implementations} for case in cases}
    rng = random.Random(20261004)
    print(f"Measuring {args.samples} fresh processes per implementation per case...", flush=True)
    for sample in range(args.samples):
        ordered = cases.copy()
        rng.shuffle(ordered)
        for case in ordered:
            names = list(implementations)
            rng.shuffle(names)
            for implementation in names:
                timings[case["name"]][implementation].append(invoke(case, implementation))
        if (sample + 1) % 10 == 0:
            print(f"  completed {sample + 1}/{args.samples} rounds", flush=True)
    cli = {}
    for case in cases:
        measurements = {name: summarize(values) for name, values in timings[case["name"]].items()}
        measurements["speedup"] = measurements["python"]["median"] / measurements["rust"]["median"]
        measurements["correction"] = case["expected"]
        cli[case["name"]] = measurements

    # Locate Cargo's bench executable via build messages, avoiding stale hashes.
    build = run(["cargo", "build", "--release", "--bench", "correction", "--message-format=json"], env, ROOT)
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith("{")]
    engine_binary = next(item["executable"] for item in artifacts
                         if item.get("reason") == "compiler-artifact"
                         and item["target"]["name"] == "correction" and item.get("executable"))
    engine_commands = {"python": [python, str(ROOT / "benchmarks/python_engine.py")], "rust": [engine_binary]}
    executables = ["git", "gzip", "grep", "go", "python", "mkdir", "ls"]
    executables += [f"benchmark_tool_{i:04}" for i in range(2000 - len(executables))]
    engine = {}
    print("Measuring warm correction engines (five rules, 2,000 candidates)...", flush=True)
    for fixture in fixtures:
        measurements = {}
        names = list(implementations)
        rng.shuffle(names)
        for name in names:
            request = {"fixture": fixture, "executables": executables, "iterations": 100, "samples": 1}
            calibration = json.loads(run(engine_commands[name], env, work, input=json.dumps(request)).stdout)
            # Aim for 30 ms per batch; cap very fast cases to bound total work.
            iterations = max(100, min(200_000, int(30_000_000 / calibration["ns_per_correction"][0])))
            request.update(iterations=iterations, samples=args.engine_samples)
            measurement = json.loads(run(engine_commands[name], env, work, input=json.dumps(request)).stdout)
            measurements[name] = summarize([ns / 1000 for ns in measurement["ns_per_correction"]])
            measurements[name]["iterations_per_sample"] = iterations
        measurements["speedup"] = measurements["python"]["median"] / measurements["rust"]["median"]
        measurements["correction"] = fixture["expected"]
        engine[fixture["name"]] = measurements
        print(f"  {fixture['name']}: {measurements['speedup']:.1f}x", flush=True)

    memory = {}
    if platform.system() == "Darwin":
        case = next(case for case in cases if case["name"] == "cd_parent_rerun")
        for name, argv in implementations.items():
            values = []
            for _ in range(5):
                measured = run(["/usr/bin/time", "-l"] + argv + case["args"], case["env"], work)
                values.append(int(re.search(r"(\d+)\s+maximum resident set size", measured.stderr)[1]) / 1024**2)
            memory[name] = summarize(values)
    metadata = {
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(),
        "python": run([python, "--version"], env, work).stdout.strip(),
        "rustc": run(["rustc", "--version"], env, work).stdout.strip(),
        "python_dependencies": json.loads(run([python, "-m", "pip", "list", "--format=json"], env, work).stdout),
        "cpu": run(["sysctl", "-n", "machdep.cpu.brand_string"], env, work).stdout.strip() if platform.system() == "Darwin" else platform.processor(),
        "python_source_sha256": source_hash(),
        "rust_binary_sha256": hashlib.sha256(Path(rust).read_bytes()).hexdigest(),
        "rust_binary_bytes": Path(rust).stat().st_size,
        "path": env["PATH"], "cli_samples": args.samples, "warmups": args.warmups,
        "engine_samples": args.engine_samples, "engine_rule_count": 5,
        "engine_executable_count": len(executables),
    }
    result = {"metadata": metadata, "cli_ms": cli, "engine_us": engine, "peak_rss_mib": memory}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    report = [
        "# Python versus Rust benchmark", "",
        f"Measured {metadata['timestamp_utc']} on {metadata['cpu']} ({metadata['machine']}, {metadata['platform']}).",
        f"Original local thefuck 3.32; {metadata['python']}; {metadata['rustc']}; Rust release build with thin LTO.", "",
        "## End-to-end CLI", "",
        f"{args.samples} fresh processes per cell after {args.warmups} warmups. Medians in milliseconds; p95 in parentheses.",
        "Case and implementation order are shuffled each round. Both programs receive identical PATH, Bash history, settings and command text.",
        "Default rules are enabled. Every correction is checked against the expected output; corrections are printed, never executed.",
        "Captured cases read the same preallocated 1 MiB terminal log. Rerun cases include obtaining failed-command output.",
        "Filesystem and bytecode caches are warm. Rust's PATH cache is enabled except in the explicitly labelled case.", "",
        "| Case | Python ms (p95) | Rust ms (p95) | Speedup |",
        "|---|---:|---:|---:|",
    ]
    for name, measurement in cli.items():
        p, r = measurement["python"], measurement["rust"]
        report.append(f"| {name} | {p['median']:.2f} ({p['p95']:.2f}) | {r['median']:.2f} ({r['p95']:.2f}) | {measurement['speedup']:.1f}× |")
    report += ["", "## Warm engine", "",
               "Medians in microseconds per correction. Five common rules are preloaded in both implementations; executable candidates and history are injected identically.",
               "Each iteration creates a fresh Command and requests the first correction. Rule discovery/imports, process startup, log parsing and command reruns are excluded.",
               f"{args.engine_samples} batches per cell, calibrated to roughly 30 ms each. Candidate list contains 2,000 names. These are a focused microbenchmark, not full CLI timings.", "",
               "| Case | Python µs | Rust µs | Speedup |", "|---|---:|---:|---:|"]
    for name, measurement in engine.items():
        report.append(f"| {name} | {measurement['python']['median']:.2f} | {measurement['rust']['median']:.2f} | {measurement['speedup']:.1f}× |")
    if memory:
        report += ["", "## Memory", "",
                   f"Median peak resident memory for cd_parent_rerun over five separate `/usr/bin/time -l` runs: Python {memory['python']['median']:.2f} MiB; Rust {memory['rust']['median']:.2f} MiB.",
                   "This measures the correction process, not aggregate memory of its child processes."]
    report += ["", "Results describe warm local macOS runs, not cold disk startup or other platforms. CLI gains include native startup, rule loading, PATH handling, terminal parsing and rerun optimizations; they do not isolate language execution speed.",
               "Raw samples, dependency versions and source/binary fingerprints are in results.json.", ""]
    report_path = args.output.with_suffix(".md")
    report_path.write_text("\n".join(report))
    print("\n" + "\n".join(report), flush=True)
    print(f"Saved {args.output} and {report_path}", flush=True)


if __name__ == "__main__":
    main()
