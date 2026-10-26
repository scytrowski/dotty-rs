# Minimal Scala 3.9 standard-library method cohort

This report completes the bounded cohort measurement for #1705. It uses the
Scala 3.9.0 source revision pinned by the repository:
`777528f19a58e794c9954a42f433373472ec57f8`.

## Inventory

The inventory scans all 649 `.scala` files under `library/src/scala`, parses and
names each file, and records canonical ordinary member methods with a body.
Constructors, declarations without a right-hand side, extension methods, and
other excluded definitions are counted separately. The scan found 9,388
eligible methods, with no parse diagnostics and no file-level namer failures.
The generated inventory is
[`typer-stdlib-method-inventory-3.9.0.tsv`](typer-stdlib-method-inventory-3.9.0.tsv);
its SHA-256 is
`99b66d17313c68d5061a8d7646887eaeee0035903d1a76597c5338f755638ecb`.

The inventory score is a deterministic syntax-based ranking aid, not a claim
that a method is semantically easy. Its formula is:

```text
RHS positioned AST nodes
+ 4 * maximum positioned-node nesting depth
+ 8 * type parameters
+ 2 * value parameters
+ 3 * calls
+ 3 * selections
+ 10 * nested methods
+ 4 * (branches + 2 * matches + 2 * tries + 2 * closures + 2 * loops)
+ 12 * default parameters
+ 4 * inferred result type
+ min(source-file import count, 5)
+ 8 * same-owner same-name overloads
+ 100 * placeholder RHS (??? or NotImplementedError)
```

Tier A0 is score 0–24, A1 is 25–55, and `edge` is above 55. Import count is a
file-level proxy and overload count is syntactic metadata; the inventory does
not model every resolver dependency. The selected cohort was frozen after
reviewing the ranked candidates and checking the whole-method audit. It contains
25 methods across eight files: 18 A0, five A1, and two intentionally selected
`edge` controls. Exact source-tree identities are in
[`typer-stdlib-method-cohort-3.9.0.txt`](typer-stdlib-method-cohort-3.9.0.txt).

## Whole-method audit

The cohort audit ran locally and serially twice with determinism verification.
Both normalized reports matched. The checked-in report is
[`typer-stdlib-method-cohort-3.9.0.tsv`](typer-stdlib-method-cohort-3.9.0.tsv);
its SHA-256 is
`a4a0208054d20a04692a385c0b56a3e11cf79860537102f8f7fbbb639d5b7755`.

| Measure | Result |
| --- | ---: |
| Selected | 25 |
| Attempted | 25 |
| Fully typed | 10 |
| Blocked | 15 |
| Excluded / not attempted | 0 / 0 |
| Already typed on the issue base | 10 |
| Newly accepted by this issue | 0 |

“Fully typed” follows the existing ordinary-member audit contract: the
canonical member, completed signature, parameter and result trees, typed body,
and result conformance must all be represented. Partial RHS success does not
count. This change adds inventory and measurement infrastructure only; it does
not change typer behavior, so all ten typed methods are baseline results.

The 15 blockers include six member-lookup or parent-adaptation failures, two
unannotated function-literal parameter failures, and isolated unstable
selection, classpath symbol loading, member resolution, ambiguous type name,
conformance, and term-name failures. For example, the `Enumeration.ValueSet`
row reaches a missing `scala/ImpureFunction1` dependency in the configured
classpath. These outcomes span distinct resolver and typing gaps, so this
increment records them without starting a broader typer mechanism.

## Reproduction

Set `SCALA39_ROOT` to a clean checkout at the pinned revision, `JAVA_HOME` to
JDK 21, `SCALA39_JDK_RELEASE=21`, and `COURSIER_MAVEN_ROOT` to a cache containing
the Scala 3.9.0 runtime jars. Then run:

```sh
SCALA39_ROOT=/path/to/scala39 \
JAVA_HOME=/path/to/jdk-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/maven-cache \
DOTTY_ORDINARY_CANDIDATE_INVENTORY=/tmp/ordinary-inventory.tsv \
cargo test -p dotty-typer --test local_definition_audit \
  pinned_scala39_ordinary_member_candidate_inventory --locked -- --ignored

SCALA39_ROOT=/path/to/scala39 \
JAVA_HOME=/path/to/jdk-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/maven-cache \
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-method-cohort-3.9.0.txt \
  --verify-determinism /tmp/ordinary-cohort.tsv
```

The inventory is regenerated from the pinned source, not from a checked-in
Scala build output. The focused audit covers only the frozen 25-method
selection; it does not run a hosted full-corpus audit.
