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

## Frozen #976 audit gate and dependency profile

The immutable pre-sprint baseline (also retained in `docs/typer-classpath-corpus-audit-976-3.9.0.md`) used the pinned Scala 3.9/JDK `21.0.8` audit: one run was completed; determinism was not checked. The frozen #972 request shapes, #973 generic-signature cases, #974 TASTy parent cases, #975 materialization gate, and #946/#826–#829/#856–#858/#880/#899/#929 cohorts remain in the audit. The local-method result is `0` / 3,297. `ImportQualifierNotFound` is `2908 (277 files)` and remains in the resolution/classpath family; the separate Typer-owned semantic leader remains `AnonymousClassInstantiationDeferred` (32 observations / 15 files).

The `resolution_blocker_attribution` table in the raw output joins each tracked first-blocker method attempt to resolver requests made during its enclosing typing attempt. It reports cause, method count, file count, distinct target count and examples, and successfully materialized prefix classes. A cause is assigned only when there is exactly one distinct failed resolver target/outcome in that method attempt; no such request or multiple candidates is reported as `unknown` with candidate causes and targets. It includes `ImportQualifierNotFound`, `SymbolResolution`, `TypeNameNotFound`, `MemberLookup`, and `TermNameNotFound`. Package resolution probes distinguish an unresolved first segment/package from a loader error; member outcomes distinguish an unresolved package member, term/object gap, incomplete class identity, loader error, and other.

The external class-symbol gate is `358` successes across `29` classes; external non-class member successes are `0`, unresolved requests are `7493`, and loader errors are `217`. Package term requests have type-side resources for `2212` requests / `51` targets; companion `$` resources occur on `2196` requests / `46` targets. A resource is evidence only and does not establish canonical module identity.

The opt-in profile observed `958` top-level loads, median nested depth `0.0`, p95 `140`, maximum depth `141`, and maximum fanout `1048` distinct classes. The largest roots are headed by `dotty/tools/io/AbstractFile`. This distribution is evidence for an eager-loading tail, while the majority of requests have no nested dependency loads.

| Candidate | Measured reach | Reuse and risk |
| --- | --- | --- |
| A. Anonymous classes | 32 observations / 15 files | Reuses ordinary `new` typing; high identity, ownership, and class-info risk. |
| B. Lazy/deferred dependency completion | `958` top-level loads; p95 depth `140`; maximum fanout `1048` classes | Reuses the new profile as before/after evidence; high loader transaction and completeness risk. |
| C. Package term/object materialization | `2212` requests / `51` target keys; companion resource on `2196` requests / `46` targets | Reuses package probes; high canonical module/companion identity risk. |
| D. External generic member adaptation | No corpus external non-class member has materialized yet. | Focused generic receiver support exists; broad substitution semantics remain unmeasured in corpus reach. |

**Historical #976 next sprint recommendation: investigate lazy/deferred dependency completion.** The p95 and maximum depth/fanout meet the measured condition for this candidate; retain the profile and require a bounded before/after reduction. Do not expand loading eagerness as part of #976. This is one recommendation; the other candidates remain separately measured alternatives.
