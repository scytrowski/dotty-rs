# Authentic generic-class semantic loading audit

This is the A1 baseline for `CtorGeneric[A](x: A)` on current main. It records
an unchanged Scala 3.9 fixture and measures the current single-root semantic
loader before any production generic-class work. No loader, resolver, core
model, public API, fixture, or historical report is changed by this audit.

## Frozen inputs and environment

- Repository base: `ed65fc4d5fdad078afcee4d15a2d25337636e9b7`.
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8` (Scala
  3.9.0); fixture format TASTy 28.9.
- Source: `crates/dotty-tasty-unpickler/tests/fixtures/semantic/Constructors.scala`.
- Authentic binary:
  `crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorGeneric.tasty`.
  SHA-256: `a95d3bd7267b9ef1abac1d575b749c88b3bdcaaa5f2b80df66d8191e6c12cf71`.
  The fixture is unchanged.
- Rust: `rustc 1.98.1 (48a229cea 2026-09-01)` and Cargo 1.98.1.
- JDK: Temurin 21.0.13+9-LTS at `$JAVA_HOME`; `java.base.jmod` SHA-256 is
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- Loader fixture order: semantic fixture directory, then `tasty_visibility`.
  The focused attempt selects `CtorGeneric` from the first entry and stops at
  the structural gate before external classpath resolution. Existing
  constructor controls use `$JAVA_HOME/jmods` after those fixture roots; the
  audit separately verifies that `$JAVA_HOME/jmods/java.base.jmod` exists.

## Addresses and semantic roles in this file

All offsets below come from this file's deep AST address index. The focused
test checks the tags and reference targets directly, and compares semantic
roles by address and owner rather than comparing arena IDs between stores.

| Address | Wire node / role |
| ---: | --- |
| 4 | top-level `TYPEDEF` for class `CtorGeneric` |
| 9 | class-header `TYPEPARAM` `A`, owned by the class |
| 12 | its `TYPEBOUNDS`; lower `TYPEREF` at 14 is `Nothing`, upper at 18 is `Any` |
| 48 | primary constructor `DEFDEF`, owned by class 4 |
| 51 | constructor-owned `TYPEPARAM` `A`; a separate symbol from address 9 |
| 60 | ordinary constructor formal `PARAM` `x`, owned by constructor 48 |
| 65 | `TYPEREFDIRECT` in `x`'s type; its target is the constructor `A` at 51 |

The important identity distinction is observable in the wire data: the class
header's `A` is at 9, while the canonical type reference in `x: A` targets the
constructor-owned `A` at 51. The ordinary unpickler enters each declaration by
its address and preserves those two owners.

## Measured first failures

The real single-root loader attempt returns this typed failure before entering
any class identity:

```text
UnsupportedShape("the class template must have no type parameters and one constructor declaration")
```

This is the current first failure and matches the previously assumed structural
gate. The selected fixture origin and no-mutation result are asserted by the
focused test. It is a structural rejection, before the loader's classpath
resolver or semantic completion transaction runs.

A second, independent `SemanticStore` with `Definitions::bootstrap` and an
ordinary `TastyUnpickler::new` session is also measured. `enter_symbols()`
passes and enters addresses 4, 9, 48, 51, and 60. With no name-based synthetic
Scala/JDK symbols installed, `complete_symbol(4)` returns:

```text
UnresolvedMember { address: 14, name: "Nothing", namespace: Type, ... }
```

`complete_symbol(48)` returns the same error. This is an external-name
resolution prerequisite in the standalone ordinary session, not the loader's
structural gate. The audit therefore does not claim that this run reaches the
class/constructor generic completion behavior after bounds are resolved.

## B1/B2 acceptance boundary for later work

B1 can admit only the measured shape: one top-level class `CtorGeneric`, the
indexed class-header parameter/bounds above, one primary constructor, the
constructor-owned type parameter and one ordinary formal, with the formal's
canonical `A` reference preserving address 51. Do not generalize this to a
second term clause, contextual constructor field, or inferred same-name
parameter identity.

B2 must use ordinary address-based entry/completion in one session and preserve
the class / constructor / formal / both type-parameter identities and owners.
The legitimate completion order must complete the class before its constructor,
while resolving `Nothing` and `Any` through the normal semantic/classpath
mechanism rather than synthetic name-based symbols. Class and constructor
completion must then expose `x` as referring to the constructor-owned `A`.
That completion behavior remains a future acceptance condition; this baseline
stops at the observed `Nothing` resolution failure.

The neighboring measurements pin the boundary and validation order:

| Fixture | Current semantic-loader result |
| --- | --- |
| `CtorGeneric` | `UnsupportedShape("the class template must have no type parameters and one constructor declaration")` |
| `CtorCurriedGeneric` | `UnsupportedShape("the class template may have at most one ordinary constructor parameter or immutable field")` |
| `CtorGenericContextOnly` | `UnsupportedShape("the selected class-header field must be immutable and unannotated")` |
| `CtorPlain` | Loads successfully from the same semantic fixture directory in the existing positive constructor control; nongeneric control |

The first two neighboring failures occur at different earlier structural
checks. They do not establish support for a partial generic shape. `CtorPlain`
remains the positive nongeneric control in the existing
`authentic_ctor_with_val_loads_and_unsupported_neighbors_preserve_state` test.

## Reproduction

Run from repository root with Rust 1.98.1, Temurin JDK 21, and the authentic
checked-in fixture:

```sh
export JAVA_HOME=/path/to/temurin-21
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR=/path/to/registered-managed-scratch
export TMP="$TMPDIR"
export TEMP="$TMPDIR"
sha256sum crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorGeneric.tasty
sha256sum "$JAVA_HOME/jmods/java.base.jmod"
cargo test -p dotty-classloader --lib \
  authentic_ctor_generic_audit_records_loader_and_ordinary_unpickler_boundary \
  --locked -- --nocapture --test-threads=1
```

The test reads the fixture through the shared classloader test helper, parses
its actual AST index, checks the direct reference bytes and semantic roles,
runs both measured paths, and asserts the two unsupported neighboring results.
The existing constructor control test verifies the positive `CtorPlain` load.
The audit does not regenerate or rewrite fixture data.

## Local validation

On the recorded base with Rust 1.98.1 and Temurin JDK 21.0.13:

- The focused audit test passed.
- `cargo test -p dotty-classloader --locked -- --test-threads=1` passed (563
  passed, 5 ignored).
- `cargo test -p dotty-tasty-unpickler --locked -- --test-threads=1` passed
  (646 passed, 1 ignored).
- `cargo test --workspace --all-targets --locked -- --test-threads=1` passed,
  including the JDK-backed tests enabled by the available JDK.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`,
  workspace Clippy with warnings denied, workspace rustdoc with warnings
  denied, and `git diff --check` passed.

The default parallel classloader crate run hit this host's 1,024 open-file
soft limit while existing tests opened JDK classpaths concurrently (554
passed, 9 failed, 5 ignored). The audit itself checks the JDK image without
opening an additional JDK classpath. Its single-threaded rerun passed; the full
workspace suite also passed single-threaded. This is recorded as a host
validation constraint, not a semantic failure for the audited fixture.
