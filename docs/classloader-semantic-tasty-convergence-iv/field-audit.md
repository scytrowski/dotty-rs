# Semantic TASTy convergence IV: constructor `val` field audit

Status: semantic TASTy and pinned Scala/JVM evidence recorded. No production
behavior changed.

## Frozen inputs and execution context

The probe reads the original checked-in files under
`crates/dotty-tasty-unpickler/tests/fixtures/semantic/` and does not regenerate
or replace them:

| Input | SHA-256 |
| --- | --- |
| `Constructors.scala` | `ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` |
| `CtorWithVal.tasty` | `53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192` |
| `CtorPlain.tasty` | `0d2f01cc5444bda30ae6a920162b3eb89fd2a5a7aefc84a1e22bef8a35120ddb` |

The source declarations are exactly `class CtorWithVal(val x: Int)` and
`class CtorPlain(x: Int)`. The fixture generator pins Scala 3.9.0, and the
repository's fixture provenance records compiler revision
`777528f19a58e794c9954a42f433373472ec57f8`; the parsed fixture header is TASTy
28.9. The semantic probe was run in the preserved local checkout at
`9341a1138326c6636af53dda0c28be13f1fab65c`. The audit branch originally
started from `c16a1270b761e25011c221025d51379444e05569`; before PR publication,
GitHub `main` advanced to `3c1755c84042411681ec7bf05045fe00ce1c9d41` with four
unrelated typer-fixture/documentation commits. The final patch was reapplied to
that refreshed base; those commits do not touch the audited classloader or
test paths.

The Rust tests ran with Rust `1.98.1` and Temurin JDK `21.0.13+9`. The semantic
resolver used the original TASTy resource and the JDK 21 `jmods` classpath for
`java.lang.Object`. The checked-in fixture set contains no generated
`CtorWithVal.class`, so the original `Constructors.scala` was compiled into an
isolated `/tmp` output directory using the Scala compiler built from the exact
pinned checkout at `777528f19a58e794c9954a42f433373472ec57f8`. The checkout
reports `git.hash=777528f` and compiler version `3.9.0-bin-SNAPSHOT`. Its build
uses its configured Scala `3.8.4` reference compiler only to bootstrap the
pinned compiler sources; the source-built `3.9.0-bin-SNAPSHOT` compiler was
the compiler that generated the classfiles. No cached 3.8.4 compiler output
was used as a fixture or substituted for the pinned source build.

The exact commands were `sbt 'scala3-bootstrapped/compile'` and
`sbt 'scala3-bootstrapped/scalac -d /tmp/issue2059-constructors-out <absolute-path-to-the-unchanged-Constructors.scala>'`.
The source input SHA-256 was rechecked as
`ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` before
compilation. Generated classfile hashes:

| Output | SHA-256 |
| --- | --- |
| `CtorWithVal.class` | `75465102b3bdfe25d1540f40c7323dfebd8a8b4f48fa44623055dc48341a1cc0` |
| `CtorPlain.class` | `4982610c9f8c63bdd54901ae3b530b63e5763fb73d6d54946d36093de185b19e` |

The temporary `.tasty` files emitted by this direct compiler invocation had
hashes `d95e5ce7a75d071a8e54a1953cd9808c9f68396da1447e4b77a38753e534af6a`
(`CtorWithVal`) and `27a67a8f99ac38bef9735c18f2b1d0b5d375cbaea2289639caf1a9d54fb5eedb`
(`CtorPlain`), which differ from the checked-in fixture hashes above. Those
temporary TASTy files were not used by the semantic probe; the original
checked-in TASTy bytes remained the inputs and were not regenerated or
replaced. The classfile inspection below is independent evidence from the
same original source compiled by the pinned source-built compiler.

## Original semantic roles

`authentic_plain_constructor_is_completed_repeatably_and_val_neighbor_is_distinct`
parses the original bytes, asserts the 28.9 header, enters symbols into a fresh
store after one `Definitions::bootstrap`, and discovers roles through the
original AST address index and entered symbol ownership. It independently
completes the class, primary constructor, class-owned field parameters, and
any class-owned method definitions at their indexed addresses. It checks the
field's class declaration-scope membership before serializing normalized
receipts. Two fresh `CtorWithVal` runs produce equal receipts; canonical symbol
identity is checked before normalization.

Observed `CtorWithVal` receipt:

| Role | AST address | Kind and owner | Visibility and flags | Type/state |
| --- | ---: | --- | --- | --- |
| Class | `TYPEDEF@4` | `Class`, package-owned | Public, no flags | Complete `ClassInfo`; one template parameter and one template stat |
| Header `x` | `PARAM@9` | `Field`, owned by `CtorWithVal` | Public, no flags (`MUTABLE` absent) | Complete immutable field, canonical `scala.Int`; in class declaration scope |
| Primary constructor | `DEFDEF@28` | `Constructor`, owned by `CtorWithVal` | Public, no flags | Complete plain one-parameter `Method`; effective result is this exact class |
| Constructor `x` | `PARAM@31` | `Parameter`, owned by `CtorWithVal.<init>` | Public, no flags | Complete parameter of canonical `scala.Int` |
| Same-name encoded accessor | — | No class-owned `DEFDEF` named `x` in the indexed TASTy tree | — | Not represented as a separate TASTy definition in this fixture |

The header field's `SymbolId` and the constructor formal's `SymbolId` are
distinct in each fresh store even though their spelling and type match. The
table records the addresses discovered from the fixture; the implementation
does not use these numbers to drive production behavior. `CtorPlain` remains
the control: `TYPEDEF@4`, `DEFDEF@30`, one ordinary `scala.Int` formal, and
repeatable fresh-store completion.

The source `val` is therefore represented in semantic TASTy as a class-owned
immutable field distinct from its constructor input. The absence of a separate
TASTy accessor definition does not mean Scala omits the JVM getter. `javap
-classpath /tmp/issue2059-constructors-out -p -c -v
me.cytrowski.tastyfixtures.semantic.CtorWithVal` on the classfile generated
above reports:

```text
private final int x;
  descriptor: I
  flags: ACC_PRIVATE, ACC_FINAL

public int x();
  descriptor: ()I
  flags: ACC_PUBLIC
  Code:
    0: aload_0
    1: getfield      // Field x:I
    4: ireturn
```

The generated getter reads the generated private-final backing field. The
same compiler invocation emits `CtorPlain` with only its public `(I)V`
constructor and no `x` field or getter. This establishes the physical JVM
roles for the source declarations, while the original `CtorWithVal.tasty`
still has no separate class-owned `DEFDEF` named `x`. The bytecode observation
does not establish a distinct semantic TASTy getter identity or a canonical
field/getter link; those must not be manufactured from the source spelling.

## Current private-loader boundary

The unchanged private loader rejects the authentic `CtorWithVal.tasty` at
structural validation, before semantic entry or transaction start, with the
first typed error:

```text
UnsupportedShape("the primary constructor must have one empty clause or one ordinary Int clause")
```

This is distinct from the older convergence III snapshot: after subsequent
constructor support, the current first rejection is the additional header
field shape. The test checks the actual returned error and snapshots the store,
loading session, package table, repository, and semantic publication cache.

`authentic_constructor_rejections_preserve_committed_ctor_plain_state` first
loads and commits authentic `CtorPlain` through the semantic loader with the
real JDK classpath. It then checks the `CtorWithVal` rejection, a wrong-owner
copy of authentic `CtorPlain` bytes, and truncated `CtorPlain` bytes. Each
failure leaves the store/session/package checkpoints unchanged, adds no
repository or semantic-cache entry for the rejected name, and leaves the
previously committed `CtorPlain` identity resolvable and reloadable. The
separate existing `authentic_regular_constructor_is_accepted_by_semantic_and_legacy_loaders`
test confirms that `CtorPlain` succeeds through both semantic and legacy paths.
The new rejection test also loads the same original `CtorWithVal.tasty` bytes
through the unchanged legacy loader with the real JDK 21 classpath and observes
a complete class symbol, while the semantic loader reports the structural
`UnsupportedShape` above. Thus legacy acceptance is measured separately from
semantic rejection; neither result is inferred from the other.

The checked-in semantic fixture set contains no verified neighboring `var`
constructor fixture, so this audit adds no synthetic mutable-field control.

## Bounded A2 projection contract

The evidence supports an A2 projection of exactly the original immutable,
class-owned `Field x: scala.Int`: preserve its canonical symbol identity,
class owner, public visibility, empty/non-mutable flags, completed type, and
class declaration-scope membership. Keep the constructor-owned `Parameter x`
as a distinct symbol owned by the constructor. Do not create an accessor
symbol based on the source spelling or assume a JVM ABI member from the TASTy
field.

The independent JVM observation establishes a public getter that reads the
backing field, but the original TASTy fixture does not encode a separate
same-name method definition or explicit field/getter identity link. A2 may
project the original immutable class-owned field exactly as described, retain
the constructor input as a separate symbol, and leave getter symbol/link
publication deferred. This audit does not authorize production changes or
broader field/accessor modeling.

## Validation performed

On the local `9341a113` checkout with isolated `CARGO_TARGET_DIR` and `TMPDIR`:

- `cargo test -p dotty-classloader --lib authentic_constructor_rejections_preserve_committed_ctor_plain_state --locked -- --nocapture` — passed with JDK 21.
- `cargo test -p dotty-classloader --test semantic_tasty_parity authentic_plain_constructor_is_completed_repeatably_and_val_neighbor_is_distinct --locked -- --nocapture` — passed.
- `cargo test -p dotty-classloader --locked -- --test-threads=1` — passed all unit and integration tests (serial execution avoids the host's file-descriptor limit).
- Both fixture hashes and the source hash above were measured directly; the test also pins and rechecks them.
- `sbt 'scala3-bootstrapped/compile'` from the pinned Scala checkout — passed; generated compiler metadata reported version `3.9.0-bin-SNAPSHOT` and `git.hash=777528f`.
- `sbt 'scala3-bootstrapped/scalac -d /tmp/issue2059-constructors-out <absolute-path-to-the-unchanged-Constructors.scala>'` — passed; output classfiles were inspected with JDK 21 `javap` and hashed above.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` — passed.
- `cargo test --workspace --all-targets --locked -- --test-threads=1` — passed with `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `RUSTFLAGS="-C debuginfo=0"` to fit the 15 GB temporary filesystem. The first normal-profile attempt exhausted `/tmp` during compilation; only the issue-local temporary target directory was removed before retrying.
- The required JDK gate (`DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test -p dotty-classfile --test classfile_jvm_encoder -p dotty-jvm --lib -p dotty --test jvm_source_compilation --test jvm_stateful_class_contract --locked -- --nocapture --test-threads=1`) — passed with Temurin JDK 21.0.13.

The pinned Scala 3.9.0 source-built compiler oracle generated and inspected the
temporary classfiles described above. All Rust checks listed above were also
rerun from a clean worktree based on refreshed `main` SHA
`3c1755c84042411681ec7bf05045fe00ce1c9d41`, with the final tests/docs patch
applied. Hosted CI on the PR will report the status for the published commit.
