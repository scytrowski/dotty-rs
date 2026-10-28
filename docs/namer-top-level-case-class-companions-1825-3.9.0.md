# Top-level case-class companion evidence (#1825)

Validated on `main` base `bd3c105a006b992bc2f1ded6db16245591506c04`, after
#1809's nested case-class support, against Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.

## Pinned roots

| Source root | Source declaration | Before top-level synthesis | After this change | Whole method |
| --- | --- | --- | --- | --- |
| `library/src/scala/Option.scala#tree=214` | `final case class Some[+A](value: A)` in package `scala` | `Some`: package-owned source `Class`, `CASE`, `ClassInfo::Missing`, no source companion; first blocker was `scala.Some` module identity collision | Canonical package-owned synthetic term `Some` and type `Some$`; `Some` ↔ `Some` links, `Some$` is owned by `scala`, and derived apply parameters follow source `[+A](value: A)`; first blocker is `IfBranchJoinUnsupported` | Blocked |
| `library/src/scala/util/Either.scala#tree=84` | `final case class Left[+A, +B](value: A)` in package `scala.util` | `Left`: package-owned source `Class`, `CASE`, `ClassInfo::Missing`, no source companion; first blocker was `scala.util.Left` module identity collision | Canonical package-owned synthetic term `Left` and type `Left$`; `Left` ↔ `Left` links, `Left$` is owned by `scala.util`, and derived apply parameters follow source `[+A, +B](value: A)`; first blocker is `ExtractorUnapplyNotFound` | Blocked |

In both roots the source class remains source-originated (`SourceId(0)`) and its
`ClassInfo` is still incomplete at Namer completion. The synthesized object and
module class are synthetic; the module class owns its declaration scope. The
class tree remains the canonical source mapping, and the module-class derived
mapping refers to that real class tree rather than an invented source tree.
The Scala classpath has `Some.tasty` and `Left.tasty` in `scala-library-3.9.0.jar`;
the loader continues to reject incomplete or incompatible source/classpath
identities instead of importing binary-only companion members.

After #1809, Namer synthesized companions only for case classes owned by a
source object `ModuleClass`; package-owned `Some` and `Left` did not enter that
path. The deterministic two-root method audit now advances past both module
identity collisions. It reports zero whole methods typed, so this is a
first-blocker advance only.

## Controls and validation

- `tools/typer-classpath-corpus-audit/top_level_case_class_companion_oracle.py`
  passed with pinned Scala 3.9.0 and JDK 21. It checks supported top-level
  `apply`, explicit source companion reuse, source-over-classpath case-class
  precedence, rejection of a binary-only companion member, and rejection of
  merging an unrelated same-named source class.
- Focused Namer tests cover one canonical package-owned object/module-class
  pair, namespaces/owners/scopes/origins, bidirectional links, source mappings,
  covariant type-parameter and value-parameter order, explicit companions,
  unsupported defaults/multiple/empty/repeated/by-name/bounded shapes,
  ambiguity rollback/retry, and independently named semantic stores.
- The focused typer test confirms an unsupported generated `apply` fails only
  when requested, while a sibling method still types and keeps its exact
  source-to-typed mapping.
- The pinned `Option.map` / `Either.swap` audit passed twice deterministically:
  0/2 fully typed, with first blockers `IfBranchJoinUnsupported` and
  `ExtractorUnapplyNotFound` respectively.
- The frozen 25-method cohort passed twice deterministically: 14/25 typed,
  11 blocked, 0 excluded, 0 not attempted.
- The full `dotty-classloader` suite passed (457 unit tests and all integration
  tests; ignored JDK profile tests remain opt-in). #1812 separately adds focused
  fail-closed and independent classpath-snapshot coverage; this PR makes no
  classloader merge-by-name change.

Reproduction commands:

```sh
cargo test -p dotty-namer --test parsed_unit top_level_case_class --locked
cargo test -p dotty-namer --test parsed_unit by_name_top_level_case_class --locked
cargo test -p dotty-typer --test typed_expressions deferred_top_level_case_class_apply --locked
python3 tools/typer-classpath-corpus-audit/top_level_case_class_companion_oracle.py
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/namer-top-level-case-class-companions-roots-1825-3.9.0.tsv \
  --verify-determinism target/issue-1825/root-results.tsv
cargo test -p dotty-namer --all-targets --locked
cargo test -p dotty-typer --all-targets --locked
cargo test -p dotty-classloader --all-targets --locked
```

Set `SCALA39_ROOT` to the pinned Scala checkout, `JAVA_HOME` to JDK 21,
`SCALA39_JDK_RELEASE=21`, and `COURSIER_MAVEN_ROOT` to the cached Maven root
when running the method audit. The frozen 25-method cohort used the same runner
and `--verify-determinism` with
`docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv`.
