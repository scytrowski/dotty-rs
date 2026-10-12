## #975 package type and member materialization

At the #975 checkpoint, package `TypeRef` plus a unique type name materialized a class into its canonical package/session identity. The pinned Scala 3.9/JDK 21 corpus recorded 358 external class symbols across 29 distinct classes (125 first materializations and 233 same-file session reuses); non-class external members remained at zero. The remaining member requests and errors are refreshed in the raw audit below. Focused fixtures cover a real JDK class and a real Scala 3.9 class, with complete class info, declaration scope, declared member access, inherited lookup, repeated identity, and rollback/retry.

Frozen cohorts are tracked row by row; a moved blocker is not full method typing:

| Cohort | Current resolution or typing result | Enclosing method |
| --- | --- | --- |
| #946 impure functions (13 rows) | All stop at `SymbolResolution` at `scala/annotation/retainsCap`'s unresolved TASTy parent. | 0 typed |
| Earlier ordinary function types (12 rows) | 8 stop at `ImportQualifierNotFound`, 1 at `MemberLookup`, and 3 at `TypeNameNotFound`. | 0 typed |
| Inferred class fields | All 14 immutable completions stop at `ImportQualifierNotFound`; 3 of 7 mutable fields complete directly, 3 stop at `ImportQualifierNotFound`, and 1 at `TypeNameNotFound`. | No external member success |
| Singleton references (22 rows) | All moved past singleton projection and stop at `ImportQualifierNotFound`. | 0 typed |
| Local signatures (14 rows) | All remain at `ImportQualifierNotFound`; the local-signature-specific deferral is gone. | 0 typed |
| Local values (21 rows) | 16 stop at `ImportQualifierNotFound`, 4 at anonymous-given identity deferral, and 1 at inline-value deferral. | No completed local-symbol mapping |

Package term/object and incomplete source-class completion remain explicit boundaries. Generic external receiver argument adaptation remains deferred even when the class identity loads. Resolver rollback removes failed loads and permits retry.

## #976 audit gate and dependency profile

The final pinned Scala 3.9/JDK `@JDK_VERSION@` audit: @AUDIT_RUN_SUMMARY@. The frozen #972 request shapes, #973 generic-signature cases, #974 TASTy parent cases, #975 materialization gate, and #946/#826–#829/#856–#858/#880/#899/#929 cohorts remain in the audit. The local-method result is `@TYPED_METHODS@` / 3,297. `ImportQualifierNotFound` is `@IMPORT_COUNT_FILES@` and remains in the resolution/classpath family; the separate Typer-owned semantic leader remains `AnonymousClassInstantiationDeferred` (32 observations / 15 files).

The `resolution_blocker_attribution` table in the raw output joins each tracked first-blocker method attempt to resolver requests made during its enclosing typing attempt. It reports cause, method count, file count, distinct target count and examples, and successfully materialized prefix classes. A cause is assigned only when there is exactly one distinct failed resolver target/outcome in that method attempt; no such request or multiple candidates is reported as `unknown` with candidate causes and targets. It includes `ImportQualifierNotFound`, `SymbolResolution`, `TypeNameNotFound`, `MemberLookup`, and `TermNameNotFound`. Package resolution probes distinguish an unresolved first segment/package from a loader error; member outcomes distinguish an unresolved package member, term/object gap, incomplete class identity, loader error, and other.

The external class-symbol gate is `@CLASS_SUCCESSES@` successes across `@DISTINCT_CLASSES@` classes; external non-class member successes are `@NONCLASS_SUCCESSES@`, unresolved requests are `@MEMBER_UNRESOLVED@`, and loader errors are `@MEMBER_ERRORS@`. Package term requests have type-side resources for `@PACKAGE_TERM_REQUESTS@` requests / `@PACKAGE_TERM_TARGETS@` targets; companion `$` resources occur on `@COMPANION_REQUESTS@` requests / `@COMPANION_TARGETS@` targets. A resource is evidence only and does not establish canonical module identity.

The opt-in profile observed `@TOP_LOADS@` top-level loads, median nested depth `@MEDIAN_DEPTH@`, p95 `@P95_DEPTH@`, maximum depth `@MAX_DEPTH@`, and maximum fanout `@MAX_FANOUT@` distinct classes. The largest roots are headed by `@LARGEST_ROOT@`. This distribution is evidence for an eager-loading tail, while the majority of requests have no nested dependency loads.

| Candidate | Measured reach | Reuse and risk |
| --- | --- | --- |
| A. Anonymous classes | 32 observations / 15 files | Reuses ordinary `new` typing; high identity, ownership, and class-info risk. |
| B. Lazy/deferred dependency completion | `@TOP_LOADS@` top-level loads; p95 depth `@P95_DEPTH@`; maximum fanout `@MAX_FANOUT@` classes | Reuses the new profile as before/after evidence; high loader transaction and completeness risk. |
| C. Package term/object materialization | `@PACKAGE_TERM_REQUESTS@` requests / `@PACKAGE_TERM_TARGETS@` target keys; companion resource on `@COMPANION_REQUESTS@` requests / `@COMPANION_TARGETS@` targets | Reuses package probes; high canonical module/companion identity risk. |
| D. External generic member adaptation | No corpus external non-class member has materialized yet. | Focused generic receiver support exists; broad substitution semantics remain unmeasured in corpus reach. |

**Next sprint recommendation: investigate lazy/deferred dependency completion.** The p95 and maximum depth/fanout meet the measured condition for this candidate; retain the profile and require a bounded before/after reduction. Do not expand loading eagerness as part of #976. This is one recommendation; the other candidates remain separately measured alternatives.
