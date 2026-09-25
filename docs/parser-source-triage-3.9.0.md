# Parser source-corpus root-cause triage

This investigation follows the Parser v0.1 source-compatibility audit in
[#194](https://github.com/scytrowski/dotty-rs/issues/194). It asks what real
syntax lies behind the generic first-diagnostic buckets; those bucket names
alone are not syntax classifications.

## Method and reference

The sample is from the pinned Scala 3.9.0 checkout at revision
`777528f19a58e794c9954a42f433373472ec57f8`, over the same 1,236 sorted
`library/src` and `compiler/src` files as the v0.1 report. For each requested
generic bucket, files were sampled at 20 evenly spaced indexes in the sorted
bucket (all 3 `ExpectedPattern` files were inspected). This is a deterministic
diagnostic sample, not a random estimate. A confirmed count below means
“observed in this sample”; it must not be extrapolated to the unsampled bucket.

The same 83 files were sent through the Scala 3.9 compilation-unit oracle in
one batch. Dotty emitted 79 normalized source trees and 4 `OracleFailure`
results (all four count among the 30 source-tree exceptions recorded in the
full-corpus report). The successfully emitted trees establish that the
corresponding Rust first errors are not Scala parser errors. The four
exceptional files are excluded from conclusions about Dotty tree behavior.

## Findings

| First diagnostic bucket | Full bucket | Sample | Confirmed roots in sample |
| --- | ---: | ---: | --- |
| `ExpectedExpression` | 243 files | 20 | Case clauses in ordinary `match` bodies: 8; case-lambda / partial-function bodies: 4; named `end` markers: 4; remaining 4 are heterogeneous expression/layout cases. |
| `ExpectedToken` | 215 files | 20 | Legacy import selector rename `=>`: 8; newline/indentation before `match` cases: 3; trailing parameter-clause comma: 1; remaining 8 span type/parameter forms and nested lambda argument syntax. |
| `UnexpectedToken` | 159 files | 20 | “Expected block statement separator”: 10; “expected template member separator”: 7; “expected top-level statement separator”: 3. At least 3 of the first group are at `}` after a complete final block expression; the rest include case/closure bodies and need separate context. |
| `ExpectedType` | 109 files | 20 | Heterogeneous type grammar: capture-checking `^` forms (2), symbolic/infix type names (4), plus by-name types, annotations, context-parameter annotations, and other distinct forms. No single ordinary type production explains the bucket. |
| `ExpectedPattern` | 3 files | 3 | One continued alternative after `|`; one character-literal alternative; one symbolic operator name in a value pattern. |

Representative source locations and minimal examples:

| Root cause | Sample evidence | Scala 3.9 behavior / current behavior | Classification and follow-up |
| --- | --- | --- | --- |
| Case-lambda / partial-function literal | 4/20 `ExpectedExpression`; `library/src/scala/Function5.scala`, `Function14.scala`, and compiler sources. | Dotty parses `{ case x => ... }`; Rust reports `ExpectedExpression` at `case` in this source form. Example: `List(1).map { case x => x }`. | Missing ordinary Scala syntax; high priority. [#223](https://github.com/scytrowski/dotty-rs/issues/223). |
| Match case-region layout/continuation | 8/20 `ExpectedExpression` samples stop at case clauses; 3/20 `ExpectedToken` samples reject the newline/indent after `match`, including `Resident.scala`, `Variances.scala`, and `HealType.scala`. | Dotty emits trees for the sampled files. Rust has focused match fixtures, so these real-source failures indicate a contextual/layout shape not covered by those fixtures, rather than absence of the whole match grammar. | Parser/scanner integration follow-up; high priority. [#229](https://github.com/scytrowski/dotty-rs/issues/229). |
| Named `end` markers | 4/20 `ExpectedExpression` samples fail at `end` in `DesugarEnums.scala`, `CheckShadowing.scala`, `Splicing.scala`, and `Objects.scala`. Separately, #194 counted 18 files in the generic top-level-expression diagnostic bucket, including `end SetupAPI`. | Dotty treats a named end marker as a layout terminator, not an expression; Rust currently reaches expression/top-level recovery at it. | Missing ordinary Scala 3 layout syntax; medium priority. [#226](https://github.com/scytrowski/dotty-rs/issues/226). |
| Legacy import rename arrow | 8/20 `ExpectedToken`, including `Definitions.scala`, `Plugins.scala`, `MessageRendering.scala`, and `Enumeration.scala`. | Dotty 3.9 parses `import java.lang.{String => JString}`; Rust expects a selector delimiter after the imported name. Modern `as` remains supported. | Deprecated but accepted compatibility syntax; medium priority. [#225](https://github.com/scytrowski/dotty-rs/issues/225). |
| Trailing comma in parameter clause | 1/20 `ExpectedToken`, `compiler/src/dotty/tools/MainGenericCompiler.scala`, at `)` after a trailing comma. | Dotty accepts `class C(x: Int,)`; Rust reports that another parameter was expected. | Missing ordinary Scala syntax; medium/low priority. [#228](https://github.com/scytrowski/dotty-rs/issues/228). |
| Final expression before block close | 10/20 `UnexpectedToken` diagnostics have the generic block-separator message; at least 3 are at `}` after the final expression in `BTypeLoader.scala`, `LazyVals.scala`, or `WeakHashSet.scala`. | Dotty accepts a block ending in an expression immediately followed by `}`. Rust reports a missing block separator at the closer in these cases. Minimal shape: `def f = { val x = 1; x }`. | Likely block-termination/recovery bug; verify scanner interaction. High priority. [#224](https://github.com/scytrowski/dotty-rs/issues/224). |
| Pattern start and alternative edge cases | All 3 `ExpectedPattern` files: `ScalaPrimitivesOps.scala` (line continuation after `|`), `GenericSignatureVisitor.scala` (`'B' | 'C'`), `StdNames.scala` (`val * : N`). | Dotty parses all three source forms; Rust rejects at the alternative/pattern start. The operator-name case is a value-pattern/definition issue, not the same root as literal patterns. | Missing ordinary pattern syntax; lower priority due low observed frequency. [#227](https://github.com/scytrowski/dotty-rs/issues/227), [#230](https://github.com/scytrowski/dotty-rs/issues/230). |

## Deferred or not yet independently classified

- Capture-checking type suffixes such as `T^` and annotations such as
  `@retainsCap` are feature-oriented syntax and remain an intentional
  compatibility boundary for this milestone; they are not ordinary-expression
  blockers. They should not be counted as accidental parser bugs.
- The remaining sampled `ExpectedType` cases are diverse (by-name parameter
  types, annotations in parameter/type positions, self types, symbolic type
  declarations, and type-pattern forms). This sample does not support merging
  them into one implementation issue. Triage them when a concrete syntax family
  is prioritized.
- `UnexpectedToken` is especially cascade-prone: the 10 block-separator
  messages do not mean 10 instances of one root cause. Only the three final
  expression-before-`}` cases are currently confirmed as the same candidate.
  Template-member and top-level separator groups remain heterogeneous.
- Other individually observed forms include infix type-operator names and
  type/member parameter modifiers. Their low sample counts do not justify
  treating them as blockers; add targeted work only when a representative
  source family is selected.

## Recommendation

Proceed with incremental namer work on the documented supported parser subset.
In parallel, prioritize the ordinary-syntax gaps with direct source evidence:
case-lambda literals, match-case layout, correct block termination, and named
`end` markers. Legacy import `=>` selectors and trailing parameter commas are
useful source-compatibility follow-ups. Keep capture-checking syntax explicitly
deferred. The remaining generic-diagnostic population needs targeted follow-up
sampling rather than implementation based on histogram labels alone.
