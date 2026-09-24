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
the Scala 3.9 oracle once, then parses the same corpus in one Rust process.
There is no Scala, Cargo, or Rust-tool restart per file. Each Rust source file
is parsed in a bounded worker with a timeout, so a panic or hang is reported as
a hard failure instead of disappearing from the measurement. A recoverable
parser diagnostic does not make the command fail; a non-zero exit status is
reserved for scanner failures, panics, and hangs.

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

The existing `tools/scala-parser-oracle/compare.sh` remains the exact
Scala/Rust differential gate for the checked-in fixture corpus. This report is
the larger source-compatibility measurement and does not weaken that gate.
