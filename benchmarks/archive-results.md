# Correction benchmark against installed CLIs

15 processes per mode; times in ms (median / p95). Cold runs
start with an empty cache directory; warm runs reuse one. Probes are
completer subprocesses (cold / warm). RSS uses the process usage returned
by wait4; child accounting depends on the OS. In-process
stage timings: `cargo bench --bench engine`.

| Case | Correction | Cold | Warm | Probes | Peak RSS |
|---|---|---|---|---|---|
| archive members (tar) | `tar xf backup.tar docs/reprot.txt` → `tar xf backup.tar docs/report.txt` | 25 / 407 | 24 / 24 | 2 / 2 | 6.9 MiB |
| archive members (gtar) | `gtar xf backup.tar docs/reprot.txt` → `gtar xf backup.tar docs/report.txt` | 38 / 104 | 38 / 40 | 2 / 2 | 7.2 MiB |
| archive members (unzip) | `unzip photos.zip docs/reprot.txt` → `unzip photos.zip docs/report.txt` | 39 / 40 | 39 / 40 | 2 / 2 | 7.4 MiB |
| archive members (zipinfo) | `zipinfo photos.zip docs/reprot.txt` → `zipinfo photos.zip docs/report.txt` | 40 / 40 | 39 / 40 | 2 / 2 | 7.4 MiB |
| archive members (7z) | `7z x photos.zip docs/reprot.txt` → `7z x photos.zip docs/report.txt` | 44 / 48 | 42 / 43 | 2 / 2 | 17.3 MiB |
| archive members (unar) | `unar photos.zip docs/reprot.txt` → `unar photos.zip docs/report.txt` | 53 / 58 | 51 / 53 | 2 / 2 | 11.5 MiB |
| archive members (ar) | `ar x libfoo.a reprot.o` → `ar x libfoo.a report.o` | 104 / 140 | 52 / 54 | 2 / 2 | 6.2 MiB |
| archive members (jar) | `jar xf app.jar docs/reprot.txt` → `jar xf app.jar docs/report.txt` | 134 / 137 | 135 / 137 | 2 / 2 | 50.2 MiB |

Measured on macOS on 2026-10-06 with bsdtar 3.5.3/libarchive 3.7.4,
GNU tar 1.35, Apple Info-ZIP UnZip 6.00 (unzip and zipinfo), 7-Zip 25.01,
The Unarchiver 1.10.7 (unar, listed through lsar's JSON), Xcode's ar, and
OpenJDK 27's jar (NOTYPO_TEST_JDK names its bin directory). The harness
creates isolated local tar/zip/jar/ar fixtures and checks every correction
and the absence of extraction. Readers need trusted_completers; help, man,
history, replay, and legacy sources are disabled. No correction is executed.

Tar's probes identify its implementation and list escaped member names.
The others list names and verify the proposed name with an exact selection;
console output can be lossy. Member names are resource edits requiring
confirmation and are not persisted in a completion cache, so warm requests
repeat both probes. /usr/bin/ar is an xcrun shim: a cold run resolves the
developer directory under the probe's private HOME first (about 50 ms).
jar starts a JVM (about 65 ms per probe and 50 MiB peak RSS, which is the
JVM's). Cold p95 values for tar and gtar include single startup outliers.
These are single-member fixtures, not measurements of large or encrypted
archives. Inputs of the other archivers and compressors are repaired from
the filesystem alone and run no probe.
