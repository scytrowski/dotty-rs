# Current-package import profile for Scala 3.9.0

This report records the bounded reach of #1173 against the frozen #1128
import-prefix cohort. The source checkout is pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The available worker JDK was
25.0.4; the frozen profile used JDK 21.0.8, so these audit counts are a new
runtime snapshot and should not be treated as byte-for-byte comparable
classpath observations.

## Cohort movement

The frozen cohort has 2,394 weighted observations across 209 files, 30 target
keys, and 904 blocker origins. The current run advanced 68 baseline rows past
`ImportQualifierNotFound`; 2,326 retain that blocker. The current profile has
2,332 downstream first-segment observations across 193 files and 29 keys. The
six-row difference between those current observations and the unresolved
baseline rows is accounted for by six new observations outside the frozen
identity set.

The five package-qualified target families break down as follows:

| First segment | Weighted observations | Files | Distinct blocker origins | Advanced past first-segment lookup | Current unresolved first blocker | Final import symbol resolved |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| `core` | 1,330 | 107 | 491 | 6 | 1,324 `ImportQualifierNotFound`; 5 `SymbolResolution`; 1 `TypeNameNotFound` among advanced rows | 0 |
| `ast` | 212 | 22 | 81 | 4 | 208 `ImportQualifierNotFound`; 4 `SymbolResolution` among advanced rows | 81 |
| `transform` | 66 | 1 | 7 | 0 | 66 `ImportQualifierNotFound` | 0 |
| `util` | 6 | 3 | 4 | 0 | 6 `ImportQualifierNotFound` | 1 |
| `io` | 2 | 1 | 2 | 0 | 2 `ImportQualifierNotFound` | 0 |

The ten first-segment advances are all in the current package: six `core` and
four `ast`. The other 1,605 observations in these five families depend on an
enclosing package context. #1173 deliberately does not search those contexts;
that behavior belongs to the follow-up covering stacked package clauses.

Final-symbol resolution is reported separately from first-segment selection.
Resolving a package prefix alone is not counted as a successful import. The
audit preserves the exact final selector outcome for each weighted row in its
reconciliation output.

## Validation

- The pinned Scala 3.9 import oracle passed its positive and negative fixtures,
  including current-package priority, root fallback, and no root retry after a
  selected qualifier lacks the requested member.
- The one-pass typer audit used `tools/typer-classpath-corpus-audit`'s frozen
  cohort and completed in 16.7 seconds on this worker.
- A checked-in TASTy artifact compiled by Scala 3.9.0 was unpicked and used by
  Namer and SourceTyper to verify canonical package and class identities.

The audit output and reconciliation are produced by the ignored test
`pinned_scala39_import_prefix_typing_probe` in `local_definition_audit.rs`.
