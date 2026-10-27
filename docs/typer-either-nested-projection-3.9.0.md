# `Either.left` / `Either.right` nested projection audit (Scala 3.9.0)

**Result: BLOCKED / UNMET.** This audit did not make either method fully typed.
It identifies a source Namer prerequisite; it does not claim that `Either.left`
or `Either.right` has improved.

## Scope and pinned inputs

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala compiler/library: pinned 3.9.0 artifacts; JDK 21.
- dotty-rs base: `bf69a070380ff9e3a6a48dcaa1670b695c9cf159` (main after #1764).
- Exact roots: `library/src/scala/util/Either.scala#tree=19` (`left`) and
  `#tree=31` (`right`).
- No hosted full standard-library corpus audit was run.

## First blocker and provenance

On the exact 25-method Tier A cohort, `left` and `right` remain blocked at
source child trees 16 and 28, respectively. Both failures are term-namespace
`MemberNotFound` results while resolving `LeftProjection` / `RightProjection`
on the canonical `Either` receiver. The failure occurs before candidate
selection, generated `apply` lookup, argument inference, or result conformance.

The source Namer probe confirms that each nested declaration is entered as a
real `CASE Class` owned by `Either$`'s `ModuleClass`. It does not enter a
term-side symbol for the case class and does not link a companion. The focused
Namer regression test records this current behavior and ensures that merely
observing the nested class does not invent an unsupported term symbol.

## Scala 3.9.0 oracle

The checked-in oracle harness under
`tools/typer-classpath-corpus-audit/either-companion-oracle/` compiles the
minimal nested case-class shape and the focused negative controls against the
pinned Scala artifacts. Its typed tree shows `EitherLike.LeftProjection.apply`
and `RightProjection.apply` instantiated with both type parameters and called
with `this`. Dotty emits each nested case class's companion `Object` and
`ModuleClass` and a generated polymorphic `apply`.

The controls establish that same-spelled nested classes retain distinct
owners, a user companion overload can coexist with generated `apply`, and an
ordinary nested class is constructed with `new` rather than a synthesized
companion. Wrong arity/type, inaccessible constructor, and ambiguous overload
examples are rejected by dotc. These results rule out treating every nested
class as a term or fabricating an `apply` based only on its spelling.

## Cohort result

Two deterministic passes of the unchanged 25-method audit produced identical
normalized reports. Current main is **12/25 fully typed**; the complete
per-method result is checked in at
[`typer-either-nested-projection-3.9.0.tsv`](typer-either-nested-projection-3.9.0.tsv).
The checked-in TSV uses `-` in the final `typed_tree` field when a method has no typed tree, avoiding ambiguous trailing empty fields. The audit output itself is byte-identical to the #1764 PR-head report.

| Root | Result on this base | First failure / note |
| --- | --- | --- |
| `Either.scala#tree=19` `left` | blocked | Term `MemberNotFound`, child tree 16 |
| `Either.scala#tree=31` `right` | blocked | Term `MemberNotFound`, child tree 28 |
| `Either.scala#tree=84` `swap` | blocked | Separate external `scala.util.Left` module identity failure; outside this issue's projection path |

All 10 methods accepted before #1764 remain fully typed with the same source
tree identities and result shapes. Two numeric typed-tree arena IDs changed in
the frozen verifier (`Option.isDefined`: 3 to 9; `Option.knownSize`: 9 to 15)
because newly accepted `Option.isEmpty` is allocated earlier. This is an arena
ID shift, not a change in those methods' accepted status, source roots, or
oracle-result shapes. The frozen cohort manifest was not edited. #1764 also
added `Option.isEmpty` and `Option.nonEmpty`, yielding the current 12/25 total.

## Disposition and next bounded dependency

The requested target remains **UNMET/BLOCKED**. Supporting this call correctly
requires Namer to represent the canonical companion `Object` / `ModuleClass`
and synthesize the case-class `apply` signature with the source class's generic
parameters and constructor parameter identities. That support must preserve
ownership, explicit companion overloads, visibility, and failure rollback.
The existing code has no general source case-class companion synthesis; adding
one here would exceed this issue's narrow resolver scope and guessing a
term-side member would violate canonical symbol identity.

The bounded prerequisite is [#1773](https://github.com/scytrowski/dotty-rs/issues/1773), which covers canonical `apply` synthesis for a deliberately restricted nested source case-class shape. This
issue should be revisited only after that prerequisite establishes the needed
source symbols and focused negative tests. No constructor lowering, `unapply`,
`copy`, JVM work, or broad overload/type-inference work is included.

## Reproduction

From the repository root, use the task-local pinned oracle harness:

```sh
env PATH="$JDK21_HOME/bin:$PATH" JAVA_HOME="$JDK21_HOME" \
  SCALA39_JDK_RELEASE=21 SCALA39_ROOT="$SCALA39_ROOT" \
  COURSIER_MAVEN_ROOT="$COURSIER_MAVEN_ROOT" \
  DOTTY_EITHER_COMPANION_TMP_ROOT="$PWD/target/issue-1747/oracle-script" \
  tools/typer-classpath-corpus-audit/either-companion-oracle/run
```

The script verifies the pinned Scala source revision, JDK release, and SHA-256
manifest for all eight Scala artifacts; it retains compiler output under the
task-specific `target/issue-1747/oracle-script` directory. The audit reports
and run logs used for the two-pass 25-method comparison were retained under
`target/issue-1747/` during this work.

To regenerate the exact 25-method ordinary-member TSV and retain both runs,
from the dotty-rs repository root:

```sh
mkdir -p target/issue-1747/reports
env PATH="/home/vdrake6/.jdks/jdk-21/bin:$PATH" \
  JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
  SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source \
  SCALA39_JDK_RELEASE=21 \
  COURSIER_MAVEN_ROOT=/home/vdrake6/.codex/worktrees/bdf3/dotty-rs/target/worktrees/issue-1746/target/issue-1746/coursier/https/repo1.maven.org/maven2 \
  DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1747/audit-tmp-repro" \
  tools/typer-classpath-corpus-audit/run \
    --method-kind ordinary \
    --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
    --verify-determinism \
    target/issue-1747/reports/ordinary-reproduced.tsv
```

The manifest contains exactly 25 pinned `path#tree` identities. The audit
forces one Cargo build/test thread, runs the ordinary-method request twice,
compares the normalized reports internally, and writes retained logs and
intermediate TSVs below the task-specific audit directory. To compare the
outputs explicitly and validate the ten previously accepted roots:

```sh
run_dir="$(cat target/issue-1747/audit-tmp-repro/run-dir.txt)"
cmp "$run_dir/first.ordinary-report.tsv" "$run_dir/second.ordinary-report.tsv"
cmp target/issue-1747/reports/ordinary-reproduced.tsv \
  "$run_dir/first.ordinary-report.tsv"
python3 - <<'PY'
import csv
from pathlib import Path

cohort = list(csv.DictReader(
    Path("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv").open(), delimiter="\t"
))
lines = Path("target/issue-1747/reports/ordinary-reproduced.tsv").read_text().splitlines()
header = next(i for i, line in enumerate(lines) if line.startswith(
    "method\towner_path\tmethod_name\tline\trange\tmodifiers\texpected_result\t"
    "actual_result\tcomplexity\tstatus\tsource_tree\ttyped_tree"
))
report = {row["method"]: row for row in csv.DictReader(lines[header:], delimiter="\t")}
typed_tree_shifts = []
for row in cohort:
    if row["gate"] != "required_typed":
        continue
    actual = report[row["identity"]]
    assert actual["status"] == "typed"
    assert actual["source_tree"] == row["source_tree"]
    assert actual["actual_result"] == row["oracle_result"]
    if actual["typed_tree"] != row["typed_tree"]:
        typed_tree_shifts.append((row["identity"], row["typed_tree"], actual["typed_tree"]))
expected_shifts = [
    ("library/src/scala/Option.scala#tree=122", "3", "9"),
    ("library/src/scala/Option.scala#tree=129", "9", "15"),
]
assert typed_tree_shifts == expected_shifts, typed_tree_shifts
print("10 previously accepted roots remain typed with stable source IDs and result shapes.")
print("Expected typed-tree arena ID shifts:", typed_tree_shifts)
PY
```

Do not add `--require-typed-cohort` to this reproduction: its frozen verifier
also compares arena-local `typed_tree` numbers and therefore exits nonzero for
the two expected shifts above, despite those roots retaining typed status,
source-tree identity, and result shape. The explicit semantic-preservation
check above reports those shifts without changing the frozen manifest.

## Validation recorded for this audit PR

- `cargo fmt --all -- --check` — passed.
- `python3 tools/check-frontend-boundaries.py` — passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` — passed.
- `cargo test --workspace --all-targets --locked` — passed.
- The mandatory JDK 21 test gate was run with `DOTTY_CLASSFILE_REQUIRE_JDK=1` and the JDK 21 `bin` directory on `PATH`.
- Focused Namer integration test: `cargo test -p dotty-namer --test parsed_unit --locked` — 102 passed.
- Scala 3.9.0 case-companion oracle harness — passed; pinned artifacts and source revision verified.
- Both repeated cohort reports were byte-identical within each run and byte-identical to the retained current-main baseline; the #1764 pre-merge report also matched.
- `git diff --check` — passed.
