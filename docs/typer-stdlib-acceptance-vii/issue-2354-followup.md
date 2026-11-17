# Issue #2354 follow-up: constructor parameter identity

Implementation base: `94cba0df6077c1196b055f441eca471326d6ae27`.
The source input remains pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`.

## Authenticated boundary and behavior

Before the change, the unchanged `Predef.scala#tree=614` method failed at
`sequenceOfChars` tree 611 with `TermNameNotFound`. The canonical plain
constructor parameter is source tree 602, a direct class-owned private
`Parameter` with source type `scala.collection.IndexedSeq[Char]`. At the live
failure it was absent from lexical and local lookup buckets and had
`SymbolInfo::Missing`.

The typer now admits that canonical parameter only from its own or a
lexically enclosing source class context. It keeps the parameter out of class
member scope and uses the class source context when completing the parameter's
type, while preserving the existing identical declaration context on the
canonical parameter and its constructor-owned derived copy. Its typed
reference retains the canonical parameter identity and source span. The plain
parameter does not gain a public accessor. Existing `val` parameters continue
to resolve as fields.

The original `charAt` method now completes with result `Char`; its parameter
type retains the canonical `scala.collection.IndexedSeq` type constructor and
`Char` argument. This is an observed downstream result of parameter admission:
the implementation does not change `IndexedSeq.apply` or member resolution.

Focused controls cover plain and `val` parameters, method-parameter and local
shadowing, generic class type-parameter identity, nested enclosing-class
access, unrelated-class and duplicate-name failures, and a late type error
followed by a corrected same-session retry.

## Frozen 25-root cohorts

Both unchanged manifests were run twice independently with determinism checks.
The main cohort attempted 25/25 roots and typed 22 (baseline 21); the analysis
cohort attempted 25/25 and typed 17 (baseline 17). All prior successful roots
retain their exact results and source-tree mappings: 21/21 main and 17/17
analysis. The manifests remain byte-for-byte unchanged:

- Main: `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`
- Analysis: `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`

This result does not change the baseline's `PROVISIONAL` qualification; the
hosted Acceptance VI receipt remains unverified.

## Validation

- `cargo test --workspace --all-targets --locked` (JDK 21 on `PATH`)
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked`
- `python3 tools/check-frontend-boundaries.py`
- `git diff --check`

Focused ignored audit test
`captures_acceptance_vii_charat_lookup_before_rollback` now verifies the
original method's typed parameter target and exact applied type identities.
