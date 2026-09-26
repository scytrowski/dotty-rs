# Parser source-corpus report

This tool measures the current `dotty-parser` parser against real Scala 3.9
source files. It is intentionally a batch measurement, not a differential
tree-equality gate: the report describes which files the Rust parser accepts
cleanly and which syntax families still produce recoverable diagnostics.

The runner expects a checkout of the pinned Scala 3.9.0 source tree. The
checkout must contain `library/src` and `compiler/src`:

```text
git clone --branch 3.9.0 --depth 1 https://github.com/scala/scala3.git /tmp/scala3-3.9.0
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output /tmp/parser-corpus-3.9.0.json
```

The convenience runner creates one compilation-mode manifest and sends it to
the Scala 3.9 oracle once. Each Rust source file is parsed in an isolated
worker process with a timeout. A worker/process failure, panic, or hang is
reported as a hard failure; when a timeout occurs, the worker process is killed
and reaped before the next file starts, so parser state and OS resources cannot
accumulate in detached threads. A recoverable parser diagnostic does not make
the command fail; a non-zero exit status is reserved for scanner failures,
worker/process failures, panics, and hangs.

The JSON report is deterministic: files are sorted, histogram keys are sorted,
and each failure bucket keeps at most five representative paths. Its schema
contains:

- attempted, clean, recoverable, and hard-failure file counts;
- panic, hang, and scanner-diagnostic counts;
- a `ParseDiagnosticKind` histogram;
- a first-failure histogram, including normalized `UnsupportedSyntax`
  messages and representative source paths.

`scala_oracle_files` records how many files the reference batch emitted. It is
an oracle liveness/count check, not a full-tree equality claim for the source
corpus; exact normalized tree equality remains the job of
`tools/scala-parser-oracle/compare.sh`.
`scala_oracle_failures` records files for which Dotty itself threw while
building its source tree; the batch keeps going so those failures remain
visible instead of truncating the corpus measurement.

The low-level binary also accepts repeated `--root` options for focused runs:

```text
cargo run --locked -p dotty-parser-corpus-report -- \
  --root tools/scala-parser-oracle/fixtures --output /tmp/report.json
```

The convenience runner verifies the pinned Scala checkout revision before
parsing and records both version and revision in the report. This keeps a
checked-in baseline reproducible rather than silently measuring a newer
compiler tree.

Pass `--namer` to run source naming in the same isolated workers and add
Namer outcomes and deferred-feature counts to the report. `--skip-oracle`
skips the Scala compiler oracle batch while retaining the pinned revision
check; in that mode the report leaves the oracle count fields empty. This is
useful when the Scala source checkout is available but its sbt oracle cannot
run. The Namer v0.1 compatibility measurement and its interpretation are in
[`namer-v0.1-compatibility.md`](../../docs/namer-v0.1-compatibility.md).

`baseline-scala3-3.9.0.json` is the initial measurement from issue #185.
`parser-v0.1-final-scala3-3.9.0.json` is the post-#186–#193 measurement used
for the Parser v0.1 gate. Recreate the latter from the pinned checkout with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-v0.1-final-scala3-3.9.0.json
```

The checked-in final report was measured against dotty-rs commit
`1562374` (the `main` tip after PR #217). Its `source_revision` identifies
the Scala checkout, not the dotty-rs revision; the latter is recorded here so
the two dimensions are not confused.

`parser-post-issue-252-scala3-3.9.0.json` reruns the same measurement at
dotty-rs commit `222dd59` (after PR #247). It uses the identical Scala
revision and sorted 1,236-file manifest. The comparison and interpretation are
recorded in [`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate the latest report with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-252-scala3-3.9.0.json
```

`parser-post-issue-289-scala3-3.9.0.json` reruns the same parser-only
measurement at dotty-rs commit `5c25649` (after PR #288), using the same pinned
Scala revision and 1,236-file manifest. Its before/after comparison is recorded
in [`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-289-scala3-3.9.0.json
```

The existing `tools/scala-parser-oracle/compare.sh` remains the exact
Scala/Rust differential gate for the checked-in fixture corpus. This report is
the larger source-compatibility measurement and does not weaken that gate.
