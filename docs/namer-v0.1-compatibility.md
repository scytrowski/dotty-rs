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
was refreshed for issue #490 after PR #497; the JSON records the exact
dotty-rs and Scala source revisions.

## Results

The parser produced a compilation-unit AST for all 1,236 files. It parsed
1,113 without diagnostics and 123 with recoverable diagnostics. The namer ran
on all 1,236 ASTs: 1,229 succeeded (99.43%) and 7 returned typed errors
(0.57%). Of the successful runs, 116 named parser-recovered ASTs. All 7 typed
errors came from recovered ASTs; none occurred on a clean parse. There were no
namer invariant violations, transaction residue after failed calls, process
failures, panics, or hangs.

The namer errors fall into two groups:

| Error | Count | Assessment |
| --- | ---: | --- |
| `InvalidVisibilityQualifier` | 4 | Valid enclosing class or package qualifiers are not resolved yet. These are a namer gap tracked in [issue #283](https://github.com/scytrowski/dotty-rs/issues/283). |
| `MalformedAstShape` | 3 | Parser recovery supplied extension children outside the supported method/export forms. These failures originate in parser recovery. |

This classification is based on each failing file's parser diagnostics and
recovered AST; parser-recovery cases should be reevaluated after those syntax
forms are supported.

## Deferred source features

The checked-in JSON inventories source forms that require work beyond entering
source declarations in scopes. “Deferred” counts only occurrences in files
where naming succeeded; parser-blocked forms are listed separately. A
materialized occurrence means the current source semantic index contains a
symbol for the corresponding AST node. Synthetic API rows count source
declarations for which the namer does not yet synthesize those APIs.

| Feature | Occurrences | In files | Deferred after successful naming |
| --- | ---: | ---: | ---: |
| Enum definitions | 78 | 57 | 5 |
| Enum cases | 611 | 57 | 97 |
| Case class synthetic APIs | 659 | 157 | 650 |
| Export forwarders | 35 | 10 | 16 |
| Derives clauses | 135 | 25 | 134 |
| Local definitions inside method bodies | 34,927 | 730 | 33,763 |
| Source annotations | 4,031 | 563 | 3,991 |
| Context-bound evidence synthesis | 812 | 42 | 812 |

The local-definition inventory is computed from AST source spans contained in
method RHS spans. It is an inventory estimate, not a claim that every such
definition should become a scope member.

The report lists parser-blocked forms separately from Namer deferrals because
those forms did not reach the namer. Other parser-recovered input remains
included in the Namer run.

## Readiness

**Recommendation: the current source semantic index is suitable as input to
the first source typer milestone.** The audit reports zero semantic-index
invariant failures, and all successful runs preserve the package/scope and
symbol ownership relationships the typer can build on. The typer milestone
must account for the documented deferred synthetic members, exports, local
scopes, and annotations. Qualified visibility has a focused blocker in issue
#283, and the remaining 3 errors are tied to parser-recovered shapes. This is a
readiness decision for the first typer input contract, not a claim that the
Namer implements typing or all Scala 3.9 source syntax.
