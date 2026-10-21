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

The resolver observed 1,389,149 package-term requests, 469,642 canonical Object results across 127 external Object identities, 20 completed external ModuleClasses, and 144 external module-member successes across 25 distinct member identities. It recorded 8,949 `InvalidTastyFile::UnresolvedSupertype` member errors, across 279 files and 50 distinct requested class roots. The repeated error count is not a distinct-root count. Exact pinned-artifact lookup finds all 50 requested roots in the Scala 3.9.0 compiler or library JAR: 18 compiler roots account for 8,590 weighted observations, and 32 library roots account for 359. These are root-level weighted observations, not distinct broken artifacts. The root-to-resource mapping, artifact versions, and verified SHA-256 values are in [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-tasty-supertype-roots.tsv`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-tasty-supertype-roots.tsv).

## Frozen import reconciliation

The post-#1180 sidecar joins all 1,063 observations, 31 target keys, 81 files, and 393 original blocker origins. It records each row's original source/method/enclosing-attempt/import identity, original failed qualifier and outcome, target classification, current qualifier identity and kind, current blocker/outcome, and whether the source method is fully typed. Target-specific classpath resource evidence remains in the companion #1180 classpath-evidence TSV, referenced by stable target key.

Of these rows, 1,034 currently identify an Object qualifier across 23 canonical identities; 25 remain at the same unresolved import blocker, and 1,038 have moved to a later blocker. None of the 1,063 methods is fully typed. Fourteen of the 23 current Object identities match the post-audit #1051 ModuleClass profile. Its completion and member outcomes are preserved as attempts; `rolled_back` work is not treated as committed semantic success.

The row-level reconciliation is [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-current-reconciliation.tsv`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-current-reconciliation.tsv), with its counts in [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-summary.txt`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-summary.txt). The complete 576-row Object ModuleClass reprofile is [`typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv).

## Next sprint recommendation

Recommend exactly one next slice: **TASTy unresolved-supertype root resolution and compatibility classification**. It is now the broadest measured cross-corpus cause by distinct requested roots (50) and affected files (279), while retaining the repeated 8,949 observations as a separate weighted measure. Start with the stable root list and artifact provenance; prioritize the compiler JAR roots by weighted volume while investigating library roots separately. Keep unsupported or malformed parent references as explicit errors. Do not count repeated member errors as separate faulty roots or manufacture completed classes.

## #1342 closeout status

The Definition of Done evidence is complete. The pinned full-corpus audit ran
in two sequential passes with `--verify-determinism`; both normalized reports
and their frozen sidecars matched byte-for-byte. The 1,455-second elapsed time,
1,236-file input, 3,297-method denominator and current deferred-load gates are
recorded above. Peak RSS was not captured and is reported as unavailable.

### Frozen cohort outcomes

| Cohort | Current evidence |
| --- | --- |
| #1128 import-prefix | All 2,394 stable rows in [`the retained current reconciliation`](typer-classpath-import-prefix-profile-1342-current-reconciliation.tsv), with the exact one-pass focused-probe provenance in [`its report`](typer-classpath-import-prefix-profile-1342-current.md). |
| Post-#1180 import-prefix | All 1,063 rows / 31 frozen keys in [`the deterministic full-audit reconciliation`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1180-current-reconciliation.tsv); 25 observations across six targets remain at the original import blocker and 1,038 advanced. |
| #1046 package-term | All 369 rows joined to the full-audit current outcomes in [`the current reconciliation`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-1046-current-reconciliation.tsv). |
| #1051 Object / ModuleClass | All 576 rows in [`the post-module reprofile`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv), generated identically by both full-audit passes. |
| Earlier semantic feature cohorts | All 120 frozen rows in [`the deterministic current-outcome sidecar`](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-deferred-cohorts.tsv). |
| #1128 source-qualified subset | All 1,615 rows in [`the current qualified-cohort comparison`](typer-classpath-import-prefix-profile-1342-qualified-cohort-current.tsv), preserving package, selector and full-method outcomes separately. |

The original snapshots remain unchanged. A moved first blocker, selected Object,
package identity or rolled-back completion is not counted as a fully typed
method or committed ModuleClass success. The full audit records 1,038 #1180
observations advancing, 144 external module-member successes across 25 member
identities, 8,949 TASTy unresolved-supertype observations across 50 distinct
roots and 279 files, and the single next-sprint recommendation: TASTy
unresolved-supertype root resolution.

The eight-case transaction regression matrix and focused/workspace CI evidence
are listed above. Dependencies #1336–#1340 are all closed. No full audit was
rerun for this closeout documentation pass.

### CI record

CI run #39514452922 passed both `Rust checks` (format, frontend boundaries,
Clippy, rustdoc, workspace tests, and diff check) and `Classfile JVM encoder
gate`. The PR also records 38 serial focused `audit_` tests, a producer test,
65 audit-tool tests, and a successful producer-to-consumer reconciliation of
all 1,063 #1180 rows. The retained #1342 probe additionally reconciles all 2,394 #1128 rows and the 1,615-row qualified cohort; its one-run status is explicit above. No hosted full-corpus workflow was dispatched. The full
corpus evidence above comes from the two serial local runs; peak RSS was not
captured.
