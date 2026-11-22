# Documentation map

`dotty-rs` is experimental Scala 3 compiler infrastructure. Its source frontend,
binary codecs, semantic adapters and JVM backend implement bounded subsets;
successful parsing, decoding or encoding alone does not establish complete
Scala compilation or interoperability.

## Start with the library guides

The [project README](../README.md) explains installation and the current feature
boundary. The [facade guide](../src/guide.md) and component guides below are
included in crate-level rustdoc. Their runnable examples use self-contained
inputs and are tested by CI. API definitions in each owning crate document
errors, limits, ownership and caller obligations.

| Library | Guide | Design or contract |
| --- | --- | --- |
| `dotty-core` | [Session state and source positions](../crates/dotty-core/src/guide.md) | [Core design and historical rationale](dotty-core-design.md) |
| `dotty-lexer` | [Tokenization and diagnostics](../crates/dotty-lexer/src/guide.md) | [Lexer and scanner](lexer-design-3.9.0.md) |
| `dotty-parser` | [Parsing through the token contract](../crates/dotty-parser/src/guide.md) | [Parser design](parser-design-3.9.0.md) |
| `dotty-namer` | [Declaration entry](../crates/dotty-namer/src/guide.md) | [Naming evidence and supported increments](namer-v0.1-compatibility.md), [error taxonomy](namer-diagnostics-taxonomy.md) |
| `dotty-typer` | [Signature and expression typing](../crates/dotty-typer/src/guide.md) | [Typing contracts and parity evidence](typer-v0.1-compatibility.md) |
| `dotty-tasty` | [Structural container round trip](../crates/dotty-tasty/src/guide.md) | [Public API](tasty-api.md), [wire format](tasty-format-3.9.0.md) |
| `dotty-tasty-unpickler` | [Semantic entry](../crates/dotty-tasty-unpickler/src/guide.md) | [Semantic projection](tasty-semantic-unpickler.md), [error taxonomy](unpickler-error-taxonomy.md) |
| `dotty-tasty-pickler` | [Semantic plans and wire fragments](../crates/dotty-tasty-pickler/src/guide.md) | [Producer design](tasty-pickler-design.md), [class state](tasty-pickler-class-state-contract.md) |
| `dotty-classfile` | [Class-file construction and decoding](../crates/dotty-classfile/src/guide.md) | [Decoder format boundary](classfile-format-jdk25.md), [encoder and verification analysis](classfile-encoder.md), [JPMS metadata](jdk-module-metadata-design.md) |
| `dotty-classloader` | [Classpath lookup](../crates/dotty-classloader/src/guide.md) | [Loading and resolution](classloader.md), [shared TASTy session](classloader-semantic-tasty-session-contract.md), [error taxonomy](classloader-error-taxonomy.md) |
| `dotty-jvm` | [Backend input boundary](../crates/dotty-jvm/src/guide.md) | [Backend and focused lowering contracts](jvm-backend.md) |
| `dotty-jvm-tasty-contract` | [Artifact linkage](../crates/dotty-jvm-tasty-contract/src/guide.md) | [JVM/TASTy interoperability boundary](jvm-tasty-binary-interop.md) |

The root Cargo package is `dotty-rs`; its Rust library is named `dotty`.
[Frontend/platform boundaries](frontend-platform-boundaries.md) records the
dependency graph. There is no public source-compilation CLI or general compiler
driver. Source-to-output integration harnesses assemble the components in tests.

## Read compatibility evidence within its recorded scope

The reference Scala compiler is **3.9.0**, source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The semantic pickler emits TASTy
**28.9.0**; some pinned compiler fixtures contain compatible **28.8.0** bytes.
The JVM backend emits class-file major **65** (Java 21). The class-file decoder's
JDK 21–26 fixture coverage is a separate guarantee from backend output and
classpath metadata support.

Documents in this directory serve different purposes:

- **Guides and contracts** explain supported workflows and invariants. Follow
  their links to focused tests and API definitions for exact behavior.
- **Compatibility reports and acceptance receipts** record a particular
  repository revision, fixture set and denominator. This includes
  [parser](parser-v0.1-compatibility.md), [namer](namer-v0.1-compatibility.md),
  [typer](typer-v0.1-compatibility.md), `*-baseline-*`, `*-acceptance-*`, and
  `classloader-semantic-tasty-convergence*` reports. Some accumulate later
  contract sections; their earlier measurements remain historical evidence.
- **Profiles and timing reports**, including `*-profile-*`, `*-performance-*`
  and `ci-*` measurements, describe the recorded host, tools, cache state and
  revision. Their timings and blocker counts are not current release metrics.
- **Original design rationale** can contain pseudocode and completed plans.
  For example, sections 3–17 of the core design retain the foundation proposal;
  the lexer increment plan records implementation history.

Do not infer a current support percentage from an old report. Keep recorded
counts, revisions, hashes and TSV/JSON receipts intact when updating surrounding
prose. Regenerate evidence only for an intentional implementation or measurement
change. For current behavior, use the owning crate's implementation, runnable
guide and focused regression tests alongside the relevant contract.

## Validation and interoperability

Run the current workspace and documentation checks from the repository root:

```sh
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo test --workspace --doc --all-features --locked
git diff --check
```

Use the toolchain in [rust-toolchain.toml](../rust-toolchain.toml). The checked-in
general CI workflow uses Rust **1.98.1** and Temurin **21.0.13+9.0.LTS**; its
separate JVM job invokes `+stable`. With that JDK's `java`, `javac` and `javap` on
`PATH`, run the mandatory external verification/execution gate explicitly:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty-rs --test jvm_source_compilation \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

Without the environment variable, some JDK-backed tests can skip external
verification when tools are unavailable. `--all-targets` does not run doctests;
the separate `--doc` invocation is required. Library roots deny `missing_docs`,
and workspace rustdoc lints reject broken and private intra-doc links. These
checks do not establish documentation accuracy or validate Markdown links in
this directory.

The [TASTy testing strategy](testing-strategy.md) distinguishes raw byte
preservation, structural equivalence and semantic observations.
[Pickler oracle setup](tasty-pickler-oracle.md) and the
[Rust-produced interoperability gate](tasty-pickler-interoperability.md) explain
the pinned Scala checkout and cached artifacts needed for independent consumer
checks. Scala-generated fixtures, Rust round trips, JVM execution and Scala
consumption establish different guarantees. The
[workflow sources](../.github/workflows) define which gates actually run in CI.
