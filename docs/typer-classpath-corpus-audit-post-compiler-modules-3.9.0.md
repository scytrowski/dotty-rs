# Post-compiler-module typer audit checkpoint (#1342)

## Inputs and run method

The audit used Scala source revision `777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and Temurin JDK 21.0.8. The full corpus was run twice, serially, with `--verify-determinism`; normalized reports and frozen sidecars compared byte-for-byte. The runner recorded 1,455 seconds for both passes together. Peak RSS was not captured, so no memory claim is made.

Both passes processed 1,236 source files with zero parser, namer, or file-read failures. The unchanged local-method denominator is 3,297 and fully typed methods remain 0. Deferred-loading p95 depth was 0 (limit 6); maximum completed-class fanout was 9 (limit 16). The external classloader gate retained all 29 historical successful class keys.

## Refreshed semantic blockers

Counts below are first-blocker method observations and affected source files, not independent source declarations:

| First blocker | Observations | Files |
| --- | ---: | ---: |
| `SymbolResolution` | 2,534 | 215 |
| `TypeNameNotFound` | 186 | 64 |
| `ImportQualifierNotFound` | 180 | 19 |
| `TermNameNotFound` | 65 | 23 |
| `MemberLookup` | 44 | 20 |
| `ObjectModuleClassUnavailable` | 27 | 3 |
| `AnonymousClassInstantiationDeferred` | 32 | 15 |

The resolver observed 1,389,149 package-term requests, 469,642 canonical Object results across 127 external Object identities, 20 completed external ModuleClasses, and 144 external module-member successes across 25 distinct member identities. It recorded 8,949 `InvalidTastyFile::UnresolvedSupertype` member errors, across 279 files and 50 distinct requested class roots. The repeated error count is not a distinct-root count.

## Frozen import reconciliation

The post-#1180 sidecar joins all 1,063 observations, 31 target keys, 81 files, and 393 original blocker origins. It records each row's original source/method/enclosing-attempt/import identity, original failed qualifier and outcome, target classification, current qualifier identity and kind, current blocker/outcome, and whether the source method is fully typed. Target-specific classpath resource evidence remains in the companion #1180 classpath-evidence TSV, referenced by stable target key.

Of these rows, 1,034 currently identify an Object qualifier across 23 canonical identities; 25 remain at the same unresolved import blocker, and 1,038 have moved to a later blocker. None of the 1,063 methods is fully typed. Fourteen of the 23 current Object identities match the post-audit #1051 ModuleClass profile. Its completion and member outcomes are preserved as attempts; `rolled_back` work is not treated as committed semantic success.

The row-level reconciliation is [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-current-reconciliation.tsv`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-current-reconciliation.tsv), with its counts in [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-summary.txt`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-summary.txt). The complete 576-row Object ModuleClass reprofile is [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv).

## Next sprint recommendation

Recommend exactly one next slice: **TASTy unresolved-supertype root resolution and compatibility classification**. It is now the broadest measured cross-corpus cause by distinct requested roots (50) and affected files (279), while retaining the repeated 8,949 observations as a separate weighted measure. Start with the stable root list and artifact provenance; keep unsupported or malformed parent references as explicit errors. Do not count repeated member errors as separate faulty roots or manufacture completed classes.

## Remaining #1342 gate

This checkpoint records the completed local serial audit and its current outputs. It does not claim the issue's full Definition of Done: final focused behavior/rollback parity checks and the workspace CI checks still need to be recorded, and exact compiler-versus-library provenance for each unresolved-supertype root still needs to be included in the ranking before #1342 can be closed.
