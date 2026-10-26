# Minimal Scala 3.9 standard-library method cohort

This report records the bounded real-source cohort requested by #1705. Source
inputs are pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`; source scope is
`library/src/scala`. The inventory visited 649 files, parsed and named them
without diagnostics or file-level namer failures, and found 9,388 canonical
ordinary methods with bodies. The inventory was generated twice and matched
byte-for-byte (SHA-256
`db0e22b513e2582f8043dba1ce9f4405f7932627d39e3bb3289909cc9eae97c9`). The
large generated inventory is not committed.

The inventory excludes constructors, abstract/no-body members, extension
methods, synthetic forwarders, non-canonical definitions, and nested/non-class
owners with explicit reason counts. Of the 9,388 eligible methods, the current
profile ranks 1,790 A0, 3,269 A1, and 4,329 edge candidates.

## Ranking and frozen cohort

The deterministic profile records source span and owner, RHS AST node count and
depth, type/value parameter counts, explicit/inferred result syntax, defaults,
method modifiers, ordinary/contextual parameter syntax, file and local imports,
local values, nested methods/scopes, overload count, and reference syntax.
Its score is a triage aid:

```text
RHS positioned AST nodes
+ 4 * maximum positioned-node nesting depth
+ 8 * type parameters
+ 2 * value parameters
+ 3 * calls
+ 3 * selections
+ 10 * nested methods
+ 2 * nested scopes
+ 4 * (branches + 2 * matches + 2 * tries + 2 * closures
       + 2 * while loops + 2 * for comprehensions)
+ 12 * default parameters
+ 4 * inferred result type
+ min(source-file import count, 5)
+ 8 * same-owner same-name overloads
+ 100 * placeholder RHS (??? or NotImplementedError)
```

A0 is score 0–24, A1 is 25–55, and `edge` is above 55. Lambdas/functions,
parsed `try`, and `for` syntax are counted from their actual untyped phase
nodes. Contextual status comes from source parameter modifiers; method-level
`implicit`/`given` modifiers are recorded separately. Tie-break order is score,
source path, then tree ID.

`external_ref_syntax_candidates` in the TSV is explicitly a syntax proxy: it
counts identifier, selection, and selected type-syntax AST nodes in the method
span. It is not a count of externally resolved symbols. Dependency evidence is
recorded separately after the pinned compiler and dotty-rs audits: all eight
selected source files compiled with Scala 3.9.0; each selected method row says
whether dotty-rs completed it or where its first resolver/typing blocker
occurred. For an unselected candidate there is no verified dependency
completeness evidence, so the syntax score must not be read as a dependency
ranking. The A1 follow-up order below uses the observed blockers and shared
failure mechanism, rather than assuming a low syntax score guarantees success.

The frozen selection contains 25 unique real method tree identities across
eight files and multiple owners: 18 A0, three A1, and four edge controls. The
compact TSV at
[`typer-stdlib-minimal-method-cohort-3.9.0.tsv`](typer-stdlib-minimal-method-cohort-3.9.0.tsv)
records all candidate and audit fields, source/typed tree identities, expected
and actual result shapes, blockers, dependency evidence, and the ten required
typed rows. No synthetic example contributes to the cohort numerator.

## Scala oracle and baseline audit

The pinned Scala 3.9.0 compiler compiled the eight selected real source files
together with `-Yexplicit-nulls -Ystop-after:typer`. Its `-Vprint:typer`
output confirmed the result shapes recorded for the ten baseline-typed
methods. Generic method results are alpha-normalized as `typeparam#0`; the
`Success.get` result uses the owner-qualified semantic path
`scala.util.Success.T`, corresponding to the compiler's `Try.this.T` /
`Success.this.T` print shape. The checked-in positive oracle control exercises
generic identity, same-named members on different owners, and nested parameter
shadowing. The negative control assigns `Int` to `String` and fails with
compiler diagnostic E007. Existing dotty-rs tests separately cover selected
owner identity and parameter/local shadowing; these controls are validation,
not accepted cohort rows. The oracle compiles the actual eight Scala source
files together against the pinned compiler/library artifacts; it does not run
the complete Scala library module build.

The strict dotty-rs audit ran locally and serially twice with determinism
verification; the normalized reports matched (SHA-256
`e0c8a2a1b616ace9f339e8f89df458afdb926e1998fa03c90e47a6dce89ce228`). Total
wall time was 1.587 seconds. The measured baseline is dotty-rs commit
`bbd93722f66ce177788cdb8e63a147c510ddf74c`, Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala compiler/library 3.9.0, and
Temurin JDK `21.0.8+9`. The exact compiler artifact versions are pinned in
`tools/typer-classpath-corpus-audit/run`. No production typer code changed in
this issue.

| Measure | Result |
| --- | ---: |
| Selected / independent source-tree methods | 25 / 25 |
| Attempted / fully typed / blocked | 25 / 10 / 15 |
| Excluded / not attempted | 0 / 0 |
| Newly typed by #1705 | 0 |
| Target: at least one newly typed method | **UNMET** |

| Tier | Selected | Baseline typed | Blocked | Acceptance |
| --- | ---: | ---: | ---: | ---: |
| A0 | 18 | 10 | 8 | 55.6% |
| A1 | 3 | 0 | 3 | 0% |
| edge controls | 4 | 0 | 4 | 0% |
| Total | 25 | 10 | 15 | 40% |

The ten baseline successes have exact source `path#tree` and typed-tree IDs in
the TSV. Both serial audit outputs preserved those same mappings, and the
executable gate compares both IDs with the frozen manifest. “Fully
parsed” requires a canonical member symbol and complete signature, a typed
method subtree, parameter/result mappings, and a typed body mapping. A partial
RHS is not counted. Actual result shapes are emitted separately from source
annotations; blocked methods report `-` for an unavailable actual result.

First blockers group by independent method as follows: member lookup (6),
unannotated function-literal parameter (2), and one each of missing member,
ambiguous type name, expected-result conformance, application-argument
conformance, unstable selection prefix, classpath symbol resolution, and term
name lookup. The TSV preserves each method's exact blocker and source range.

## Ranked A1 follow-up

1. **Generic source-parent adaptation for `Either.left` and `Either.right`**
   (A1 tree IDs 19 and 31). Both hit the same `MemberLookup` /
   `ParentTypeAdaptation` failure because a source receiver has two generic
   arguments. This shared fix has the clearest reuse and could also affect the
   selected `Either.swap` edge control; test positive and negative receiver
   arity/conformance cases before broadening.
2. **Application-argument conformance for `Tuple.toArray`** (A1 tree ID 23).
   The selected method reaches `ApplicationArgumentConformanceUnsupported`
   after its dependencies compiled. This is a separate focused conformance
   gap; first prove the exact Scala array argument/result shape and preserve
   the existing mismatch rejection.

These are backlog directions only. #1705 made no semantic changes and does
not start either mechanism.

## Reproduction

Set `SCALA39_ROOT` to a checkout at the pin, `JAVA_HOME` to Temurin JDK 21,
`SCALA39_JDK_RELEASE=21`, and `COURSIER_MAVEN_ROOT` to the Maven cache containing
Scala 3.9.0 artifacts. Generate the all-source inventory twice and compare:

```sh
DOTTY_ORDINARY_CANDIDATE_INVENTORY=/tmp/ordinary-inventory.tsv \
  cargo test -p dotty-typer --test local_definition_audit \
  pinned_scala39_ordinary_member_candidate_inventory --locked -- --ignored
```

The compiler classpath below is the exact pinned artifact list constructed by
the audit runner. Compile all eight real selected files and print their typed
trees:

```sh
SCALA39_CLASSPATH="$(printf '%s:' \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar" \
  "$COURSIER_MAVEN_ROOT/org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar")"
mkdir -p /tmp/scala39-ordinary-oracle
SCALA="$SCALA39_ROOT/library/src/scala"
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yexplicit-nulls -Ystop-after:typer \
  -Vprint:typer -d /tmp/scala39-ordinary-oracle \
  "$SCALA/Option.scala" "$SCALA/util/Either.scala" \
  "$SCALA/util/Try.scala" "$SCALA/Tuple.scala" \
  "$SCALA/Enumeration.scala" "$SCALA/Predef.scala" \
  "$SCALA/Function1.scala" "$SCALA/collection/Iterator.scala"

# Positive identity/ownership/shadowing and negative type-error controls.
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yexplicit-nulls -Ystop-after:typer \
  -Vprint:typer -d /tmp/scala39-ordinary-oracle \
  tools/typer-classpath-corpus-audit/oracle-controls/OrdinaryPositive.scala
! java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yexplicit-nulls -Ystop-after:typer \
  -d /tmp/scala39-ordinary-oracle \
  tools/typer-classpath-corpus-audit/oracle-controls/OrdinaryNegative.scala
```

Run the strict dotty-rs audit and enforce the ten frozen typed methods:

```sh
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism /tmp/ordinary-cohort.tsv
```

The gate rejects missing/blocked required methods, source/typed-tree mapping
changes, and changed normalized result shapes. Pass, regression, missing-row,
typed-tree-mapping, and changed-shape cases are covered in
`tools/typer-classpath-corpus-audit/test_verify_ordinary_cohort.py`. The full
cohort audit is local and serial; no hosted full-corpus audit was run.
