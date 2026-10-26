# Ordinary source method audit (Scala 3.9.0)

This audit measures source-defined, ordinary member `DefDef`s separately from
the historical local-method audit. A method is `typed` only when the Typer
completes its canonical Namer symbol and emits a real typed `DefDef` mapping,
typed parameter and result trees, and a typed RHS whose result conforms to the
completed signature. Successful RHS expression typing by itself is not enough.

## Frozen local-audit contract

Implementation baseline: `491c37c85fbff27c520300e4592393588dd09c9f`.
The audit keeps its historical `local_def_methods` / `typed_local_methods`
denominators unchanged; the ordinary-member selection and report are separate.
Before this work, the focused smoke selected three files and attempted 31 local
methods, with 0 typed. Its performance TSV SHA-256 was
`4f9209e2ed4ea5ebcc73b0c7dc3f171345df5e41c065670093ef29eb176465ec`.

The frozen #1566 `package-term` baseline remains 60 files, 369 local-method
rows, 158 enclosing attempts, and 0 fully typed methods, against the separate
3,297 historical denominator. The closeout report records normalized report
SHA-256 `8eb1142bdd70a88ace58d460343db02ddfa75a6076a970746c4483b425b58832`
and method TSV SHA-256
`00d2bec6b78fb78ab49139672e4f5b5e87fcea0014398c8c57b2199649466582`.
The frozen #1051 `object-module` cohort remains 576 rows across 77 files; its
frozen identity TSV SHA-256 is
`4c7d7e4bf87ef1e213b1d26b785cbc4998ba4d0eff1a699c26ccda2d469efaa4`.
The #1680 seven-file sample remains a separate historical measurement of 36
local methods and 0 typed. These counts do not estimate ordinary member-method
coverage.

These are frozen checked-in baseline artifacts, not reruns from this worktree.
This environment does not contain the pinned Scala checkout or its JDK/classpath
cache, so the three-file smoke and two frozen cohorts could not be re-executed
against this implementation here. The historical files were left untouched;
re-run the focused three-file smoke documented in
[`typer-focused-audit-3.9.0.md`](typer-focused-audit-3.9.0.md), plus
`--focused-cohort package-term` and `--focused-cohort object-module`, in a
configured audit environment before using the PR as a refreshed corpus
measurement.

The ordinary inventory walks source `DefDef` nodes and requires their exact
canonical source/tree identity from the Namer index. It selects methods owned
by source classes, traits, or module classes. Constructors, declarations
without RHS, synthetic methods/forwarders, extensions, noncanonical identities,
and nested local methods receive explicit `excluded::<reason>` rows when
requested. Overloads remain separate because each has its own `path#tree`.

## Run a bounded audit

Use a small manifest containing exact source-tree identities, one per line:

```text
library/src/scala/collection/SomeFile.scala#tree=1234
```

The runner validates paths, numeric IDs, duplicates, and the pinned source
checkout before building or running the audit. Rust validates that each target
is a real `DefDef`; Namer ownership and method shape determine whether it is
attempted or explicitly excluded. Files with no local `def` are included when
their ordinary member identity is selected.

```sh
SCALA39_ROOT=/path/to/scala-3.9.0 \
JAVA_HOME=/path/to/jdk-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/maven \
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest /path/to/small-method-selection.tsv \
  --verify-determinism \
  /tmp/ordinary-method-report.tsv
```

The source checkout must be at Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`; the runner uses the configured JDK
and Scala 3.9.0 classpath, serial execution, and compares two normalized TSV
reports when `--verify-determinism` is set. It accepts only a small explicit
selection; this workflow does not run the hosted full corpus.

Each request has exactly one status:

- `typed`: complete canonical member definition, signature, parameter/result
  trees, body, and conformance are represented in the real typed arena and
  source-to-typed index.
- `blocked::<error>`: an independent attempt failed; the first Typer error and
  its debug cause are retained.
- `not_attempted::<reason>`: parsing/naming could not reach the requested
  method.
- `excluded::<reason>`: the selected tree is a valid definition shape outside
  the ordinary-member cohort.

The report has a schema/version header, selected/attempted/typed/blocked/
excluded/not-attempted counts, then one sorted row per request:

`method, owner_path, method_name, line, range, modifiers, expected_result, complexity,
status, attempted_tree`.

`complexity` is the count of positioned source AST nodes inside the RHS range;
it is a selection aid, not a semantic complexity score. `expected_result` is
the source result-type slice, empty when the result is inferred. A typed
attempted tree must match the requested method tree.

## Fixture evidence and limits

[`MemberOnly.scala`](../crates/dotty-typer/tests/fixtures/ordinary-member-audit/MemberOnly.scala)
is the positive fixture: its object method has a declared `Int` result and a
literal RHS, with no local definitions. Typer unit tests also cover an inferred
result, a result-type error with rollback/retry, a trait member, and overloads
with distinct source-tree identities. The audit selector tests malformed and
duplicate selections; inventory tests cover abstract/no-RHS, nested, synthetic,
and module-class ownership exclusions. The runner's deterministic mode checks
byte-identical reports.

The current Typer API now exposes a narrow canonical member-definition entry
point and reuses the existing local-method typed-tree builder. This supports a
bounded subset of ordinary methods; unsupported RHS semantics remain blocked
with their actual Typer error. Parameter defaults are also explicitly blocked
until the Typer can preserve and type those default-expression trees; the audit
must not report such methods as complete merely because their parameter roots
were mapped. In particular, the fixture result is not a claim about the Scala
standard-library corpus. Re-run a small pinned standard library manifest
before drawing any corpus coverage conclusion.

Machine-readable fixture example: [`typer-ordinary-method-audit-sample-3.9.0.tsv`](typer-ordinary-method-audit-sample-3.9.0.tsv).
