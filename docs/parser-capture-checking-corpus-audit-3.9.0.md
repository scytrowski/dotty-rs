# Capture-checking corpus audit (Scala 3.9.0)

## Scope and method

This report compares parser behavior on the same sorted Scala 3.9.0 `library/src`
and `compiler/src` corpus before and after capture-checking type syntax landed.
It is a source-compatibility measurement, not a semantic capture-checking test.

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- Parser revision measured: `d2f710011606a2a72bdae001c0579de91ee356e8`.
- Corpus: 1,236 Scala files, identical to issue #342.
- The capture cohorts use the parser's effective `ParserFeatures` after
  compilation-unit imports have been processed: 242 files enabled capture
  checking and 994 remained disabled. This is not a source-text substring
  classifier.
- Capture syntax evidence separates raw `^` characters, lexer `^` operator
  tokens (comments and strings are excluded), and capture-specific source trees
  represented in the AST. Token hits not represented as capture nodes were
  inspected individually.
- The Scala parser oracle returned 1,236 results with the same 30 existing
  oracle exceptions. Its result count is a liveness check, not corpus-wide AST
  equality.

The machine-readable deterministic report is
[`parser-post-issue-348-scala3-3.9.0.json`](../tools/parser-corpus-report/parser-post-issue-348-scala3-3.9.0.json).

## Results

| Effective capture policy | Files | Clean | Recoverable | Clean rate | Scanner diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: |
| Enabled by compilation-unit import | 242 | 148 | 94 | 61.2% | 0 |
| Disabled | 994 | 649 | 345 | 65.3% | 13 |
| Total | 1,236 | 797 | 439 | 64.5% | 13 |

Compared with issue #342, clean files increased from 758 to 797 (+39), and
recoverable files fell from 478 to 439. All 39 additional clean parses are in
the capture-enabled cohort: that cohort moved from 109/242 clean to 148/242,
while the disabled cohort remained exactly 649/994 clean. This localizes the
observed improvement to files whose effective parser policy enables the new
syntax; it does not imply that every importing file uses capture syntax.

| Measure | Before capture syntax (#342) | After capture syntax (#348) | Change |
| --- | ---: | ---: | ---: |
| Total diagnostic occurrences | 14,733 | 10,468 | -4,265 (-29.0%) |
| Capture-enabled cohort diagnostics | 8,090 | 3,825 | -4,265 (-52.7%) |
| Hard failures / panics / hangs | 0 / 0 / 0 | 0 / 0 / 0 | unchanged |
| Scala oracle results / exceptions | 1,236 / 30 | 1,236 / 30 | unchanged |

The diagnostic-occurrence reduction is not a count of independent failures:
parser recovery can emit cascades, and successful earlier syntax changes how
far each file is parsed.

## Capture-syntax triage

Within the 242 capture-enabled files:

| Evidence | Files |
| --- | ---: |
| Raw `^` character anywhere | 113 |
| Lexer `^` operator token | 109 |
| Capture-specific syntax represented in the AST | 103 |

The four raw-only files (`annotation/retains.scala`, `caps/package.scala`,
`quoted/Quotes.scala`, and `util/Either.scala`) contain the character only in
comments or documentation examples. Of the six lexer-token files without a
capture-specific AST node, five use ordinary XOR or declare the Boolean/Int/
Long compile-time `infix type ^` operators. The remaining file,
`library/src/scala/collection/package.scala`, does contain actual capture
suffixes (`(... )^` and `C^{t}`), but its first parser diagnostic occurs
earlier at `object +:`: symbolic object names are not supported, so the parser
never reaches those signatures. This is an unrelated grammar blocker, not
evidence that the capture productions reject that source. Thus 103 is the
parser-confirmed count; source inspection confirms at least one additional
capture-using file whose parse is blocked earlier. The report includes paths
for these raw-only and unconfirmed-token cases so the audit can be reproduced.

The no-import cohort has 83 raw `^` hits, but only 21 lexer `^` tokens; none
produces capture-specific AST nodes because capture checking is effectively
disabled. This is why raw source grep would substantially overstate relevant
syntax.

## First failures and next targets

Overall first-failure counts are `ExpectedExpression` (171 files),
`ExpectedType` (108), `UnexpectedToken` (97), and `ExpectedToken` (54). The
disabled cohort accounts for 148 of the `ExpectedExpression` first failures,
so that remains the broadest grammar target. Within the capture-enabled cohort,
the leading first failure is `ExpectedType` (41), followed by
`ExpectedExpression` (23), `ExpectedToken` (19), and `UnexpectedToken` (10);
one file first fails on the known interleaved type/parameter-clause limitation.

Before this change, the importing cohort's leading `ExpectedType` bucket was
83 files. It is now 41. Meanwhile `ExpectedExpression` rose from 15 to 23 and
`UnexpectedToken` from 5 to 10; these are consistent with parsing progressing
past newly supported capture types and encountering later unsupported syntax,
not evidence by themselves of a regression. The seven prior compound-template
self-type first failures no longer appear in that cohort; first-failure counts
do not establish whether those files are now clean or merely fail later.

The next high-value work is to reduce representative `ExpectedExpression`
cases across the no-import cohort, then deep-dive the remaining capture-enabled
`ExpectedType` cases. Keep per-file transitions and syntax-specific examples
separate from aggregate diagnostic occurrences when evaluating those changes.
