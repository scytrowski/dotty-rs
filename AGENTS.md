# AGENTS.md

## Project overview

`dotty-rs` is an experimental implementation of Scala 3 compiler
infrastructure in Rust. It is under active development and is not a complete
Scala compiler or a drop-in replacement for `scalac`.

The repository is developed incrementally. The assigned GitHub issue defines
the scope of an implementation increment; avoid turning a focused issue into a
general compiler refactor or implementing adjacent roadmap items speculatively.

For source-language and TASTy compatibility work, Scala 3.9.0 is the current
reference unless an issue explicitly changes that target. The repository's
compatibility fixtures and reports are pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. Do not silently adopt behavior
from a newer Scala release.

The project is pre-1.0. APIs and internal architecture may evolve, but changes
should still preserve existing invariants and avoid unnecessary churn.

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
- `dotty-classfile` owns JVM class-file decoding and encoding.
- `dotty-classloader` owns classpath and binary loading concerns and combines
  TASTy/class-file information without moving those responsibilities into the
  source frontend.
- the root `dotty` crate is the public facade over the compiler libraries.
- `tools/` contains compatibility, corpus, oracle, and diagnostic tooling. It
  is part of the validation infrastructure, not production compiler logic.

### Boundary rules

Preserve these boundaries unless the issue explicitly requires an architectural
change:

1. `dotty-core` must remain foundational. It must not depend on the lexer,
   parser, TASTy, class-file, classloader, namer, or typer implementations.
2. The parser must remain independent of the concrete lexer. Use the shared
   `TokenSource` contract.
3. The namer must consume the shared AST/semantic model rather than parser
   internals.
4. The typer must not parse source or decode TASTy/class files. Those are
   separate frontend/adapter responsibilities. It may consume source
   definitions from `dotty-core`, but must not depend on `dotty-namer`.
5. TASTy and class-file crates must not invent parallel symbol/type models when
   the information belongs in `dotty-core`.
6. Prefer extending an existing shared abstraction over introducing a second
   representation for the same compiler concept.
7. Do not introduce reverse dependencies merely to reuse a convenience helper.
   Move genuinely shared logic to the lowest appropriate crate instead.

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

Corpus and compatibility metrics are evidence, not substitutes for focused
tests. A lower diagnostic count alone does not prove a parser change is
correct, and a parseable round-trip alone does not prove encoder equivalence.

Do not regenerate checked-in baselines or compatibility expectations merely to
make a failing test green. Regeneration must correspond to an intentional,
reviewable behavior change.

## Required checks

The repository CI currently enforces:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

Run targeted crate/tests while iterating, then run the complete relevant checks
before considering the change finished.

Compatibility/oracle workflows under `.github/workflows/` may perform
additional validation for lexer, parser, or TASTy changes. Use the corresponding
workflow/tooling when the issue touches those compatibility surfaces.

## Change workflow

1. Read the assigned issue and inspect the affected implementation and tests.
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
