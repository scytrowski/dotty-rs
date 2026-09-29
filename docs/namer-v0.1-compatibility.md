# Namer v0.1 source compatibility

This report measures the source namer against the Scala 3.9.0 compiler and
library sources. It checks whether the parser's current compilation-unit AST
can be named and whether the resulting source semantic index satisfies its
structural invariants. It does not claim that the namer performs typing,
overload resolution, or compiler desugaring.

The source semantic index is defined in `dotty-core`, because both the namer
and typer use this contract. The namer remains responsible for populating the
index; its public API re-exports the core types for compatibility.

## Reproduction

The corpus is the 1,236 Scala files under `library/src` and `compiler/src` in
Scala revision `777528f19a58e794c9954a42f433373472ec57f8`. Recreate the checked-in
JSON report with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --namer --skip-oracle \
  --output tools/parser-corpus-report/namer-v0.1-scala3-3.9.0.json
```

The runner verifies that exact Scala revision. `--skip-oracle` omits the sbt
oracle batch; it does not affect the Rust parser or namer run. The JSON report
records the source revision and leaves oracle counts unset. The measurement
was made at the dotty-rs `#272` implementation branch before its PR merge.

## Results

The parser produced a compilation-unit AST for all 1,236 files. It parsed 503
without diagnostics and 733 with recoverable diagnostics. The namer ran on all
1,236 ASTs: 1,196 succeeded (96.76%) and 40 returned typed errors (3.24%). Of
the successful runs, 693 named parser-recovered ASTs. All 40 typed errors came
from recovered ASTs; none occurred on a clean parse. There were no namer
invariant violations, transaction residue after failed calls, process
failures, panics, or hangs.

The namer errors fall into three groups:

| Error | Count | Assessment |
| --- | ---: | --- |
| `InvalidVisibilityQualifier` | 14 | Valid enclosing class or package qualifiers are not resolved yet. These are a namer gap and are tracked in [issue #283](https://github.com/scytrowski/dotty-rs/issues/283). |
| `MalformedAstShape` | 16 | The parser recovered unsupported extension contents, so the namer received an extension child outside the supported method/export forms. These failures originate in parser recovery. |
| `UnsupportedGivenNameShape` | 10 | The parser recovery AST supplied `Error` or `New` shapes where anonymous-given naming expects a supported type shape. These failures originate in parser recovery. |

This classification is based on each failing file's parser diagnostics and
recovered AST; parser-recovery cases should be reevaluated after those syntax
forms are supported. The 14 qualified-visibility failures are actionable
Namer follow-up work, rather than being hidden among parser failures.

## Deferred source features

The checked-in JSON inventories source forms that require work beyond entering
source declarations in scopes. “Deferred” counts only occurrences in files
where naming succeeded; parser-blocked forms are listed separately. A
materialized occurrence means the current source semantic index contains a
symbol for the corresponding AST node. Synthetic API rows count source
declarations for which the namer does not yet synthesize those APIs.

| Feature | Occurrences | In files | Deferred after successful naming |
| --- | ---: | ---: | ---: |
| Enum definitions | 77 | 56 | 63 |
| Enum cases | 538 | 53 | 484 |
| Case class synthetic APIs | 622 | 153 | 551 |
| Export forwarders | 33 | 9 | 11 |
| Derives clauses | 135 | 25 | 131 |
| Local definitions inside method bodies | 34,902 | 695 | 29,579 |
| Source annotations | 2,881 | 540 | 2,742 |
| Context-bound evidence synthesis | 0 | 0 | 0 |

The local-definition inventory is computed from AST source spans contained in
method RHS spans. It is an inventory estimate, not a claim that every such
definition should become a scope member. Context-bound evidence syntax did
not occur in this source corpus.

The parser blocked 20 package-object declarations in 20 files. Those are
reported separately from Namer deferrals because no package-object AST
reached the namer. Other parser-recovered input remains included in the Namer
run, which makes the counts useful for finding gaps without attributing
parser diagnostics to Namer.

The highest-volume parser blockers are top-level expressions (7,172
diagnostics across 428 files; for example
`compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala`), legacy
implicit parameter clauses (775 across 118 files; for example
`compiler/src/dotty/tools/dotc/ast/Desugar.scala`), wildcard imports using `_`
(133 across 89 files; for example
`compiler/src/dotty/tools/dotc/core/Signature.scala`), and unsupported
extension contents (155 across 25 files; for example
`compiler/src/dotty/tools/dotc/Run.scala`). These figures count all parser
diagnostics, so one file can contribute multiple occurrences; the checked-in
JSON separately reports first-failure file counts.

## Readiness

**Recommendation: the current source semantic index is suitable as input to
the first source typer milestone.** The audit reports zero semantic-index
invariant failures, and all successful runs preserve the package/scope and
symbol ownership relationships the typer can build on. The typer milestone
must account for the documented deferred synthetic members, exports, local
scopes, and annotations. Qualified visibility has a focused blocker in issue
#283, and the other 26 errors are tied to parser-recovered shapes. This is a
readiness decision for the first typer input contract, not a claim that the
Namer implements typing or all Scala 3.9 source syntax.
