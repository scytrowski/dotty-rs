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
  receivers; the bounded same-source
  `Reader.read(counter) = counter.get()` cross-class call; and a linked
  `CounterFactory.create` that constructs and returns the exact `Counter`
  reference type. The paired JVM/TASTy adapter also checks complete Rust linked
  batches for plain constructor-reference/private-field aliasing and for a
  three-class `Counter` → `CounterOwner` graph built through
  `CounterOwnerFactory.wrap`. Strict pinned Scala 3.9 gates consume Rust TASTy
  for single-class, independent two-class, Reader, factory,
  constructor-injection, and three-class reference-constructor cases. The
  reference-constructor client uses the Rust-only six-artifact batch, proves
  all three sidecars supply member lookup, and runs unchanged Rust classes on
  JDK 21; a separate oracle rebuilds Rust TASTy with `-from-tasty`. These are
  bounded features, not arbitrary Scala/JVM code generation.

The source-to-class-file pipeline is exercised in
[`tests/jvm_source_compilation.rs`](tests/jvm_source_compilation.rs), with JVM
verification and execution in CI. The opt-in root `dotty::jvm_tasty_artifacts`
library module emits an owned, decoded-and-validated Counter `.class`/`.tasty`
pair or a complete in-memory batch for the audited independent Counter/Gauge,
linked Counter/Reader, constructor-injected Counter/CounterOwner, and
Counter/CounterOwner/CounterOwnerFactory source shapes. Batch emission uses one
frontend session, links each class to the exact UUID from its TASTy header, and
returns nothing unless every artifact passes whole-batch validation. Test-only
publication stages complete batches
with handled-error cleanup. Strict pinned Scala 3.9.0 gates verify both
single-class, independent two-class, and cross-class TASTy-dependent lookup;
the gates compare artifact bytes across fresh sessions and execute unchanged
Rust classes under `java -Xverify:all`. These bounded library and fixture
paths are not a compiler driver/CLI or general Scala ABI support. See the
[binary interoperability record](docs/jvm-tasty-binary-interop.md) and
[independent multi-class audit](docs/jvm-tasty-multiclass.md) and
[cross-class acceptance record](docs/jvm-tasty-cross-class.md) for exact
versions, hashes, controls, and reproduction commands.

The constructor-injected `CounterOwner(counter)` batch is structurally checked
for all four Rust-produced files and their source, JVM ABI, and TASTy links.
Its strict JDK 21 runtime gate proves the original Rust `CounterOwner` stores
and reads the exact injected `Counter` instance. The pinned Scala 3.9 consumer
now resolves both classes from their Rust TASTy sidecars, compiles an
independent typed constructor client, and runs the unchanged Rust classes. The
strict #1759 release gate runs the Scala consumer on the emitted Rust batch,
checks constructor-parameter TASTy addresses, and verifies alias-batch
publication failures before promoting the four linked artifacts. See the
[constructor-injection release record](docs/jvm-tasty-constructor-injection.md)
for the pinned toolchain, outputs, and reproduction commands.

The separate `CounterOwnerFactory.wrap(counter)` fixture constructs an Owner
that retains the exact input Counter. Its final strict gate checks six linked
Rust artifacts, fresh-session byte equality, handled publication failures, a
JDK 21 identity runtime, and the independent pinned Scala 3.9 consumer. See the
[reference-constructor release record](docs/jvm-tasty-reference-constructor-calls.md)
for the ABI, hashes, diagnostics, unsupported forms, and reproduction commands.

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
| `dotty-jvm-tasty-contract` | Consistency validation between JVM and TASTy producer plans |
| `dotty-rs` (root package; library `dotty`) | Selected public re-exports, bounded paired-plan/artifact coordination (including linked Counter/CounterOwner composition), and integration tests |

An external compiler can depend on the `dotty-rs` package (library name `dotty`)
and assemble a source-to-JVM pipeline with
`dotty::{core, lexer, parser, namer, typer, jvm}`. The caller owns
the session stores, orders the phases and chooses where to write generated classes.
For direct access to the semantic pickler, depend on `dotty-tasty-pickler`.
Source frontend production dependencies remain independent of the JVM adapters. See the
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
  -p dotty-rs --test jvm_source_compilation \
  --test jvm_stateful_class_contract \
  --locked -- --nocapture
```

Scala oracle runs and corpus audits have additional pinned checkout, JDK, and
cached dependency requirements. Follow the tool-specific instructions before
regenerating fixtures or reports.

With JDK 21 on `PATH`, run the standalone client test to exercise the public
facade. Its Cargo workspace declares a single dependency on `dotty-rs`, aliased
as `dotty`. The test compiles two Scala sources in both orders, compares the class
files and runs a Java caller against the generated classes. The same client measures a full
in-memory compilation with one and four workers. See the
[dependent-source workload and phase timings](docs/compiler-client-timings.md).

```sh
cargo test --manifest-path tests/fixtures/compiler-client/Cargo.toml --target-dir target --locked
```

## Cargo packages

The root package is named `dotty-rs`; its Rust library remains `dotty`.
For an explicit dependency alias, use:

```toml
[dependencies]
dotty = { package = "dotty-rs", version = "0.1.0" }
```

The 13 library packages share version 0.1.0, edition 2024, Apache-2.0 licensing,
and a minimum supported Rust version of 1.98.1. Internal path dependencies also
declare registry versions. Development tools and standalone client fixtures
remain non-publishable.

Published packages contain source code, their README, and the complete license.
Integration tests, binary fixtures, compatibility reports and Scala oracle tools
remain in the repository and are validated from the checkout. The root package
also retains the three TASTy inspection/validation binaries in `src/bin`.

To verify the complete publication set without uploading anything, run:

```sh
cargo package --workspace --exclude dotty-parser-smoke-dump \
  --exclude dotty-parser-corpus-report --exclude tasty-pickler-oracle --locked
```

Cargo prepares the selected interdependent packages together, so their initial
publication can be checked before any component exists in the registry.
During local review, add `--allow-dirty` to inspect uncommitted manifest changes.

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
  [JVM backend](docs/jvm-backend.md),
  [reference-constructor release record](docs/jvm-tasty-reference-constructor-calls.md).
- [Lexer oracle](tools/scala-lexer-oracle/README.md),
  [parser oracle](tools/scala-parser-oracle/README.md),
  [parser/namer corpus tooling](tools/parser-corpus-report/README.md),
  [typer classpath audit](docs/typer-classpath-corpus-audit-3.9.0.md), and
  [pickler oracle](docs/tasty-pickler-oracle.md).
- [CI workflows](.github/workflows): Rust checks, mandatory JVM execution, and
  separate compatibility/oracle and full-corpus audit workflows.

## License

See [LICENSE](LICENSE).
