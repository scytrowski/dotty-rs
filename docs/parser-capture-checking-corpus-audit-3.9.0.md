# Capture-checking corpus audit (Scala 3.9.0)

## Scope and method

This is a source-corpus reclassification, not a claim that capture-checking syntax is implemented or that enabling the parser feature flag changes current behavior.

- Scala sources: pinned Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`, the same sorted `library/src` + `compiler/src` corpus of 1,236 files used by the baseline.
- Parser source measured: dotty-rs `87b309d2f85e02323c4130a0ee6dae0626b4e5c7` (the parser source snapshot measured by issue #342 and recorded in PR #343). PR #343 added only the baseline report/docs; it did not change parser grammar.
- Import classifier: source files containing a direct import of `language.experimental.captureChecking`, either `import language.experimental.captureChecking` or `import scala.language.experimental.captureChecking`. This found 242 files (240 + 2); the other 994 files have no such import.
- Marker sub-classifier: among the 242 importing files, whether the source contains the ASCII character `^` anywhere. This is deliberately only a coarse lexical indicator; comments and string literals can match.
- The same corpus-report worker parsed the two cohorts independently. These runs use the parser's default feature policy. The Scala oracle was not rerun for the subsets.

## Results

| Cohort | Files | Clean | Recoverable | Clean rate | Scanner diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: |
| Imports capture checking | 242 | 109 | 133 | 45.0% | 0 |
| No capture-checking import | 994 | 649 | 345 | 65.3% | 13 |
| Total | 1,236 | 758 | 478 | 61.3% | 13 |

First-diagnostic buckets in the import cohort:

| First diagnostic | Files |
| --- | ---: |
| `ExpectedType` | 83 |
| `ExpectedToken` | 22 |
| `ExpectedExpression` | 15 |
| `UnexpectedToken` | 5 |
| Unsupported compound template self types | 7 |
| Unsupported interleaved type-parameter clauses | 1 |

The `ExpectedType` bucket is the strongest capture-syntax lead, but first-diagnostic buckets still identify only where parsing first failed; they are not a complete per-file root-cause attribution.

### Coarse `^` split within the import cohort

| Source marker | Files | Clean | Recoverable | Clean rate |
| --- | ---: | ---: | ---: | ---: |
| Contains `^` somewhere | 113 | 4 | 109 | 3.5% |
| No `^` character | 129 | 105 | 24 | 81.4% |

For the 113 marker files, first failures were `ExpectedType` in 75 files, `ExpectedToken` in 19, `ExpectedExpression` in 7, `UnexpectedToken` in 1, and unsupported compound self types in 7. The marker is not a syntax-aware parse: inspect/reduce examples before treating all 113 as capture-type failures.

Diagnostic occurrences (not affected-file counts) were 8,090 in the import cohort and 6,643 in the no-import cohort. They are highly cascade-sensitive and should not be used as a projected clean-parse gain.

## Feature-policy caveat

`ParserFeatures::capture_checking` exists, but no parser grammar production currently reads the flag; outside its declaration/default, the source references are tests. Therefore this audit cannot honestly report a parser “feature off vs feature on” delta. The cohorts are classified by source import, while parsing itself still uses the default feature policy.

This is consistent with the current implementation boundary: capture syntax remains unimplemented. Dotty 3.9.0 marks capture references/sets and the type `^` suffix as capture-checking grammar in [Parsers.scala](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Parsers.scala#L1672) and [the refined-type grammar](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/parsing/Parsers.scala#L1930). Its feature policy is tied to whether the compilation unit enables capture checking in [Feature.scala](https://github.com/scala/scala3/blob/3.9.0/compiler/src/dotty/tools/dotc/config/Feature.scala#L153).

## Recommendation

Capture-checking syntax is a substantial, evidence-backed candidate: 75 of the 113 files containing both the feature import and a `^` marker fail first in the type grammar, and only 4 of those 113 parse cleanly. This does **not** imply that implementing capture syntax would make 109 files clean; many contain other unsupported constructs and later errors.

A useful next increment should first make the corpus/parser feature policy reflect compilation-unit imports (or otherwise run a clearly feature-enabled mode), then implement and measure the pinned Dotty subset: capture references, capture sets, and the `^` type suffix. Keep the feature disabled for files that do not enable it, and report the before/after cohorts separately.
