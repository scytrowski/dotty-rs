# AGENTS.md

## Project overview

`dotty-rs` is an experimental implementation of Scala 3 compiler
infrastructure in Rust. It is under active development and is not a complete
Scala compiler or a drop-in replacement for `scalac`.

The repository is developed incrementally. The assigned GitHub issue or explicit
user request defines the scope of an implementation increment; avoid turning a
focused issue into a general compiler refactor or implementing adjacent roadmap
items speculatively.

For source-language and TASTy compatibility work, Scala 3.9.0 is the current
reference unless an issue explicitly changes that target. The repository's
compatibility fixtures and reports are pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. Do not silently adopt behavior
from a newer Scala release.

The project is pre-1.0. APIs and internal architecture may evolve, but changes
should still preserve existing invariants and avoid unnecessary churn.

## Current implementation status

The workspace includes a source frontend (lexer, parser, namer, and typer),
structural TASTy and class-file codecs, semantic TASTy unpickling and pickling,
classpath/JDK metadata loading, and a bounded JVM backend. The typer completes
signatures and selected expressions; it is no longer only a signature milestone.
These components remain incomplete Scala subsets with explicit unsupported cases.

`tests/jvm_source_compilation.rs` assembles the real frontend and backend in one
session and tests generated classes, including state, control flow, bounded
inheritance, reference joins, nested construction, and chained call receivers.
That assembly and artifact publication are test infrastructure, not a public
source compiler/session API or CLI. JVM class emission and semantic TASTy
pickling are separate output paths; do not assume combined Scala ABI emission.

The semantic pickler emits TASTy 28.9.0. Existing binary fixture manifests may
record older compatible minor versions. The current JVM backend emits class-file
major version 65 (Java 21); class-file decoding and JDK metadata support have
separate compatibility surfaces. Do not infer one component's support from
another's successful fixture.

## Workspace architecture

The workspace is intentionally split into small compiler components with
one-way dependency boundaries:

- `dotty-core` owns shared compiler contracts and semantic data structures:
  source locations, diagnostics, token contracts, names, IDs, symbols, scopes,
  types, semantic storage, the source semantic index, and phase-indexed ASTs.
- `dotty-lexer` tokenizes Scala source and produces the token contracts from
  `dotty-core`.
- `dotty-parser` is the incremental handwritten Scala 3 source parser. It
  consumes `dotty-core::TokenSource` rather than depending on the concrete
  lexer implementation and produces the shared untyped AST.
- `dotty-namer` enters source declarations into the shared semantic model. It
  operates on `dotty-core` trees and must not depend on parser-private types.
- `dotty-typer` completes source semantics and builds typed information on top
  of `dotty-core`. It consumes the shared source semantic index and must not
  depend on the namer crate. Unsupported or ambiguous semantics should be
  reported explicitly rather than guessed.
- `dotty-tasty` owns structural/lossless TASTy decoding and encoding.
- `dotty-tasty-unpickler` projects decoded TASTy into the shared
  `dotty-core` semantic model.
- `dotty-tasty-pickler` validates borrowed source/typed semantic plans and emits
  a bounded subset of TASTy through `dotty-tasty`. It depends in production only
  on `dotty-core` and `dotty-tasty`; source frontend dependencies are test-only.
- `dotty-classfile` owns JVM class-file decoding and encoding, symbolic bytecode
  construction, and class-file verification analysis.
- `dotty-classloader` owns classpath and binary loading concerns and combines
  TASTy/class-file information without moving those responsibilities into the
  source frontend.
- `dotty-jvm` validates already-typed compilation plans and lowers supported
  semantics to class files. Its production dependencies are `dotty-core` and
  `dotty-classfile`; it must not assemble or rerun the source frontend.
- the root `dotty` crate exposes selected compiler libraries and hosts
  cross-component integration tests. It does not re-export every workspace
  crate; the lexer, namer, JVM backend, and semantic pickler are used directly.
- `tools/` contains compatibility, corpus, oracle, and diagnostic tooling. It
  is part of the validation infrastructure, not production compiler logic.

### Boundary rules

Preserve these boundaries unless the issue explicitly requires an architectural
change:

1. `dotty-core` must remain foundational. It must not depend on the lexer,
   parser, TASTy codecs/adapters, class-file, classloader, namer, typer, or JVM
   backend implementations.
2. The parser must remain independent of the concrete lexer. Use the shared
   `TokenSource` contract.
3. The namer must consume the shared AST/semantic model rather than parser
   internals.
4. The typer must not parse source or decode TASTy/class files. Those are
   separate frontend/adapter responsibilities. It may consume source
   definitions and the target-independent `SymbolResolver` contract from
   `dotty-core`, but must not depend in production on `dotty-namer`.
5. TASTy and class-file crates must not invent parallel symbol/type models when
   the information belongs in `dotty-core`.
6. Prefer extending an existing shared abstraction over introducing a second
   representation for the same compiler concept.
7. Do not introduce reverse dependencies merely to reuse a convenience helper.
   Move genuinely shared logic to the lowest appropriate crate instead.
8. The production/build dependency closures of core, lexer, parser, namer, and
   typer must remain independent of `dotty-classfile`, `dotty-classloader`, and
   `dotty-jvm`. Platform details such as descriptors, JMOD paths, bytecode, and
   JVM internal names belong in adapters, not frontend contracts. Test-only
   adapter dependencies are allowed. `tools/check-frontend-boundaries.py`
   enforces this direction using Cargo metadata with all features enabled.
9. Backend and pickler plans borrow canonical session stores and exact typed
   roots. Preserve source-to-typed bindings and explicit parameter/member order;
   do not reconstruct identity from spelling or allocation order. Validation
   cannot authenticate foreign same-slot arena IDs, so callers must preserve
   the single-session invariant.

## Sources of truth

Before editing a component, inspect its implementation, focused tests, relevant
design/compatibility document, and the assigned issue.

Useful project documents include:

- `docs/parser-design-3.9.0.md`
- `docs/parser-v0.1-compatibility.md`
- `docs/namer-v0.1-compatibility.md`
- `docs/typer-v0.1-compatibility.md`
- `docs/dotty-core-design.md`
- `docs/tasty-format-3.9.0.md`
- `docs/tasty-semantic-unpickler.md`
- `docs/classloader.md`
- `docs/frontend-platform-boundaries.md`
- `docs/classfile-encoder.md`
- `docs/jvm-backend.md` and the focused `docs/jvm-*.md` contracts
- `docs/tasty-pickler-design.md`
- `docs/tasty-pickler-class-state-contract.md`
- `docs/tasty-pickler-oracle.md`
- `docs/testing-strategy.md` (TASTy decoder/fixture coverage)

Compatibility reports and profiling documents are revision-specific evidence,
not current coverage percentages. Read their recorded revisions and scopes;
prefer focused contracts and current regression tests for newly added behavior.

Some design documents record an earlier development stage and can lag behind
the implementation. When prose conflicts with current code and regression
tests, verify the intended behavior from the issue and current architecture,
then update the stale documentation if it is in scope. Do not preserve an
obsolete constraint merely because an old document says that a later component
does not exist yet.

For Scala compatibility questions, prefer the pinned Scala 3.9.0 sources,
specification, compiler-generated fixtures, and repository oracle/corpus tools
over assumptions about how Scala "probably" behaves.

## Implementation principles

### Keep increments narrow

- Implement the smallest coherent behavior that satisfies the issue.
- Avoid unrelated cleanup, renames, broad API redesigns, or speculative
  abstractions.
- Reuse existing arenas, IDs, interning, symbol tables, type representations,
  diagnostics, and AST nodes before creating new ones.
- If an adjacent unsupported case is discovered, add a focused regression test
  only when it is part of the issue; otherwise document or file it separately
  instead of expanding scope silently.

### Preserve compiler invariants

- Symbol, tree, type, scope, and source identity must use the repository's
  typed IDs rather than ad-hoc integers or name-based identity.
- Preserve ownership, scope, source-span, and origin information when lowering
  or projecting between representations.
- Do not silently manufacture semantic information merely to make a test pass.
  Unsupported, ambiguous, malformed, or incomplete inputs should remain
  observable through typed errors or diagnostics.
- Semantic operations that can partially mutate shared state must leave the
  store in a valid state when they fail. Use the existing checkpoint/rollback
  mechanisms where applicable.
- Keep behavior deterministic. Tests and semantic output must not depend on
  hash-map iteration order, filesystem traversal order, or allocation order
  unless that order is explicitly part of the contract.

### Treat external input as untrusted

Source files, TASTy files, class files, archives, and classpath data must not be
able to crash the library through ordinary malformed input.

- Return typed errors or diagnostics instead of panicking.
- Validate lengths, indexes, references, UTF-8 assumptions, recursion depth,
  and allocation sizes at the layer that introduces them.
- Keep explicit recursion/fuel limits where recursive compiler operations can
  otherwise become unbounded.
- Parser recovery must make progress; malformed source must not produce hangs
  or infinite recovery loops.

### Prefer correctness over speculative acceptance

The project intentionally implements Scala semantics incrementally.

- The parser may recover from unsupported syntax, but recovery must preserve
  useful spans and diagnostics.
- The namer should not guess symbol ownership or visibility when the semantic
  relationship is unknown.
- The typer should reject unsupported or ambiguous typing situations with a
  typed error rather than selecting a plausible-looking candidate.
- Binary decoders should preserve raw information when a higher-level
  interpretation is incomplete, where the format and existing API support
  lossless preservation.

## Testing requirements

Every behavior change must have regression coverage at the lowest useful layer.

### Focused tests first

- Add a small unit test for the exact new behavior or bug before relying on a
  corpus-wide or end-to-end test.
- Keep independently regressible behaviors independently observable.
- Assert exact diagnostics/errors, relevant source spans, symbol/type
  relationships, or decoded fields rather than only checking success/failure.
- Add cross-crate or fixture-based tests when the behavior crosses a component
  boundary.

### Component-specific validation

- Parser changes should cover successful parsing and relevant recovery/error
  behavior. Use the Scala parser oracle and corpus tools when the issue changes
  compatibility or corpus behavior.
- Namer changes should validate semantic-index invariants, ownership, scopes,
  rollback behavior, and parser-recovered inputs where relevant.
- Typer changes should test positive typing, ambiguity/unsupported cases, and
  source-to-TASTy semantic parity when the changed behavior is part of that
  compatibility surface.
- TASTy changes must preserve the distinction between wire/raw, structured,
  semantic, and file-level behavior. State explicitly whether a round trip is
  byte-for-byte, structural, or semantic.
- Class-file/classloader changes should use focused binary fixtures and test
  malformed inputs and origin/resolution behavior when applicable.
- JVM backend changes should cover canonical plan validation, exact descriptors,
  instructions and stack-map frames where relevant, deterministic output, and
  typed rejection without partial artifacts. Use the real source pipeline and
  mandatory JDK verification/execution gate for supported code generation.
- Semantic pickler changes should cover read-only validation, exact source/typed
  bindings, declaration/reference identity, deterministic output, limits, and
  failure without partial artifacts. A parseable Rust round trip is not Scala
  interoperability: use the pinned pickler oracle for changed producer shapes.
  Distinguish incomplete declaration skeletons from complete `PickledTasty`
  output and Scala-generated oracle fixtures from Rust-produced artifacts.

Corpus and compatibility metrics are evidence, not substitutes for focused
tests. A lower diagnostic count alone does not prove a parser change is
correct, and a parseable round-trip alone does not prove encoder equivalence.

Do not regenerate checked-in baselines or compatibility expectations merely to
make a failing test green. Regeneration must correspond to an intentional,
reviewable behavior change.

## Required checks

Use the toolchain pinned by `rust-toolchain.toml` (currently Rust 1.98.1 with
rustfmt and Clippy) for local checks. The general Rust CI job currently installs
stable; the JVM job explicitly invokes `+stable`. Python 3 is required for the
frontend boundary guard.

The general CI job in `.github/workflows/ci.yml` enforces:

```text
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

Run targeted crate/tests while iterating, then run the complete relevant checks
before considering the change finished.

CI also has a separate mandatory JDK 21 gate. Put `java`, `javac`, and `javap`
on `PATH` and reproduce it locally with:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

Without that environment variable, JDK-backed tests may skip external checks
when tools are missing. Passing ordinary Rust tests alone does not establish
that the JVM gate ran. Ordinary fixture tests need no Scala rebuild; explicit
Scala oracle runs require the checkout, JDK, and cached dependencies documented
by the corresponding tool.

Compatibility/oracle workflows under `.github/workflows/` perform additional
lexer, parser, TASTy, and full typer corpus validation. Use the corresponding
workflow/tooling when the issue touches those compatibility surfaces.

## Change workflow

1. Read the assigned issue or user request and inspect the affected implementation
   and tests.
2. Identify the owning crate and verify that the change respects dependency
   boundaries.
3. Reproduce the missing behavior or bug with the smallest useful test.
4. Implement the narrowest change that makes the behavior correct.
5. Run focused tests for the affected crate.
6. Run dependent/parity/corpus tests when a shared contract or compatibility
   surface changed.
7. Run the repository CI checks.
8. Update public API documentation, compatibility reports, or design documents
   when the change makes them materially stale.

Do not use a passing test suite as justification for broadening the issue. A
small, reviewable compiler increment with explicit unsupported behavior is
preferable to a larger implementation that guesses at semantics.
