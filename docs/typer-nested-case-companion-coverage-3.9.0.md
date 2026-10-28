# Nested case-class companion coverage (Scala 3.9.0)

This report records issue #1809's bounded Namer increment against the frozen
method cohorts. The reference compiler is Scala 3.9.0 at
`777528f19a58e794c9954a42f433373472ec57f8` (built as
`3.9.0-bin-SNAPSHOT-git-777528f`) and the oracle controls ran on JDK 21.0.8.
The implementation comparison starts at repository source SHA
`21ca7e0d6921166fda810897efa9658956042735`.

## Oracle controls

`tools/typer-classpath-corpus-audit/check_nested_case_companion_oracle.sh`
compiles the valid shapes and checks the exact diagnostic IDs for invalid
shapes. The controls passed at the pinned compiler revision. The fixtures and
expected classifications are listed in
[`typer-nested-case-companion-oracle-3.9.0.tsv`](typer-nested-case-companion-oracle-3.9.0.tsv).
The valid fixture covers empty, ordinary and defaulted clauses; multiple value
clauses; repeated parameters; private constructors; secondary constructors;
bounded and context-bound type parameters; abstract case classes; and an
explicit companion. Scala rejects by-name parameters, contextual-only clauses,
missing clauses, and a same-name term/class collision with E055, E144, E004,
and E083 respectively. The two multi-clause contextual-first controls also
produce E144.

## Frozen cohort result

The original method identities remain frozen: 369 package-term rows and 576
object-module rows, with no missing or extra methods after the change. Of the
39 rows previously blocked by `UnsupportedCaseClassCompanionShape`, 39 now pass
Namer and reach a later Typer stage (9 package-term, 30 object-module). None of
the 945 methods is fully typed in the current audit. Reaching Typer is a Namer
result, not evidence that the method body typed successfully. The Namer
records that the generated apply was deferred. Typer reports
`OverloadedSelectionDeferred` for calls through that companion, including when
an explicit apply overload exists, because the absent generated overload could
change selection. The guard also covers explicit type applications such as
`N.apply[Int](1)`, whose member selection happens before call resolution, and
unqualified `apply` calls inside the companion. A local `apply` binding remains
eligible when it shadows the companion member. A separate whole-method test
confirms that an empty ordinary clause still gets a usable zero-argument apply.

The per-method receipt is
[`typer-nested-case-companion-methods-3.9.0.tsv`](typer-nested-case-companion-methods-3.9.0.tsv).
It joins the before/after audits by selected method identity and records each
first blocker and typed status. The frozen Tier A cohort remains unchanged: 25
methods attempted, 14 typed, 11 blocked, and zero excluded; the required typed
cohort gate passed.

## Source evidence and implementation boundary

The frozen rows exercise defaulted nested case classes in
`compiler/src/dotty/tools/dotc/util/Signatures.scala` and
`compiler/src/dotty/tools/dotc/cc/CaptureSet.scala`, plus the curried nested
case class `ExportInfo` in
`compiler/src/dotty/tools/dotc/transform/sjs/PrepJSExports.scala`. The latter
has multiple value clauses. These are cases where the Namer can preserve the
canonical class/companion relationship and continue naming sibling members,
while the current subset lacks enough constructor metadata to safely build a
generated `apply`.

The Namer now defers that generated member for valid unsupported shapes and
preserves explicit companion members. It still rejects the oracle-confirmed
invalid forms. An empty single parameter clause remains supported. The focused
typer regression checks both that an unrelated sibling method types and that
constructing the deferred shape produces the explicit missing-apply error.

## Reproduction

With JDK 21, the pinned Scala checkout staged, and the audit classpath set up as
documented by `tools/typer-classpath-corpus-audit/README.md`, run the oracle
controls:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/jdk-21 \
tools/typer-classpath-corpus-audit/check_nested_case_companion_oracle.sh
```

Then run the serial `package-term` and `object-module` focused-cohort audits
using `tools/typer-classpath-corpus-audit/run` and its frozen package-term and
object-module method manifests. The tracked per-method receipt above preserves
the before/after comparison. These audits cover the selected source files; this
change did not run the hosted full 1,236-file corpus.
