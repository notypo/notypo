#!/usr/bin/env python3
"""Correction latency, probes, and memory against installed CLIs.

Runs `notypo --json` (which never executes the correction): each case cold
(an empty cache directory) and warm (after one run filled it), reporting
median and p95 wall time, completer probes, and peak RSS. Cases whose app
is missing are skipped. For the comparison with Python thefuck, see
compare.py.

    cargo build --release
    cargo bench --bench engine            # in-process stages
    python3 benchmarks/structured.py      # writes benchmarks/structured-results.md
    python3 benchmarks/structured.py --shell-fixtures --case shell \
        --output benchmarks/shell-results.md
    python3 benchmarks/structured.py --protocols --case protocol \
        --output benchmarks/protocol-results.md
"""

import argparse
import io
import json
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "release" / "notypo"

# (label, app that must be installed, command)
CASES = [
    ("aws operation and option", "aws", "aws ec2 describ-instances --regoin eu-west-1"),
    ("gcloud two groups", "gcloud", "gcloud compte instnaces list"),
    ("az group", "az", "az storage acount list"),
    ("git subcommand", "git", "git sttus"),
    ("kubectl (cobra)", "kubectl", "kubectl gt pods"),
    ("terraform (posener)", "terraform", "terraform plna"),
    ("ls option (man page)", "ls", "ls --colro=auto"),
    ("program name", "git", "gti status"),
]


def run(command, env):
    """One process: wall seconds, peak RSS in KiB, and the parsed report."""
    args = [str(BINARY), "--json"]
    if env.get("TF_HISTORY") != command:
        args.extend(["--force-command", command])
    started = time.perf_counter()
    proc = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            cwd=env.get("NOTYPO_BENCH_CWD"))
    out = proc.stdout.read()
    _, status, usage = os.wait4(proc.pid, 0)
    elapsed = time.perf_counter() - started
    rss = usage.ru_maxrss // 1024 if sys.platform == "darwin" else usage.ru_maxrss
    report = json.loads(out) if out else None
    return elapsed, rss, report


def summarize(times):
    times = sorted(times)
    p95 = times[math.ceil(len(times) * 0.95) - 1]
    return statistics.median(times) * 1000, p95 * 1000


def environment(cache):
    env = dict(os.environ)
    env.update({
        "XDG_CACHE_HOME": str(cache),
        "TF_SHELL": "bash",
        "THEFUCK_NO_COLORS": "true",
        "HISTFILE": str(cache / "history"),
    })
    for key in ("TF_HISTORY", "SHELL_LOGGER_SOCKET", "NOTYPO_NO_CACHE"):
        env.pop(key, None)
    return env


# Installed apps reached through the newer bridges, each under the trust it
# needs: (label, app, command, expected, trusted_completers, trusted_help).
PROTOCOL_CASES = [
    ("pip protocol", "pip3", "pip3 config lsit", "pip3 config list", ["python:pip"], []),
    ("npm protocol", "npm", "npm config lsit", "npm config list", ["npm:npm"], []),
    ("cargo protocol", "cargo", "cargo build --releae", "cargo build --release",
     ["rust:cargo"], ["rust:cargo"]),
    ("dotnet protocol", "dotnet", "dotnet build --configuraton Release",
     "dotnet build --configuration Release", ["dotnet:sdk"], []),
    ("clap protocol (sofka)", "sofka", "sofka --readoly", "sofka --readonly",
     ["sofka"], ["sofka"]),
    ("clap protocol (just shim)", "just", "just --dry-rnu", "just --dry-run", ["just"], []),
    ("urfave/cli protocol (lefthook)", "lefthook", "lefthook valdate", "lefthook validate",
     ["github.com/evilmartians/lefthook/v2"], ["lefthook"]),
    ("cobra protocol (k9s, user-trusted)", "k9s", "k9s --readoly", "k9s --readonly",
     ["github.com/derailed/k9s"], []),
    ("bash handler protocol (bq)", "bq", "bq qeury x", "bq query x", ["bq"], []),
    ("Click protocol (SAM)", "sam", "sam build --use-contaner",
     "sam build --use-container", ["python:samcli"], ["python:samcli"]),
    ("Click protocol (OCI)", "oci", "oci os bucket list --namespce x",
     "oci os bucket list --namespace x", ["python:oci_cli"], ["python:oci_cli"]),
    ("oclif manifests (eas)", "eas", "eas build --platfrom ios", "eas build --platform ios",
     [], []),
    ("Symfony Console protocol (Composer)", "composer", "composer install --dry-rnu",
     "composer install --dry-run", ["composer"], ["composer"]),
    ("kingpin protocol (promtool)", "promtool", "promtool check config --syntax-onyl prometheus.yml",
     "promtool check config --syntax-only prometheus.yml",
     ["github.com/prometheus/prometheus/cmd/promtool"], ["promtool"]),
    ("kingpin protocol (kopia)", "kopia", "kopia snapshot restore --paralell 4 object-id output",
     "kopia snapshot restore --parallel 4 object-id output", ["github.com/kopia/kopia"], ["kopia"]),
    ("kingpin/fisk protocol (nats)", "nats", "nats stream ls --jsoon",
     "nats stream ls --json", ["github.com/nats-io/natscli/nats"], ["nats"]),
    ("RabbitMQ CLI protocol (command)", "rabbitmqctl", "rabbitmqctl lsit_queues",
     "rabbitmqctl list_queues", ["rabbitmq:rabbitmqctl"], ["rabbitmq:rabbitmqctl"]),
    ("RabbitMQ CLI protocol (option)", "rabbitmqctl",
     "rabbitmqctl delete_queue --if-emtpy orders", "rabbitmqctl delete_queue --if-empty orders",
     ["rabbitmq:rabbitmqctl"], ["rabbitmq:rabbitmqctl"]),
    # 2026-10-06: posener argument-slot checks, cobra alias confirmation and
    # boolean-flag detection, and shell-handler arity checks add probes.
    ("posener protocol (vault, audited)", "vault", "vault secrets lsit", "vault secrets list",
     [], []),
    ("cobra protocol (helm, audited, alias check)", "helm", "helm instal x", "helm install x",
     [], []),
    ("cobra boolean flag (restic, audited)", "restic", "restic backup --exclud x .",
     "restic backup --exclude x .", [], []),
    ("zsh handler protocol (tmux)", "tmux", "tmux attahc -t x", "tmux attach -t x",
     ["tmux"], []),
]


def protocol_cases():
    cases = []
    for label, app, command, expected, completers, help_ in PROTOCOL_CASES:
        overrides = {
            "NOTYPO_TRUSTED_COMPLETERS": json.dumps(completers),
            "NOTYPO_TRUSTED_HELP": json.dumps(help_),
            "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
            "NOTYPO_REPLAY_FOR_DIAGNOSIS": "false",
            "HOMEBREW_NO_AUTO_UPDATE": "1",
        }
        if label.startswith("Click protocol"):
            # Match the installed-client checks and a shell-captured failure.
            # OCI's root protocol fails; failure context permits checking its
            # later option without treating an unknown root word as invalid.
            overrides["NOTYPO_EXIT_STATUS"] = "1"
            overrides["TF_HISTORY"] = command
            overrides["TF_ALIAS"] = "fuck"
        if app == "oci":
            # Its unsupported root reply needs the trusted help fallback to
            # establish the existing command path before native option repair.
            overrides["NOTYPO_DISABLED_SOURCES"] = "man:history:legacy"
        cases.append((label, app, command, overrides, expected))
    return cases + powershell_cases()


def powershell_cases():
    """PowerShell's own commands, described by a profile-free PowerShell. The
    session report is what the PowerShell function would pass."""
    pwsh = os.environ.get("NOTYPO_BENCH_PWSH") or shutil.which("pwsh")
    if not pwsh:
        print("skip PowerShell protocol: pwsh is not installed")
        return []
    cases = []
    for label, command, expected, report in [
        ("PowerShell protocol (parameter)", "Get-ChildItem -Recrse", "Get-ChildItem -Recurse",
         "Get-ChildItem\tCmdlet\tCmdlet\tGet-ChildItem"),
        ("PowerShell protocol (cmdlet name)", "Get-ChildItme -Name", "Get-ChildItem -Name",
         "Get-ChildItme\t\t\t"),
    ]:
        overrides = {
            "TF_SHELL": "powershell",
            "NOTYPO_POWERSHELL": pwsh,
            "NOTYPO_POWERSHELL_COMMANDS": report,
            "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
        }
        cases.append((label, pwsh, command, overrides, expected))
    # Apps' generated completers registered in a session, as a profile's
    # `<app> completion powershell | Out-String | Invoke-Expression` leaves
    # them; the variable is built by the integration function's own code.
    for label, app, generator, command, expected in [
        ("PowerShell completer protocol (mdbook)", "mdbook", "mdbook completions powershell",
         "mdbook serv", "mdbook serve"),
        ("PowerShell completer protocol (rustup)", "rustup", "rustup completions powershell",
         "rustup toolchian list", "rustup toolchain list"),
    ]:
        if shutil.which(app) is None:
            continue
        overrides = {
            "TF_SHELL": "powershell",
            "NOTYPO_POWERSHELL": pwsh,
            "NOTYPO_POWERSHELL_COMPLETIONS": session_completions(pwsh, generator, command),
            "NOTYPO_TRUSTED_COMPLETERS": json.dumps([app]),
            "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
        }
        cases.append((label, app, command, overrides, expected))
    command = "Get-NotypoBench -Scope west -Target wesst-one"
    cases.append(("PowerShell parameter callback protocol", pwsh, command, {
        "TF_SHELL": "powershell",
        "NOTYPO_POWERSHELL": pwsh,
        "NOTYPO_POWERSHELL_PARAMETERS": session_parameters(pwsh, command),
        "NOTYPO_TRUSTED_COMPLETERS": json.dumps(["Get-NotypoBench"]),
        "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
        "NOTYPO_REPLAY_FOR_DIAGNOSIS": "false",
    }, "Get-NotypoBench -Scope west -Target west-one"))
    command = "Get-NotypoBench wesst-one -Scope west"
    cases.append(("PowerShell positional callback protocol", pwsh, command, {
        "TF_SHELL": "powershell",
        "NOTYPO_POWERSHELL": pwsh,
        "NOTYPO_POWERSHELL_PARAMETERS": session_parameters(pwsh, command),
        "NOTYPO_TRUSTED_COMPLETERS": json.dumps(["Get-NotypoBench"]),
        "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
        "NOTYPO_REPLAY_FOR_DIAGNOSIS": "false",
    }, "Get-NotypoBench west-one -Scope west"))
    return cases


def session_parameters(pwsh, line):
    """Capture a callback-bearing function with the integration's own code.
    It has no operation to run; the body throws if accidentally invoked."""
    definitions = """function Get-NotypoBench {
        [CmdletBinding()] param([Parameter(Position=0)][ValidateSet('west','east')][string]$Scope,
            [Parameter(Position=1)][ArgumentCompleter({
                param($cmd,$parameter,$word,$ast,$bound)
                $bound['Scope'] + '-one'
            })][string]$Target)
        throw 'the benchmark command must never run'
    }"""
    capture = (ROOT / "src" / "shells" / "powershell_parameters.ps1").read_text()
    script = (". ([scriptblock]::Create($env:NOTYPO_BENCH_DEFINITIONS))\n"
              "$history = $env:NOTYPO_BENCH_LINE\n" + capture + "\n"
              "[Console]::Out.Write($env:NOTYPO_POWERSHELL_PARAMETERS)")
    text = subprocess.run([pwsh, "-NoProfile", "-NonInteractive", "-Command", script],
                          capture_output=True, text=True, check=True,
                          env=dict(os.environ, NOTYPO_BENCH_DEFINITIONS=definitions,
                                   NOTYPO_BENCH_LINE=line)).stdout
    snapshot = json.loads(text)
    if snapshot["commands"][0]["portable"] is not True:
        raise RuntimeError("PowerShell did not capture the benchmark function")
    return text


def session_completions(pwsh, generator, line):
    """NOTYPO_POWERSHELL_COMPLETIONS for `line` in a session that loaded the
    completer `generator` prints, collected by the integration's own code."""
    alias = subprocess.run([str(BINARY), "--alias"], capture_output=True, text=True, check=True,
                           env=dict(os.environ, TF_SHELL="powershell")).stdout
    start = alias.index("$env:NOTYPO_POWERSHELL_COMPLETIONS = try {")
    end = alias.index("} catch { };", start) + len("} catch { };")
    script = (f"{generator} | Out-String | Invoke-Expression\n"
              f"$history = $env:NOTYPO_BENCH_LINE\n{alias[start:end]}\n"
              "[Console]::Out.Write($env:NOTYPO_POWERSHELL_COMPLETIONS)")
    return subprocess.run([pwsh, "-NoProfile", "-NonInteractive", "-Command", script],
                          capture_output=True, text=True, check=True,
                          env=dict(os.environ, NOTYPO_BENCH_LINE=line)).stdout


def shell_fixtures(scratch):
    """Installed native shell handlers; no app startup or cloud writes."""
    app = "notypo-shell-fixture"
    root = scratch / "shell-fixtures"
    bin_dir = root / "bin"
    bin_dir.mkdir(parents=True)
    marker = root / "operation-marker"
    executable = bin_dir / app
    executable.write_text("#!/bin/sh\nprintf ran > \"$NOTYPO_BENCH_MARKER\"\nexit 99\n")
    executable.chmod(0o755)
    source = f"{app} nodes lsit --foramt jsno"
    expected = f"{app} nodes list --format json"
    cases = []
    for shell in ("fish", "zsh"):
        if shutil.which(shell) is None:
            print(f"skip {shell} shell handler: {shell} is not installed")
            continue
        completions = root / shell
        completions.mkdir()
        if shell == "fish":
            (completions / f"{app}.fish").write_text(f"""complete -c {app} -f
complete -c {app} -n 'not __fish_seen_subcommand_from nodes' -a nodes
complete -c {app} -n '__fish_seen_subcommand_from nodes; and not __fish_seen_subcommand_from list' -a list
complete -c {app} -n '__fish_seen_subcommand_from list' -l format -x -a 'json yaml'
""")
            path_key = "NOTYPO_FISH_COMPLETE_PATH"
        else:
            (completions / f"_{app}").write_text(f"""#compdef {app}
case ${{(Q)words[2]}} in
  '') compadd -- nodes;;
  nodes)
    case ${{(Q)words[3]}} in
      '') compadd -- list;;
      list) _arguments '1:group:(nodes)' '2:command:(list)' '--format[Format]:format:(json yaml)' '*:argument:';;
    esac;;
esac
""")
            path_key = "NOTYPO_ZSH_FPATH"
        overrides = {
            "TF_SHELL": shell,
            "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", os.defpath),
            path_key: str(completions),
            "NOTYPO_TRUSTED_COMPLETERS": app,
            "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
            "NOTYPO_BENCH_MARKER": str(marker),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
        }
        cases.append((f"{shell} shell handler", shell, source, overrides, expected))
    return cases, marker


def archive_fixtures(root):
    """Real installed readers list local fixture archives; no extraction runs."""
    work = root / "archives"
    work.mkdir()
    with tarfile.open(work / "backup.tar", "w") as archive:
        entry = tarfile.TarInfo("docs/report.txt")
        entry.size = 1
        archive.addfile(entry, io.BytesIO(b"x"))
    for name in ["photos.zip", "app.jar"]:
        with zipfile.ZipFile(work / name, "w") as archive:
            archive.writestr("docs/report.txt", "x")

    def member(name, data):
        header = f"{name:<16}{0:<12}{0:<6}{0:<6}{644:<8}{len(data):<10}`\n".encode()
        return header + data + (b"\n" if len(data) % 2 else b"")

    # A BSD/System V archive; member names have no directories.
    (work / "libfoo.a").write_bytes(b"!<arch>\n" + member("report.o", b"x"))
    # jar needs a JDK; macOS's /usr/bin/jar is a stub without one.
    jdk = os.environ.get("NOTYPO_TEST_JDK")
    path = os.pathsep.join(p for p in [jdk, os.environ.get("PATH", os.defpath)] if p)
    cases = []
    for app, mode, archive, member_name in [
            ("tar", "xf", "backup.tar", "docs/reprot.txt"),
            ("gtar", "xf", "backup.tar", "docs/reprot.txt"),
            ("unzip", "", "photos.zip", "docs/reprot.txt"),
            ("zipinfo", "", "photos.zip", "docs/reprot.txt"),
            ("7z", "x", "photos.zip", "docs/reprot.txt"),
            ("unar", "", "photos.zip", "docs/reprot.txt"),
            ("ar", "x", "libfoo.a", "reprot.o"),
            ("jar", "xf", "app.jar", "docs/reprot.txt")]:
        command = " ".join(word for word in [app, mode, archive, member_name] if word)
        overrides = {
            "NOTYPO_BENCH_CWD": str(work),
            "XDG_CONFIG_HOME": str(root / "config"),
            "NOTYPO_TRUSTED_COMPLETERS": json.dumps([app]),
            "NOTYPO_TRUSTED_HELP": "[]",
            "NOTYPO_DISABLED_SOURCES": "help:man:history:legacy",
            "NOTYPO_REPLAY_FOR_DIAGNOSIS": "false",
            "NOTYPO_EXIT_STATUS": "1",
            "TF_HISTORY": command,
            "PATH": path,
        }
        if app == "jar" and subprocess.run([shutil.which("jar", path=path) or "jar", "--version"],
                                           capture_output=True).returncode != 0:
            print("skip archive members (jar): no JDK (set NOTYPO_TEST_JDK)")
            continue
        cases.append((f"archive members ({app})", app, command, overrides,
                      command.replace("reprot", "report")))
    return cases, work


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=15)
    parser.add_argument("--output", default=str(ROOT / "benchmarks" / "structured-results.md"))
    parser.add_argument("--shell-fixtures", action="store_true", help="include isolated fish/Zsh handlers")
    parser.add_argument("--protocols", action="store_true",
                        help="include installed apps behind the newer protocol bridges")
    parser.add_argument("--archive-fixtures", action="store_true",
                        help="include installed archive readers and isolated local archives")
    parser.add_argument("--case", help="only run labels containing this text")
    options = parser.parse_args()
    if options.samples < 1:
        parser.error("--samples must be positive")
    if not BINARY.exists():
        sys.exit("build first: cargo build --release")

    rows = []
    with tempfile.TemporaryDirectory(prefix="notypo-bench-") as scratch:
        scratch = Path(scratch)
        cases = [(label, app, command, {}, None) for label, app, command in CASES]
        marker = None
        archives = None
        if options.shell_fixtures:
            fixtures, marker = shell_fixtures(scratch)
            cases.extend(fixtures)
        if options.protocols:
            cases.extend(protocol_cases())
        if options.archive_fixtures:
            fixtures, archives = archive_fixtures(scratch)
            cases.extend(fixtures)
        for label, app, command, overrides, expected in cases:
            if options.case and options.case not in label:
                continue
            if shutil.which(app) is None:
                print(f"skip {label}: {app} is not installed")
                continue
            slug = re.sub(r"[^A-Za-z0-9]+", "-", label)
            cold, warm, rss = [], [], []
            probes_cold = probes_warm = None
            suggestion = None
            for n in range(options.samples):
                cache = scratch / f"cold-{slug}-{n}"
                cache.mkdir()
                elapsed, peak, report = run(command, environment(cache) | overrides)
                cold.append(elapsed)
                rss.append(peak)
                probes_cold = report["probes"]
                suggestion = (report["candidates"] or [{}])[0].get("command")
                if expected and suggestion != expected:
                    sys.exit(f"{label}: expected {expected!r}, got {suggestion!r}")
            cache = scratch / f"warm-{slug}"
            cache.mkdir()
            run(command, environment(cache) | overrides)
            for _ in range(options.samples):
                elapsed, peak, report = run(command, environment(cache) | overrides)
                if expected and report["candidates"][0]["command"] != expected:
                    sys.exit(f"{label}: warm correction changed")
                warm.append(elapsed)
                rss.append(peak)
                probes_warm = report["probes"]
            row = {
                "case": label,
                "command": command,
                "suggestion": suggestion,
                "cold": summarize(cold),
                "warm": summarize(warm),
                "probes": (probes_cold, probes_warm),
                "rss_kib": max(rss),
            }
            rows.append(row)
            print(json.dumps(row))
        if marker is not None and marker.exists():
            sys.exit("the shell fixture operation was executed")
        if archives is not None and any(
                (archives / name).exists() for name in ["docs", "report.o", "META-INF"]):
            sys.exit("the archive fixture was extracted")

    fixture_only = bool(rows) and all(r["case"].endswith(" shell handler") for r in rows)
    protocol_only = bool(rows) and all(" protocol" in r["case"] for r in rows)
    lines = [
        "# Correction benchmark with installed shell handlers" if fixture_only else
        "# Correction benchmark against installed CLIs",
        "",
        f"{options.samples} processes per mode; times in ms (median / p95). Cold runs",
        "start with an empty cache directory; warm runs reuse one. Probes are",
        "completer subprocesses (cold / warm). RSS uses the process usage returned",
        "by wait4; child accounting depends on the OS. In-process",
        "stage timings: `cargo bench --bench engine`.",
        "",
        "| Case | Correction | Cold | Warm | Probes | Peak RSS |",
        "|---|---|---|---|---|---|",
    ]
    for r in rows:
        lines.append(
            "| {} | `{}` → `{}` | {:.0f} / {:.0f} | {:.0f} / {:.0f} | {} / {} | {:.1f} MiB |".format(
                r["case"], r["command"], r["suggestion"], *r["cold"], *r["warm"],
                *r["probes"], r["rss_kib"] / 1024,
            )
        )
    if fixture_only:
        lines.extend([
            "",
            "Isolated fixture apps use the installed shells' real native completion",
            "handlers. Each correction fixes a nested command, option, and enum value.",
            "The harness checks every result and an operation marker; suggestions are",
            "never executed. This measures shell-handler overhead, not a cloud CLI.",
            "",
            "Handwritten handlers provide partial evidence and use request-local memoization.",
            "Warm runs reuse the cache directory but repeat their native probes.",
            "Zsh's completion-system initialization dominates its elapsed time.",
        ])
    if protocol_only:
        lines.extend([
            "",
            "Installed apps behind the newer bridges, each with the trust it needs",
            "(`trusted_completers`, plus `trusted_help` for Cargo, Sofka's generator,",
            "lefthook's urfave/cli v3 hooks, Click/Symfony app discovery, kingpin's",
            "and RabbitMQ's metadata); fallback help, man, history, and legacy",
            "sources are off except OCI's trusted help fallback for its unsupported root reply.",
            "Click rows use a simulated shell-captured failure (history and status 1); replay is off.",
            "The harness checks every suggestion; none is executed.",
            "These bridges keep answers in request memory, so warm runs repeat their",
            "probes; cobra answers for command-only contexts reach the disk cache.",
            "The bq case runs the Cloud SDK's bash helper, which starts Python for",
            "`bq help`. PowerShell cases start a profile-free PowerShell once to",
            "describe the cmdlet, and once more to list command names when the",
            "name itself is wrong; their peak RSS is that PowerShell's.",
        ])
    Path(options.output).write_text("\n".join(lines) + "\n")
    print(f"wrote {options.output}")


if __name__ == "__main__":
    main()
