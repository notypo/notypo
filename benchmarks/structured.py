#!/usr/bin/env python3
"""Structured engine latency, probes, and memory, end to end.

Runs `notypo --json` (which never executes the correction) against the
installed CLIs: each case cold (an empty cache directory) and warm (after
one run filled it), reporting median and p95 wall time, completer probes,
and peak RSS. Cases whose app is missing are skipped. The rule engine is
compared only on commands that are safe to rerun, because it reruns the
failed command to read its output.

    cargo build --release
    cargo bench --bench engine            # in-process stages
    python3 benchmarks/structured.py      # writes benchmarks/structured-results.md
"""

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "release" / "notypo"

# (label, app that must be installed, command, safe to rerun)
CASES = [
    ("aws operation and option", "aws", "aws ec2 describ-instances --regoin eu-west-1", False),
    ("gcloud two groups", "gcloud", "gcloud compte instnaces list", False),
    ("az group", "az", "az storage acount list", False),
    ("git subcommand", "git", "git sttus", True),
    ("kubectl (cobra)", "kubectl", "kubectl gt pods", False),
    ("terraform (posener)", "terraform", "terraform plna", False),
    ("ls option (man page)", "ls", "ls --colro=auto", True),
    ("program name", "git", "gti status", True),
]


def run(command, env, engine):
    """One process: wall seconds, peak RSS in KiB, and the parsed report."""
    args = [str(BINARY)]
    args += ["--json", "--force-command", command] if engine == "native" else ["-y", "--force-command", command]
    started = time.perf_counter()
    proc = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    out = proc.stdout.read()
    _, status, usage = os.wait4(proc.pid, 0)
    elapsed = time.perf_counter() - started
    rss = usage.ru_maxrss // 1024 if sys.platform == "darwin" else usage.ru_maxrss
    report = json.loads(out) if engine == "native" and out else None
    return elapsed, rss, report


def summarize(times):
    times = sorted(times)
    p95 = times[min(len(times) - 1, max(0, round(len(times) * 0.95) - 1))]
    return statistics.median(times) * 1000, p95 * 1000


def environment(cache, engine):
    env = dict(os.environ)
    env.update({
        "XDG_CACHE_HOME": str(cache),
        "TF_SHELL": "bash",
        "THEFUCK_NO_COLORS": "true",
        "NOTYPO_ENGINE": engine,
        "HISTFILE": str(cache / "history"),
    })
    for key in ("TF_HISTORY", "SHELL_LOGGER_SOCKET", "NOTYPO_NO_CACHE"):
        env.pop(key, None)
    return env


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=15)
    parser.add_argument("--output", default=str(ROOT / "benchmarks" / "structured-results.md"))
    options = parser.parse_args()
    if not BINARY.exists():
        sys.exit("build first: cargo build --release")

    rows = []
    with tempfile.TemporaryDirectory(prefix="notypo-bench-") as scratch:
        scratch = Path(scratch)
        for label, app, command, rerun_safe in CASES:
            if shutil.which(app) is None:
                print(f"skip {label}: {app} is not installed")
                continue
            cold, warm, rss, legacy = [], [], [], []
            probes_cold = probes_warm = None
            suggestion = None
            for n in range(options.samples):
                cache = scratch / f"cold-{label}-{n}"
                cache.mkdir()
                elapsed, peak, report = run(command, environment(cache, "native"), "native")
                cold.append(elapsed)
                rss.append(peak)
                probes_cold = report["probes"]
                suggestion = (report["candidates"] or [{}])[0].get("command")
            cache = scratch / f"warm-{label}"
            cache.mkdir()
            run(command, environment(cache, "native"), "native")
            for _ in range(options.samples):
                elapsed, peak, report = run(command, environment(cache, "native"), "native")
                warm.append(elapsed)
                rss.append(peak)
                probes_warm = report["probes"]
            if rerun_safe:
                for _ in range(options.samples):
                    elapsed, _, _ = run(command, environment(cache, "legacy"), "legacy")
                    legacy.append(elapsed)
            row = {
                "case": label,
                "command": command,
                "suggestion": suggestion,
                "cold": summarize(cold),
                "warm": summarize(warm),
                "probes": (probes_cold, probes_warm),
                "rss_kib": max(rss),
                "legacy": summarize(legacy) if legacy else None,
            }
            rows.append(row)
            print(json.dumps(row))

    lines = [
        "# Structured engine benchmark",
        "",
        f"{options.samples} processes per mode; times in ms (median / p95). Cold runs",
        "start with an empty cache directory; warm runs reuse one. Probes are",
        "completer subprocesses (cold / warm). Peak RSS is the largest process in",
        "the tree, which for cloud CLIs is the app's own completer. The rule",
        "engine column reruns the command and is measured only where that is",
        "harmless. In-process stage timings: `cargo bench --bench engine`.",
        "",
        "| Case | Correction | Cold | Warm | Probes | Peak RSS | Rule engine |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        legacy = "{:.0f} / {:.0f}".format(*r["legacy"]) if r["legacy"] else "n/a"
        lines.append(
            "| {} | `{}` → `{}` | {:.0f} / {:.0f} | {:.0f} / {:.0f} | {} / {} | {:.1f} MiB | {} |".format(
                r["case"], r["command"], r["suggestion"], *r["cold"], *r["warm"],
                *r["probes"], r["rss_kib"] / 1024, legacy,
            )
        )
    Path(options.output).write_text("\n".join(lines) + "\n")
    print(f"wrote {options.output}")


if __name__ == "__main__":
    main()
