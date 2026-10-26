# dotty-rs

An experimental implementation of Scala 3 compiler infrastructure in Rust.

`dotty-rs` develops a source frontend, shared semantic model, TASTy codecs and
semantic adapters, classpath loading, and a bounded JVM backend. It is a set of
compiler libraries and validation tools, not a complete Scala compiler or a
drop-in replacement for `scalac`. There is no public source-compilation CLI.
APIs and supported language subsets are still evolving.

## Current capabilities

- **Source frontend:** contextual lexing, a handwritten parser with recovery,
  declaration naming, signature completion, and incremental expression typing.
  The typer supports selected references, calls, construction, primitive
  operations, blocks, local values and assignments, conditionals, and loops.
  Unsupported or ambiguous semantics remain explicit errors.
- **Shared semantics:** interned names, typed IDs, symbols, scopes, types,
  phase-indexed trees, source-to-semantic and source-to-typed indexes, and a
  target-independent symbol resolver contract.
- **TASTy:** structural decoding/encoding, semantic unpickling into the shared
  model, and a separate semantic pickler for a restricted source subset. The
  pickler emits real typed bodies, selected primitive control flow, and bounded
  private primitive class state. Scala-reader interoperability is tested for
  specific fixtures; this is not general Scala ABI compatibility.
- **JVM binaries and classpaths:** class-file decoding/encoding, symbolic bytecode
  construction and verification analysis, directory/archive loading, and JDK
  module metadata support.
- **JVM backend:** validation and emission from already-typed compilation plans.
  Supported cases include Int/Boolean operations, control flow, local and private
  field state, planned-class references, construction, calls, and bounded
  same-unit inheritance. Source integration tests also cover sibling reference
  joins, nested construction, reference field initialization, and chained call
  receivers, and the bounded same-source `Reader.read(counter) = counter.get()`
  cross-class call, and a linked `CounterFactory.create` that constructs and
  returns the exact `Counter` reference type. Strict pinned Scala 3.9 gates
  consume the Rust TASTy for single-class, independent two-class, Reader and
  factory cross-class cases; the factory consumer runs the unchanged Rust
  classes on JDK 21 and a separate oracle rebuilds Rust TASTy with
  `-from-tasty`. These are bounded features, not arbitrary Scala/JVM code
  generation.

The source-to-class-file pipeline is exercised in
[`tests/jvm_source_compilation.rs`](tests/jvm_source_compilation.rs), with JVM
verification and execution in CI. The opt-in root `dotty::jvm_tasty_artifacts`
library module emits an owned, decoded-and-validated Counter `.class`/`.tasty`
pair or a complete in-memory batch for the audited independent Counter/Gauge
and linked Counter/Reader source shapes. Batch emission uses one frontend
session, links each class to
the exact UUID from its TASTy header, and returns nothing unless every artifact
passes whole-batch validation. Test-only publication stages complete batches
with handled-error cleanup. Strict pinned Scala 3.9.0 gates verify both
single-class, independent two-class, and cross-class TASTy-dependent lookup;
the gates compare artifact bytes across fresh sessions and execute unchanged
Rust classes under `java -Xverify:all`. These bounded library and fixture
paths are not a compiler driver/CLI or general Scala ABI support. See the
[binary interoperability record](docs/jvm-tasty-binary-interop.md) and
[independent multi-class audit](docs/jvm-tasty-multiclass.md) and
[cross-class acceptance record](docs/jvm-tasty-cross-class.md) for exact
versions, hashes, controls, and reproduction commands.

Compatibility targets Scala **3.9.0**, pinned to source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The semantic pickler emits TASTy
**28.9.0**; binary compatibility fixtures may record other compatible minor
versions in their manifests. Current JVM backend output uses class-file major
version **65** (Java 21). Historical corpus reports describe their recorded
revisions and are not measurements of the current checkout.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `dotty-core` | Shared semantic model and frontend/backend contracts |
| `dotty-lexer` | Scala source tokenization and contextual scanning |
| `dotty-parser` | Source parsing over the shared `TokenSource` contract |
| `dotty-namer` | Declaration entry and source semantic indexing |
| `dotty-typer` | Source signature completion and expression typing |
| `dotty-tasty` | Structural/lossless TASTy codec |
| `dotty-tasty-unpickler` | TASTy-to-core semantic projection |
| `dotty-tasty-pickler` | Validated core/source semantics to bounded TASTy output |
| `dotty-classfile` | JVM class-file codec and bytecode builder |
| `dotty-classloader` | Classpath discovery and binary loading |
| `dotty-jvm` | Typed-semantic plans to JVM class files |
| `dotty` (root) | Selected public re-exports, bounded paired-plan/artifact coordination (including linked Counter/CounterOwner composition), and integration tests |

The root facade does not re-export every workspace crate: use crates such as
`dotty-lexer`, `dotty-namer`, `dotty-jvm`, and `dotty-tasty-pickler` directly when
assembling a pipeline. Source frontend production dependencies remain independent
of the JVM adapters; see the
[frontend/platform boundaries](docs/frontend-platform-boundaries.md).

## Build and validate

Use the Rust toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml)
(currently **1.98.1**, with rustfmt and Clippy), Cargo, and Python 3 for the
boundary check. Run from the repository root:

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

Ordinary Rust fixture tests use checked-in artifacts without rebuilding Scala.
JDK-backed tests may skip their external verification when Java tools are
unavailable. To require the JVM gate used by CI, install JDK 21 with `java`,
`javac`, and `javap` on `PATH`, then run:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

Scala oracle runs and corpus audits have additional pinned checkout, JDK, and
cached dependency requirements. Follow the tool-specific instructions before
regenerating fixtures or reports.

## Documentation and tools

- [Contributor/agent instructions](AGENTS.md): scope, dependency boundaries,
  invariants, and validation requirements.
- [Core design](docs/dotty-core-design.md),
  [parser design](docs/parser-design-3.9.0.md),
  [namer compatibility](docs/namer-v0.1-compatibility.md), and
  [typer compatibility](docs/typer-v0.1-compatibility.md).
- [TASTy format](docs/tasty-format-3.9.0.md),
  [semantic unpickler](docs/tasty-semantic-unpickler.md),
  [semantic pickler design](docs/tasty-pickler-design.md), and
  [class-state pickler contract](docs/tasty-pickler-class-state-contract.md).
- [Classloader](docs/classloader.md),
  [class-file encoder](docs/classfile-encoder.md), and
  [JVM backend](docs/jvm-backend.md).
- [Lexer oracle](tools/scala-lexer-oracle/README.md),
  [parser oracle](tools/scala-parser-oracle/README.md),
  [parser/namer corpus tooling](tools/parser-corpus-report/README.md),
  [typer classpath audit](docs/typer-classpath-corpus-audit-3.9.0.md), and
  [pickler oracle](docs/tasty-pickler-oracle.md).
- [CI workflows](.github/workflows): Rust checks, mandatory JVM execution, and
  separate compatibility/oracle and full-corpus audit workflows.

## License

See [LICENSE](LICENSE).
