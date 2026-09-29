# Namer v0.1 source compatibility

This report measures source naming against the Scala 3.9.0 compiler and
library sources. It checks whether the parser's compilation-unit AST can be
named and whether the resulting source semantic index satisfies its structural
invariants. It does not claim that the namer performs typing, overload
resolution, or compiler desugaring.

The source semantic index lives in `dotty-core`, because both the namer and
typer consume the same contract. The namer populates that index.

## Reproduction

The corpus is the 1,236 Scala files under `library/src` and `compiler/src` in
Scala revision `777528f19a58e794c9954a42f433373472ec57f8`. Recreate the report
with:

```text
tools/parser-corpus-report/run /tmp/scala3-3.9.0 \
  --namer --skip-oracle \
  --output tools/parser-corpus-report/namer-v0.1-scala3-3.9.0.json
```

The runner verifies the pinned Scala revision and records the exact dotty-rs
revision. `--skip-oracle` skips only the sbt reference run; it does not affect
the Rust lexer, parser, or namer measurement.

## Results

The latest measurement is at dotty-rs revision `f42b3db` and Scala revision
`777528f`. The parser produced ASTs for all 1,236 files: 1,127 had no
recoverable diagnostics and 109 had diagnostics. There were zero hard parser
failures, process failures, panics, or hangs. The namer ran on all files and
succeeded on 1,229 (99.43%); seven returned typed errors. All seven errors were
on parser-recovered input. No semantic invariant failures or transaction
residue were found.

Compared with the previous checked-in report (parser revision `4cf7eb2`), the
corpus has 14 more clean parses and 14 fewer recovered parses. Namer success
and error counts are unchanged; 14 fewer successful Namer runs now require
parser recovery.

| Namer error | Count | Files | Classification |
| --- | ---: | --- | --- |
| `InvalidVisibilityQualifier` | 4 | `CaptureSet.scala`, `TypeComparer.scala`, `ProtoTypes.scala`, `Promise.scala` | Recovered ASTs contain qualified private forms whose owner cannot yet be resolved. |
| `MalformedAstShape` | 3 | `tpd.scala`, `QuoteMatcher.scala`, `NamedTuple.scala` | Parser recovery produced extension children outside the supported method/export forms. |

These are typed errors, not invariant failures. Their parser-recovered inputs
need reevaluation as the corresponding syntax support improves.

## Enum identity coverage

The audit distinguishes enum declarations, their class and companion identities,
and the three enum-case forms. It found 78 enum definitions; 69 enum class
identities and all 69 corresponding companion object/module-class pairs were
materialized. The AST contains 390 singleton cases (384 materialized), 130
comma-group singleton cases (116 materialized), and 91 parameterized cases (82
materialized).

Four enum definitions and 12 enum cases are classified as parser-blocked. The
four definitions were present as detached AST nodes in recovered files, outside
any containing `Template.body`; the Namer therefore had no owner context. A
clean nested-enum regression confirms that this case is distinct from a Namer
identity failure. The remaining unmatched enum occurrences are in unsuccessful
Namer runs or local scopes and are not counted as completed-run structural
failures. Synthetic enum APIs such as `values`, lookup methods, and `ordinal`
are later synthesis work, not enum identity failures.

## Export handoff

The source corpus contains 10 export keyword occurrences. Seven export sites
were recorded in source semantic metadata; none were blocked by parser
recovery. Two AST export sites occur inside method bodies, whose local scopes
are outside this Namer gate. The remaining unrecorded site occurs in a unit
where Namer returned a typed error. No typed export forwarders were
synthesized: that work belongs to a later typed phase. The report measures
syntax, semantic handoff, parser recovery, and forwarder synthesis separately.

## Remaining feature classification

Counts below describe the current corpus inventory. “Deferred” counts
occurrences in units where naming succeeded; parser-blocked syntax and failed
Namer runs remain separate.

| Feature | Occurrences / files | Deferred after successful naming | Classification |
| --- | ---: | ---: | --- |
| Local declarations inside method bodies | 34,995 / 730 | 33,831 | Namer scope/identity gap: method-local scopes are not entered by this gate. |
| Case-class synthetic APIs | 659 / 157 | 650 | Semantic synthesis/lowering gap. |
| Derives clauses | 135 / 25 | 134 | Semantic synthesis/lowering gap. |
| Context-bound evidence | 812 / 42 | 812 | Semantic synthesis/lowering gap. |
| Source annotations | 4,031 / 563 | 3,991 | Typer gap: syntax remains on AST declarations, but annotation semantics are not projected to symbols. |
| Package objects | Covered by regression | 0 known deferred | Supported Namer identity; no longer a parser blocker. |
| Enum synthetic APIs | Not counted by this identity gate | Later feature | Intentional synthesis work after enum identity entry. |
| Typed export forwarders | 10 export syntaxes | 0 synthesized here | Intentional later typed-phase work. |

The local-definition count is an inventory of declaration spans contained in
method RHS spans. It does not claim that every such declaration must become a
class or package scope member.

## Structural checks and readiness

For successful Namer runs, the corpus audit checks canonical and derived symbol
identities, declaration ownership and scopes, reciprocal companion links and
uniqueness, enum case owners, export site owners/contexts, source contexts,
duplicate identities, transaction rollback, and package/source-wrapper
identity reuse. All reported semantic invariant failures and transaction
residue counts are zero. Parser-recovered detached enum nodes are listed as
parser-blocked rather than charged as Namer invariant failures.

The current source semantic index is suitable for the first source typer
milestone, with the deferred local-scope, annotation, synthetic-member, and
forwarder work above kept explicit. This is a readiness statement about the
Namer-to-typer contract, not a claim of complete Scala 3.9 source support.
