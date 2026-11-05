# Semantic TASTy convergence IV: one immutable constructor field

Status: private semantic class loading accepts the original Scala 3.9
`CtorWithVal(val x: Int)` fixture. The public/default legacy
`ClassLoader::load_class` route is unchanged.

## Frozen inputs and baseline

The accepted resources are the original checked-in Scala 3.9.0 fixtures
(compiler revision `777528f19a58e794c9954a42f433373472ec57f8`, TASTy 28.9):

| Input | SHA-256 |
| --- | --- |
| `Constructors.scala` | `ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` |
| `CtorWithVal.tasty` | `53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192` |
| `CtorPlain.tasty` | `0d2f01cc5444bda30ae6a920162b3eb89fd2a5a7aefc84a1e22bef8a35120ddb` |

The implementation starts from refreshed `main` at
`eedcf312b989c9e5003a26a7a2576124db3ff05b`. Before this change,
`validate_semantic_class_shape` rejected `CtorWithVal` with
`UnsupportedShape("the primary constructor must have one empty clause or one ordinary Int clause")`.
The original field audit established that TASTy contains a class-owned field
and a separate constructor-owned formal, and no separate encoded getter
definition. The generated JVM getter is outside this semantic contract; the
loader does not invent a getter symbol.

## Indexed roles and completed graph

Selection remains structural and name-agnostic. The reusable immutable-field
selector supplies the original AST addresses to the ordinary whole-class
selection; no fixture name or address is used by production logic. For the
pinned positive fixture, the audited addresses are:

| Role | AST address | Semantic kind and owner | Type/state |
| --- | ---: | --- | --- |
| Class | `TYPEDEF@4` | `Class`, canonical package owner | Complete class, parent is canonical `java.lang.Object` |
| Header `x` | `PARAM@9` | `Field`, owned by that class | Complete, public, immutable, canonical `scala.Int`, declared once in class scope |
| Primary constructor | `DEFDEF@28` | `Constructor`, owned by that class | Complete plain method, result is this exact class |
| Constructor `x` | `PARAM@31` | `Parameter`, owned by that constructor | Complete canonical `scala.Int` |

The field and constructor formal are distinct indexed `SymbolId`s despite
sharing a source name and type. The field is completed using its source index;
the constructor formal is checked against its own constructor owner. Both are
validated before the existing single-class transaction can publish
`identified`, resolved/cache, repository, or origin entries. The effective
constructor result remains the owning class even though its serialized result
tree is `Unit`.

## Transaction, retry and negative controls

The new field work uses the existing
`load_semantic_class_transaction`. A test injects a failure after the field,
class and constructor have completed. It verifies rollback of store/session/
package checkpoints, builtin/Object state, the field-bearing root, repository
and positive caches, while a previously committed `CtorPlain` stays usable.
Retry in the same loader selects the same original resource and commits one
field declaration.

The loader accepts only one ordinary primary-constructor `Int` formal and,
optionally, one immutable unannotated class-header `Int` field with the matching
name. It rejects extra header fields, extra class declarations/accessors,
non-plain or secondary constructor shapes, generic/type-parameter shapes,
wrong package ownership, malformed TASTy, duplicate prior class identity, and
missing `java.lang.Object` before success publication or with complete rollback.
There is no source-authentic mutable-header-field fixture in the checked-in
audited set; the existing flag-validation regression verifies that `MUTABLE`
is not accepted, without treating fabricated input as source provenance.

`Open`, `CtorPlain`, `CtorWithVal` and `PlainPrivate` are exercised sequentially
in both plain/field orders. Same-loader repeat requests preserve cached class
identity and do not duplicate declarations; normalized receipts agree across
independent stores. Existing frozen audit reports, fixture bytes, and the
legacy/default loading route remain unchanged.

## Validation

Implementation commit: `a10e4c53c1` (`classloader: load immutable constructor
val field`), based on refreshed `main` at
`eedcf312b989c9e5003a26a7a2576124db3ff05b`. A subsequent test-only cleanup
removes a singleton loop from the wrong-owner rollback regression.

Toolchain: Rust `1.98.1 (48a229cea 2026-09-01)` from the pinned toolchain and
Temurin JDK `21.0.13.0+9-tem` (`$JAVA_HOME/bin/{java,javac,javap}`).

Passed checks:

- `cargo check -p dotty-classloader --lib --locked`
- `cargo test -p dotty-classloader --locked -- --test-threads=1`
- `cargo test -p dotty-tasty-unpickler --locked -- --test-threads=1`
- `cargo fmt --all -- --check`
- `python3 tools/check-frontend-boundaries.py`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- `git diff --check`
- `cargo test --workspace --all-targets --locked -- --test-threads=1`
- `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty-classfile --test classfile_jvm_encoder -p dotty-jvm --lib -p dotty --test jvm_source_compilation --test jvm_stateful_class_contract --locked -- --nocapture --test-threads=1`

Both test commands passed. They used the pinned Rust 1.98.1 toolchain and
Temurin JDK 21.0.13. The JDK gate was run with explicit `java`, `javac` and
`javap` paths and workspace-local Java/Cargo temp storage because `/tmp` was
full during parallel repository work. Hosted CI is still pending on the PR.
