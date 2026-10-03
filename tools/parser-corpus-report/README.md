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

Schema version 5 records the parser commit as `parser_revision`, partitions
outcomes into `capture_checking_cohorts`
(`enabled`, `disabled`, and `unknown`) using the parser's effective
`ParserFeatures` after compilation-unit imports have been processed. Each
cohort records the same clean/recoverable/hard-failure and diagnostic summary
as the whole corpus. Its caret audit distinguishes raw `^` characters from
lexer `^` operator tokens and capture constructs represented in the parsed
AST; this prevents comments and string contents from being counted as source
syntax while keeping unparsed operator tokens visible for manual review. The
report includes paths for raw-only marker hits and lexer caret tokens for which
the parser did not build a capture-specific AST shape, making the remaining
candidate set inspectable rather than treating every caret as capture syntax.

With `--namer`, the report also records Namer errors and invariant failures,
enum identity coverage by case category, parser-blocked enum definitions and
cases, export syntax-to-semantic handoff, and deferred feature counts. Export
forwarder synthesis remains a separate typed-phase metric. The reproducible
Scala 3.9.0 Namer results and their classification are in
[`namer-v0.1-compatibility.md`](../../docs/namer-v0.1-compatibility.md).

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

`parser-post-issue-308-scala3-3.9.0.json` reruns the same measurement at
dotty-rs commit `5ced54d` (main after PR #308), using the same pinned Scala
revision and sorted 1,236-file manifest. It records 600 clean parses (48.54%),
636 recoverable files, and zero hard failures, panics, or hangs. The comparison
with the post-#289 run and current failure categories are documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-308-scala3-3.9.0.json
```

`parser-post-issue-323-scala3-3.9.0.json` reruns that same corpus after PRs
#319–#321, at `dotty-rs` commit `3cd9089` (main after PR #321). It records 715
clean parses (57.85%), 521 recoverable files, and zero hard failures, panics,
or hangs. The Scala oracle emitted all 1,236 results with the same 30 Dotty
exceptions. The comparison and interpretation are in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-323-scala3-3.9.0.json
```

`parser-post-issue-342-scala3-3.9.0.json` reruns the corpus at dotty-rs commit
`87b309d2` (main after PRs #340–#341), with the same pinned Scala revision and
sorted 1,236-file manifest. It records 758 clean parses (61.33%), 478
recoverable files, and zero hard failures, panics, or hangs. The Scala oracle
emitted all 1,236 results with the same 30 Dotty exceptions. The comparison and
interpretation are in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-342-scala3-3.9.0.json
```

`parser-post-issue-348-scala3-3.9.0.json` measures the same corpus after PR
#351, at parser revision `d2f710011606a2a72bdae001c0579de91ee356e8`. It includes
effective capture-checking cohorts, caret-token/AST evidence, and the exact
parser and Scala source revisions. It records 797 clean parses (64.48%), 439
recoverable files, and zero hard failures, panics, or hangs; the oracle emitted
all 1,236 results with the same 30 exceptions. The comparison and capture
syntax triage are documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md) and
[`parser-capture-checking-corpus-audit-3.9.0.md`](../../docs/parser-capture-checking-corpus-audit-3.9.0.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-348-scala3-3.9.0.json
```

`parser-post-issue-379-scala3-3.9.0.json` measures the same corpus after PRs
#371, #373, #375, #377, and #378 at parser revision
`e6cc85b06082850ca52855bb510500f66535c200`. It records 908 clean parses
(73.46%), 328 recoverable files, and zero hard failures, panics, or hangs. The
Scala oracle emitted all 1,236 results with the same 30 exceptions. The
comparison and interpretation are documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-379-scala3-3.9.0.json
```

`parser-post-issue-399-scala3-3.9.0.json` reruns the same corpus after PR #393,
at parser revision `d0c2f517a25d08f31e1f90bb34a1935dc038c4aa`. It records 987
clean parses (79.85%), 249 recoverable files, and zero hard failures, panics,
or hangs. The Scala oracle again emitted all 1,236 results with 30 exceptions.
The comparison and first-failure analysis are in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-399-scala3-3.9.0.json
```

`parser-post-issue-411-scala3-3.9.0.json` measures the same corpus at parser
revision `ff5a6949ee09c5846ad585d8f1580a3e7d376827` (main after PR #411), using
the same pinned Scala source revision and sorted 1,236-file manifest. It
records 1,040 clean parses (84.14%), 196 recoverable files, and zero hard
failures, panics, or hangs. The Scala oracle emitted all 1,236 results with 30
exceptions. The comparison and interpretation are recorded in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate the report with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-411-scala3-3.9.0.json
```

`parser-post-issue-436-scala3-3.9.0.json` reruns the corpus at parser revision
`45191bc6a6336f045d6e8fa35e799785134dec42` (main after PRs #433–#435).
It records 1,074 clean parses (86.89%), 162 recoverable files, and zero hard
failures, panics, or hangs. The Scala oracle emitted all 1,236 files with 30
exceptions. The comparison and interpretation are recorded in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-436-scala3-3.9.0.json
```

The existing `tools/scala-parser-oracle/compare.sh` remains the exact
Scala/Rust differential gate for the checked-in fixture corpus. This report is
the larger source-compatibility measurement and does not weaken that gate.

`parser-post-issue-477-scala3-3.9.0.json` reruns the corpus at parser revision
`46bab6e40f78f67e8ff2f39fe6864574b78adf66` (main after PR #477), against the
same pinned Scala revision and 1,236-file manifest. It records 1,113 clean
parses (90.05%), 123 recoverable files, and zero hard failures, process
failures, panics, or hangs. The Scala oracle emitted all 1,236 results with 30
exceptions. The comparison is documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-477-scala3-3.9.0.json
```

`parser-post-issue-506-scala3-3.9.0.json` reruns the same corpus at parser
revision `4c6b1e2e53aa4bd988c11e7352a506758133b990` (main after PR #503), using
the same pinned Scala revision and 1,236-file manifest. It records 1,127 clean
parses (91.18%), 109 recoverable files, and zero hard failures, process
failures, panics, or hangs. The Scala oracle emitted all 1,236 results without
an oracle failure. The before/after comparison and remaining notable cases are
documented in [`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-506-scala3-3.9.0.json
```

`parser-post-issue-516-scala3-3.9.0.json` reruns the same corpus after PR #510 at parser revision `c08a2f4880fa687167f288fc50bbc5e94a98de6a`. It records 1,127 clean parses (91.18%), 109 recoverable files, and zero hard failures, process failures, panics, or hangs. The oracle emitted all 1,236 files with zero failures. Diagnostic occurrences increased by 86 while clean/recoverable counts and first-failure bucket counts stayed unchanged; `SafeRefs.scala` is now clean, so the aggregate points to an offsetting newly diagnostic file. The exact comparison is documented in [`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).

Recreate this report at merge revision c08a2f4880fa687167f288fc50bbc5e94a98de6a with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-516-scala3-3.9.0.json
```

`parser-post-issue-541-scala3-3.9.0.json` reruns the corpus after PR #538 at
parser revision `c6061d1f4235dfb48514a0ac4e90c17df51f1cbc`, using the same
pinned Scala source revision and 1,236-file manifest. It records 1,135 clean
parses (91.83%), 101 recoverable files, and zero hard failures, process
failures, panics, or hangs. The Scala oracle emitted all 1,236 files with zero
failures. Compared with the post-#510 report, clean parses increased by eight
and diagnostic occurrences fell by 130; `UnsupportedSyntax` fell by 135 while
`ExpectedExpression` rose by 18. The detailed comparison is in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-541-scala3-3.9.0.json
```

`parser-post-issue-562-scala3-3.9.0.json` measures current `main` after PR #561
at parser revision `49da0fa117ad406e4a97c785e73e2fb9349107b4`, with the same
pinned Scala revision and 1,236-file manifest. It records 1,142 clean parses
(92.39%), 94 recoverable files, and zero hard failures, process failures,
panics, or hangs. The Scala oracle emitted all 1,236 files with zero failures.
The comparison with the previous report is in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-562-scala3-3.9.0.json
```

`parser-post-issue-576-scala3-3.9.0.json` reruns the same measurement after
PR #576 at merge revision `0a6a55aecbdb766d2cdc4a814aff8b30a1fd37f8`. The
pinned Scala revision and 1,236-file corpus are unchanged. It records 1,165
clean parses (94.26%), 71 recoverable files, and zero hard parser failures,
process failures, panics, or hangs. The Scala oracle emitted all 1,236 files
with zero failures. The comparison and its limitations are in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-576-scala3-3.9.0.json
```

`parser-post-issue-599-scala3-3.9.0.json` reruns the same corpus at parser
revision `002d7c56724ba4a3798f421b0ac24e2efb2d48a3` (main after PR #597), using
the same pinned Scala revision and sorted 1,236-file inventory. It records
1,163 clean parses, 73 recoverable files, and zero hard failures, process
failures, panics, or hangs; the Scala oracle emitted all 1,236 results without
failures. The comparison, including the two newly reported unbound placeholders
in interpolated patterns, is documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-599-scala3-3.9.0.json
```

`parser-post-issue-619-scala3-3.9.0.json` reruns the same corpus after PR #618
at parser revision `030f9765ce84d82b43ad6bfbfd4420fa9d6ea752`. It records
1,168 clean parses, 68 recoverable files, and zero hard failures, process
failures, panics, or hangs; the Scala oracle emitted all 1,236 files without
failures. Its comparison with #597 is documented in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-619-scala3-3.9.0.json
```

`parser-post-issue-658-scala3-3.9.0.json` measures current `main` after PRs
#653–#656 at parser revision `a2a09fe39b771088ddbc21090cd76011a6ffd8df`.
It uses the same pinned Scala 3.9.0 source revision and sorted 1,236-file
corpus as issue #619. Two runs produced byte-identical reports. It records
1,176 clean parses (95.15%), 60 recoverable files, and zero hard failures,
process failures, panics, or hangs; the Scala oracle emitted all 1,236 files
with zero failures. The before/after analysis is in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-658-scala3-3.9.0.json
```

`parser-post-issue-684-scala3-3.9.0.json` measures `main` at merge revision
`3511201cf3f49109e02be6b6d8f1a1224ddcb6d1`, after PRs #672, #673, #678, and
#683. It uses the same pinned Scala 3.9.0 source revision and sorted 1,236-file
corpus. It records 1,188 clean parses (96.12%), 48 recoverable files, and zero
hard parser failures, process failures, panics, or hangs. The Scala oracle
emitted all 1,236 files with zero failures. Two runs produced byte-identical
JSON (SHA-256
`2d432adc1d1c96cb50578e37ae67701f06fd5c3fa69682f098c702cab8c2bc24`). The
comparison and interpretation are in
[`parser-v0.1-compatibility.md`](../../docs/parser-v0.1-compatibility.md).
Recreate it with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --output tools/parser-corpus-report/parser-post-issue-684-scala3-3.9.0.json
```
