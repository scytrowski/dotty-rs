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

The full-corpus audit and focused rollback evidence are recorded above. The
#1128 and qualified historical cohorts now have retained current row-level
comparisons, but the final closeout is still unverified: this focused profile
was run once (without determinism comparison), and the #1046/#1051 cohorts
remain at their latest recorded full-audit checkpoint rather than a refreshed
post-sprint row-level comparison. Keep issue #1342 open until that remaining
evidence and its required checks are explicitly accepted.

## Final closeout evidence

The following focused regressions are present on the audited mainline and ran
as part of the successful workspace CI run for PR #1376
([run 39514452922](https://github.com/scytrowski/dotty-rs/actions/runs/39514452922)).
The table maps each rollback-matrix requirement to its direct regression; it
does not infer semantic success from work later rolled back.

| Required case | Regression evidence |
| --- | --- |
| Object identity resolves, requested member is absent | `audit_and_classpath_resolvers_match_for_real_scala_source_module` checks the real `scala.io.Source` module and both successful `stdin` and unresolved `noSuchMember` lookups. |
| Incomplete ModuleClass has malformed or unsupported parent | `audit_module_completion_failure_is_atomic` checks a malformed completion failure leaves the shell Missing and the store checkpoint unchanged; `real_scala39_compiler_objects_have_canonical_module_identities` checks pinned compiler Object identities and their exact completion outcomes. |
| Module/member completes, then a later source expression fails | `source_expression_failure_rolls_back_module_before_successful_member_retry` checks the real `Source.stdin` path, rollback, and successful retry. |
| Pre-existing Missing shell is completed, then caller rolls back | `audit_resolver_forwards_module_completion_and_preserves_caller_rollback` checks restoration of the pre-existing shell and successful repeat completion. |
| New Object/ModuleClass pair is created, then rolled back | `audit_resolver_rolls_back_a_newly_created_module_identity_pair` checks store checkpoint restoration and removal of both new identities. |
| Source-owned package/class/Object survives classpath lookup | `compiler_tasty_lookup_does_not_shadow_a_source_owned_module_pair`, `package_term_does_not_replace_an_unbacked_source_owned_object`, and `source_owned_object_keeps_priority_over_same_named_compiler_tasty_object` cover backed-pair conflicts, unbacked Object preservation, and source typing precedence. |
| Ambiguous/duplicate or failed member operation follows partial classpath work | `missing_ambiguous_and_failed_members_are_atomic_and_retryable` checks no partial state survives missing, ambiguous, or failed member requests and that retry works. |
| Same resolver session retries to one canonical pair and a real member | `audit_and_classpath_resolvers_match_for_module_rollbacks_and_retry` compares wrapper and production resolver behavior across rollback/retry; `audit_and_classpath_resolvers_match_for_real_scala_source_module` checks canonical module identity and actual member lookup. |

### Historical cohort comparisons and remaining closeout gate

| Cohort | Latest row-level evidence | Status |
| --- | --- | --- |
| #1128 import-prefix cohort | All 2,394 frozen rows joined to retained current probe outcomes in [`current reconciliation`](typer-classpath-import-prefix-profile-1342-current-reconciliation.tsv); summary and method notes in [`profile report`](typer-classpath-import-prefix-profile-1342-current.md). Historical #1132 and #1180 snapshots are preserved. | Current row-level comparison recorded; one profile pass, determinism not checked. |
| Post-#1180 qualified import cohort | All 1,063 rows joined in [`current reconciliation`](typer-classpath-import-prefix-profile-1342-1180-current-reconciliation.tsv), with summary in [`summary`](typer-classpath-import-prefix-profile-1342-1180-current-summary.txt). | Current row-level comparison recorded; no method is fully typed. |
| #1128 source-qualified candidate cohort | All 1,615 rows joined in [`qualified cohort comparison`](typer-classpath-import-prefix-profile-1342-qualified-cohort-current.tsv), with target totals in [`summary`](typer-classpath-import-prefix-profile-1342-qualified-cohort-current-summary.txt). | Current row-level comparison recorded; no method is fully typed. |
| #1046 and #1051 module cohorts | Their 369-row and 576-row current outcomes remain in the append-only [full audit report](typer-classpath-corpus-audit-3.9.0.md) and [post-module 576-row reprofile](typer-classpath-corpus-audit-post-compiler-modules-3.9.0-object-module-reprofile.tsv). | Latest available comparison is the post-module audit checkpoint; no newer row-level comparison is included in the retained #1342 profile outputs. |
| Earlier semantic feature cohorts | Frozen baselines and their last recorded outcomes remain in the [full audit report](typer-classpath-corpus-audit-3.9.0.md) and [deferred-classpath hardening log](typer-deferred-classpath-hardening-3.9.0.md). | Historical snapshots preserved; current row-level comparison remains unverified. |

The new #1128 sidecar contains every frozen identity and distinguishes an
observed current blocker from missing evidence; it does not infer typing
success from a moved import blocker. For example,
`ClassPathFactory.scala|261|327|9` now records the `FileUtils` Object and
`TypeNameNotFound`, rather than the stale #1132 `ImportQualifierNotFound`.
The source-qualified cohort comparison similarly keeps package resolution,
member selection, and full-method typing separate.

Issue #1342 remains open. Its remaining closeout gate is a determinism-checked
current profile and explicit post-sprint comparisons for the #1046/#1051 and
earlier semantic cohorts, using retained outputs where available. Do not close
the issue based only on the older #1180 checkpoint. No full-corpus audit was
rerun for this documentation update.

The final blocker ranking and exactly one next-sprint recommendation are the
post-module results above: investigate TASTy unresolved-supertype roots,
ranking 50 distinct roots and 279 files separately from 8,949 repeated
observations. Full-audit limits and determinism evidence are in “Inputs and
run method”; the full corpus is not rerun for this documentation closeout.

### CI record

CI run #39514452922 passed both `Rust checks` (format, frontend boundaries,
Clippy, rustdoc, workspace tests, and diff check) and `Classfile JVM encoder
gate`. The PR also records 38 serial focused `audit_` tests, a producer test,
65 audit-tool tests, and a successful producer-to-consumer reconciliation of
all 1,063 #1180 rows. The retained #1342 probe additionally reconciles all 2,394 #1128 rows and the 1,615-row qualified cohort; its one-run status is explicit above. No hosted full-corpus workflow was dispatched. The full
corpus evidence above comes from the two serial local runs; peak RSS was not
captured.
