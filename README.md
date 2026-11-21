# dotty-rs

Experimental Scala 3 compiler infrastructure for Rust applications.

`dotty-rs` provides a source frontend, a shared semantic model, TASTy and JVM
class-file codecs, classpath loading, and a bounded JVM backend. You can use
these components independently or assemble a pipeline with the root Rust
library, `dotty`. The caller owns the compiler state and controls the phases.

The project is under active development and preparing its first crates.io
release. APIs and supported language subsets may change. It is not a complete
Scala compiler or a drop-in replacement for `scalac`, and it has no public
source-compilation CLI.

## Quick start

Use a local checkout while the first release is being prepared. The examples
below use the project's pinned Rust **1.98.1** toolchain and need no JDK or
Scala installation.

```sh
rustup toolchain install 1.98.1 --profile minimal
git clone https://github.com/scytrowski/dotty-rs.git
cargo +1.98.1 new --bin dotty-example
cd dotty-example
cargo +1.98.1 add --path ../dotty-rs --rename dotty
```

The root Cargo package is `dotty-rs`; its Rust library and the dependency alias
used here are `dotty`. You can also depend on individual component packages.

### Parse Scala source

Replace `src/main.rs` with:

```rust
use dotty::SourceDiagnostic;
use dotty::core::{NameInterner, SourceId, SourceText};
use dotty::lexer::ContextualScanner;
use dotty::parser::parse_compilation_unit;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "class Answer:\n  def value: Int = 42\n";
    let scanner = ContextualScanner::new(text)?;
    if !scanner.diagnostics().is_empty() {
        return Err(format!("Lexer diagnostics: {:?}", scanner.diagnostics()).into());
    }

    let mut names = NameInterner::new();
    let parsed = parse_compilation_unit(
        SourceText::new(text)?,
        SourceId::from_index(0),
        scanner,
        &mut names,
    );
    for diagnostic in &parsed.diagnostics {
        let report = SourceDiagnostic::from_parser(diagnostic);
        eprintln!("{}: {}", report.code.unwrap_or("parser"), report.message);
    }
    if !parsed.diagnostics.is_empty() {
        return Err("Source contains parser diagnostics".into());
    }

    println!("Parsed Answer.scala");
    Ok(())
}
```

Run `cargo +1.98.1 run`. The result contains the untyped AST and its root.
Parsing can recover and return an AST alongside diagnostics; inspect both
lexer and parser diagnostics before proceeding. `SourceDiagnostic` exposes
the parser's stable issue code and source location, using UTF-8 byte offsets.
A successful parse does not establish that the source can be typed or emitted.

### Name declarations and type a method

Typing uses a bootstrapped semantic store and the index returned by naming.
The following example checks the signature and body of `Answer.value`; it
does not emit a class file.

<details>
<summary>Complete example: replace src/main.rs and run cargo +1.98.1 run</summary>

```rust
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, SourceText};
use dotty::lexer::ContextualScanner;
use dotty::namer::name_compilation_unit;
use dotty::parser::parse_compilation_unit;
use dotty::source_declarations::source_classes;
use dotty::typer::SourceTyper;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let text = "class Answer:\n  def value: Int = 42\n";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();

    let scanner = ContextualScanner::new(text)?;
    if !scanner.diagnostics().is_empty() {
        return Err(format!("Lexer diagnostics: {:?}", scanner.diagnostics()).into());
    }
    let parsed = parse_compilation_unit(SourceText::new(text)?, source, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(format!("Parser diagnostics: {:?}", parsed.diagnostics).into());
    }
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Answer.scala",
        &mut store,
        &mut packages,
    )?;
    let classes = source_classes(&parsed.ast, parsed.root, source, &index, &store)?;
    let class = classes.first().ok_or("Expected a source class")?;
    let method = class.methods().next().ok_or("Expected a source method")?;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let typed_method = typer.type_method_definition(method.tree)?;
    println!("Typed method: {typed_method:?}");
    Ok(())
}
```

</details>

Keep trees, source indexes, names, symbols, and types in the same session.
Allocate a distinct `SourceId` for each source in that session. Arena IDs do
not identify their originating store; mixing IDs from separate sessions can
violate compiler invariants. See the [core design](docs/dotty-core-design.md)
and [typer contract](docs/typer-v0.1-compatibility.md).

### Read a TASTy file

Replace `src/main.rs` with this independent example:

```rust
use dotty::tasty::TastyFile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("Pass a .tasty file path")?;
    let bytes = std::fs::read(path)?;
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes)?;
    let header = file.header();
    println!(
        "TASTy {}.{}.{}",
        header.major_version, header.minor_version, header.experimental_version
    );
    Ok(())
}
```

Run it against a checked-in fixture:

```sh
cargo +1.98.1 run -- \
  ../dotty-rs/crates/dotty-tasty/tests/fixtures/simple_def/SimpleDef.tasty
```

This validates the file structure for the Scala 3.9.0 format target. Semantic
unpickling is a separate operation. See the [TASTy API guide](docs/tasty-api.md)
for raw, structured, encoding, and explicit compatible-version APIs.

## Capabilities and limits

| Area | Current scope |
| --- | --- |
| Source frontend | Contextual lexing, handwritten parsing with recovery, declaration naming, signature completion, and selected expression/method typing. Unsupported or ambiguous semantics produce diagnostics or typed errors. |
| Shared semantics | Interned names, typed IDs, symbols, scopes, types, phase-indexed trees, source indexes, and a target-independent symbol resolver. |
| TASTy | Structural decoding/encoding, semantic unpickling into the shared model, and semantic pickling of a bounded source subset, including typed bodies and selected class state. |
| JVM binaries and classpaths | Class-file decoding/encoding, symbolic bytecode construction, verification analysis, directory/archive loading, and JDK module metadata. |
| JVM backend | Emission from validated, already-typed plans: selected primitive operations, control flow, fields, construction, calls, references, and bounded same-unit inheritance. |

The [source-to-JVM integration tests](tests/jvm_source_compilation.rs) assemble
the frontend and backend in one session. The root `jvm_tasty_artifacts` module
also emits validated in-memory `.class`/`.tasty` batches for specific audited
source shapes. These paths do not provide a general compiler driver or
general Scala ABI compatibility. The [JVM backend contract](docs/jvm-backend.md),
[binary interoperability record](docs/jvm-tasty-binary-interop.md),
[constructor-injection record](docs/jvm-tasty-constructor-injection.md), and
[reference-constructor record](docs/jvm-tasty-reference-constructor-calls.md)
describe the tested shapes, unsupported cases, and reproduction commands.

Compatibility targets Scala **3.9.0**, pinned to source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The semantic pickler emits TASTy
**28.9.0**; some binary fixture manifests record older compatible minor
versions. The JVM backend emits class-file major version **65** (Java 21).
Decoder and classpath support are separate compatibility surfaces. Historical
corpus reports apply to their recorded revisions and scopes.

## Workspace libraries

| Crate / Rust library | Responsibility |
| --- | --- |
| `dotty-rs` (Rust library `dotty`) | Selected re-exports, bounded pipeline helpers, paired-artifact coordination, and integration tests |
| [`dotty-core`](crates/dotty-core/) | Shared semantic model and compiler contracts |
| [`dotty-lexer`](crates/dotty-lexer/) | Scala source tokenization and contextual scanning |
| [`dotty-parser`](crates/dotty-parser/) | Parsing over the shared `TokenSource` contract |
| [`dotty-namer`](crates/dotty-namer/) | Declaration entry and source semantic indexing |
| [`dotty-typer`](crates/dotty-typer/) | Source signature completion and expression typing |
| [`dotty-tasty`](crates/dotty-tasty/) | Structural/lossless TASTy codec |
| [`dotty-tasty-unpickler`](crates/dotty-tasty-unpickler/) | TASTy-to-core semantic projection |
| [`dotty-tasty-pickler`](crates/dotty-tasty-pickler/) | Validated semantic plans to bounded TASTy output |
| [`dotty-classfile`](crates/dotty-classfile/) | JVM class-file codec, bytecode builder, and verification analysis |
| [`dotty-classloader`](crates/dotty-classloader/) | Classpath discovery and binary loading |
| [`dotty-jvm`](crates/dotty-jvm/) | Typed-semantic plans to JVM class files |
| [`dotty-jvm-tasty-contract`](crates/dotty-jvm-tasty-contract/) | Read-only consistency checks between JVM and TASTy plans |

The root library exposes `dotty::{core, lexer, parser, namer, typer, tasty,
tasty_unpickler, classfile, classloader, jvm}`. Use `dotty-tasty-pickler`
directly for semantic pickling. Frontend production dependencies remain
independent of the JVM adapters; see the
[dependency boundaries](docs/frontend-platform-boundaries.md).

Generate local API documentation from the repository root with
`cargo doc --workspace --no-deps --locked`. The root library's entry page is
`target/doc/dotty/index.html`.

## Build and validate

Use the toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml), with
rustfmt and Clippy, plus Python 3 for the dependency boundary check. From the
repository root, run the Rust CI checks:

```sh
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
git diff --check
```

Ordinary fixture tests use checked-in artifacts without rebuilding Scala.
JDK-backed tests can skip external verification when Java tools are missing.
To require that verification, put `java`, `javac`, and `javap` from **JDK
21.0.13** on `PATH` (the patch version is checked by some acceptance tests),
then run:

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --workspace --all-targets --locked
```

The [standalone compiler client](tests/fixtures/compiler-client/) and
[editor client](tests/fixtures/lsp-client/README.md) demonstrate public-API
integration in separate Cargo workspaces. Their checks run separately in
[CI](.github/workflows/ci.yml); they are validation fixtures, not distributed
compiler or language-server products. For the compiler client, with the same
JDK on `PATH`, run:

```sh
cargo test --manifest-path tests/fixtures/compiler-client/Cargo.toml --target-dir target --locked
```

Scala oracle and corpus runs also require their documented pinned Scala
checkout and dependency caches. Follow the tool-specific instructions before
regenerating artifacts or compatibility reports.

## Documentation and tools

Some design notes describe earlier development stages. Consult them alongside
the current API documentation and focused regression tests.

- **Frontend:** [parser design](docs/parser-design-3.9.0.md),
  [parser compatibility](docs/parser-v0.1-compatibility.md),
  [namer compatibility](docs/namer-v0.1-compatibility.md), and
  [typer compatibility](docs/typer-v0.1-compatibility.md).
- **Binary formats and loading:** [TASTy format](docs/tasty-format-3.9.0.md),
  [semantic unpickler](docs/tasty-semantic-unpickler.md),
  [semantic pickler](docs/tasty-pickler-design.md),
  [class-file encoder](docs/classfile-encoder.md), and
  [classloader](docs/classloader.md).
- **Interop evidence:** [multi-class audit](docs/jvm-tasty-multiclass.md),
  [cross-class acceptance](docs/jvm-tasty-cross-class.md), and
  [pickler oracle](docs/tasty-pickler-oracle.md).
- **Compatibility tools:** [lexer oracle](tools/scala-lexer-oracle/README.md),
  [parser oracle](tools/scala-parser-oracle/README.md), and
  [parser/namer corpus tooling](tools/parser-corpus-report/README.md).
- **Contributing:** [project instructions](AGENTS.md),
  [testing strategy](docs/testing-strategy.md), and
  [CI workflows](.github/workflows/).

## License

Licensed under the [Apache License 2.0](LICENSE).
