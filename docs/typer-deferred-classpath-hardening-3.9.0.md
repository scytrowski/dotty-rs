# Deferred classpath hardening: Scala 3.9.0 (#1023)

## Lifecycle contract

The source revision remains `777528f19a58e794c9954a42f433373472ec57f8`.
The comparison uses the same Scala 3.9.0 Maven artifacts and Temurin
OpenJDK 21.0.8 as the immutable [#976 baseline](typer-classpath-corpus-audit-976-3.9.0.md).
The [current report](typer-classpath-corpus-audit-3.9.0.md) is regenerated with
`tools/typer-classpath-corpus-audit/run --verify-determinism`.

| State | Shared symbol | Session | Loader-local repository |
| --- | --- | --- | --- |
| Unknown | No external identity | No identified/resolved entry | No entry |
| Identified external shell | Canonical `SymbolId`, `Missing`, binary origin/owner | `identified`; not `resolved` | Identity establishment does not insert `Loading` |
| Completing | Same ID; incomplete until ClassInfo is published | Identified; completed dependencies may enter `resolved` | `ClassEntry::Loading` only during this loader's full load |
| Completed | Same ID, `Complete(ClassInfo)`, owned declaration scope | Identified and resolved | `Loaded` for that loader; another loader can use the session cache |
| Failed full load | A pre-existing shell returns to its previous state; new identities disappear on rollback | No failed positive-cache entry | Typed `Failed` context retained locally without dangling allocated IDs |
| Source-owned Missing | Source identity, source origin | No owned external identity | External adapter declines completion |

`Missing` does not itself prove that the external resolver owns the symbol.
`Deferred(CompletionId)` remains a shared representation without a general
completer engine. A loader-local `Loading` entry is never handed to the next
loader as a successfully resolved class. Failure observations and load profiles
survive rollback; the semantic store and session changes do not.

## Permanent dependency policy

These regressions live in `crates/dotty-classloader/src/loader.rs`, except
where an integration file is explicitly named. Tests assert canonical symbols,
exact info states, type relationships, counts, and rollback allocations.

| Dependency role | Policy | Permanent regression |
| --- | --- | --- |
| Superclass | Eager | `superclass_and_interface_still_complete_eagerly_and_are_profiled` |
| Interface / TASTy trait parent | Eager | Same classfile test; `loading_source_tasty_resolves_iterator_through_package_alias` |
| Field object type | Shell | `field_type_dependency_completes_only_its_identity` |
| Method parameter / result / object array | Shell | `method_parameters_results_and_arrays_share_deferred_identities` |
| Class generic bound | Shell | `generic_arguments_bounds_and_wildcards_keep_structure_using_shells` |
| Method generic bound | Shell | Same signature regression |
| Generic argument / wildcard bound | Shell | Same signature regression; exact Applied/Bounds structure is preserved |
| Annotation identity | Shell where sufficient; existing unresolved fallback | `resolves_a_real_annotation_on_a_class`, `enters_a_resolved_field_and_method_annotation_onto_their_own_symbols`, and deferred member-reference profiles |
| TASTy member type | Shell / focused Error fallback | `best_effort_tasty_member_type_keeps_a_named_class_as_shell`, `unresolved_tasty_member_type_names_the_reason_when_no_name_was_reduced_at_all` |
| Nested semantic owner | Identity for static/local owners; complete non-static enclosing owner for generic scope | `ensure_nested_class_identity_uses_metadata_and_keeps_outer_as_shell`, enclosing-generics classfile regressions |

Classfile descriptors/signatures and TASTy trees are different contracts;
classfile annotation and InnerClasses policies are not fabricated for TASTy
files that do not carry those JVM attributes. TASTy parent and member tests
exercise the corresponding semantic contracts instead.

Identity-shell regressions additionally cover top-level/repeated identity,
package entry uniqueness, session handoff, in-place upgrade, nested visibility,
Object builtin reuse, malformed identity rollback, and preservation of old
shells on failed completion. The top-level identity snapshot explicitly checks
`identified`, `resolved`, repository, metadata, dependency absence, and the
single package-scope entry before and after full completion. Identity-only
creation does not lower member signatures or load their dependencies.

## On-demand and rollback matrix

`tests/classpath_source_typing.rs` records exact class-resource read order for
A -> B, Holder -> Child -> Parent, and the never-used Unused -> NeverRead edge.
B completes only at member use. Unused stays Missing and NeverRead is not read.
Repeated lookup on completed B adds no read or completion. Child's hierarchy
parent is eagerly completed while completing Child; a second lazy parent hook
would contradict the explicitly retained eager-parent policy.

| Provenance / outcome | Permanent regression and assertions |
| --- | --- |
| Shell predates source expression; member is used, later term fails | `direct_import_member_completion_participates_in_expression_rollback`: complete symbol restored, members removed, store/typed AST/source typed index/source type index restored, successful retry |
| Shell predates expression; on-demand member path fails | `source_member_selection_completes_shells_on_demand_and_rolls_back_in_place`: original shell restored and retried with exact completion order |
| Shell introduced while loading a class in a failed source expression | `expression_failure_rolls_back_loaded_class_and_allows_a_later_resolution`: symbols and package link removed, both source indexes and AST restored, same resolver retried |
| Resolver completion retained then outer caller rolls back | `resolver_rollback_restores_a_preexisting_shell_and_allows_retry`: old shell preserved, completion metadata/cache removed |
| Shell and member introduced inside one outer resolver transaction | `direct_member_outer_rollback_removes_a_same_transaction_shell_and_member`: identity/package/session entries removed |
| Missing / ambiguous / failed member request | `missing_ambiguous_and_failed_members_are_atomic_and_retryable`: `Ok(None)` and `Err` restore store/session; retry works |
| Existing owned package type shell | `package_type_lookup_completes_an_owned_shell_atomically`: same ID, failure atomicity, outer rollback, retry, no repeat I/O |

The package-type case fixes a measured regression at
`scala.Enumeration.ValueSet.fromSpecific`: its already identified
`scala.collection.mutable.Builder` was previously returned as unresolved.
Only a canonical identity owned by the session can be completed. Existing
source-owned Missing classes remain unresolved, with no classpath twin.

Resolver prefix tests retain field, method, type namespace, Applied, ThisType,
Explicit owner, missing, ambiguous, and failed-completion paths. Real JDK and
Scala fixtures prove declared members, including the JDK 21 opt-in tests.
InnerClasses metadata does not invent a type declaration that the shared
semantic model does not support. Audit regressions distinguish an absent member
after successful temporary completion from an unowned incomplete identity;
operation evidence is read before its semantic rollback is misinterpreted.

## Metric definitions

All counters describe work performed, including work later rolled back.
`classes` retains the old-compatible combined set of binary identities touched,
including failed requests. `completed_classes` contains only classes whose full
load succeeded in this operation; completed cache hits are separate. Completion
depth counts eager load edges, never shell-only member references.

- `identity_shell_creations` / `identity_shell_reuses`: incomplete identities,
  not completed classes or compiler builtin aliases.
- `member_type_dependencies_deferred`: successful descriptor, signature, and
  annotation identity references resolved without recursively completing them.
- `hierarchy_eager_loads`: superclass/interface requests, including cache hits
  and errors. Non-static enclosing generic owners also remain eager and appear
  in the completion depth/fanout.
- `on_demand_completions`: successful root full loads that began with a
  persistent identified Missing shell. Package-type and direct member requests
  participate alongside the explicit completion hook.
- `completed_cache_hits`: full-load calls satisfied by completed session entries.
- Per-root output includes completion count, shell creation/reuse, and sorted
  completed names. Top roots are sorted by completed fanout, then stable
  root/file/method/request keys.
- Per-method lifecycle observations record encountered shells and successful
  completions, independently of later rollback. Nested local method rows point
  to the enclosing attempt responsible for their first blocker. Direct field
  probes preserve their own request trace separately from full-method counts.

The automatic report gate checks p95 <= 70, maximum completed fanout <= 524,
retention of every frozen successful target key and the class-success count,
an unchanged typed-method denominator, and retention of every frozen
semantic cohort row. Unknown or ambiguous attribution is emitted explicitly.

## Pinned before/after results

Two full normalized runs on Temurin 21.0.8 were byte-identical. The unchanged
parser/namer setup processed 1,236 files, with no read/parser/namer failures.
The source revision, artifact versions, denominator, and definition of a typed
local method are unchanged.

| Metric | Frozen #976 | #1023 |
| --- | ---: | ---: |
| Full-load requests | 958 | 965 |
| Median / p95 / max completion depth | 0 / 140 / 141 | 0 / 3 / 4 |
| Max distinct fully completed classes in a request | 1048 | 8 |
| Max combined binary identities touched | 1048 | 48 |
| Max new identity shells in a request | n/a | 31 |
| Successful full completions (work, including later rollback) | n/a | 877 |
| Identity-shell creations / reuses | n/a | 2076 / 5511 |
| Deferred member-type references | n/a | 8347 |
| Eager hierarchy requests | n/a | 807 |
| Completed cache hits | n/a | 271 |
| On-demand root completions of persistent shells | n/a | 3 |
| External class successes / distinct classes | 358 / 29 | 358 / 29 |
| External non-class successes / distinct members | 0 / 0 | 0 / 0 |
| External unresolved / errors | 7493 / 217 | 7493 / 217 |
| Typed local methods | 0 / 3297 | 0 / 3297 |

P95 completion depth drops by **97.9%**; maximum completed fanout drops by
**99.2%**. Both exceed the required 50% reduction. The old-compatible combined
identity metric drops by 95.4%; it is retained separately from the completed
set. The old profile did not expose a separate completed-class set: 1048 is
the unchanged eager-era touched-class fanout used by the issue as its baseline,
not a retrospectively regenerated completion count. Comparing combined
identities (1048 -> 48) provides the strict old-compatible measurement as well.
The longest eager chain is now four edges, and the largest completed root
is still `dotty/tools/io/AbstractFile`: eight successful classes, 31 new shells,
48 combined identities, with cache hits separate. Top-N per-request rows are
stable and sorted in the current raw report. More root requests do not mean
more fanout: temporarily completed identities can be retried after rollback.
The three on-demand root completions are `scala/Int` twice and
`scala/collection/mutable/Builder` once; the sorted root counters are retained
in the raw profile.

All 29 frozen successful canonical target keys remain successful. The
`Builder` package-shell regression found during hardening was corrected before
this measurement, restoring the exact 358 successes and per-method request
outcomes. No source semantics or package object materialization was added.

Corpus field/method/type/other class-prefix declaration successes remain zero.
All 3,771 incomplete class-prefix requests in the frozen cohort are source
Missing identities, not resolver-owned external shells; preceding package and
source-completion blockers prevent this corpus from reaching the new member
path. A class materialized by a package type request is a class-symbol success,
not an external non-class member success. Focused real JDK/Scala fixtures prove
field and method lookup through external shells independently of corpus reach.

## Error and first-blocker accounting

All 217 previously recorded member-error observations retain the same
method/request target accounting: **zero disappeared**, **zero recorded errors
postponed to a later member request**, and **zero new recorded errors**. The
remaining categories are 212 unresolved TASTy supertype failures (26 requested
roots, 51 files) and five unresolved-class failures (two roots, two files).

The largest true requested-class supertype failure is
`scala/annotation/retainsCap` (110). `dotty/tools/tasty/TastyVersion` still fails
through the eagerly required `scala/Serializable` parent (four observations);
`CodePointStringStepper` still requires `scala/collection/Stepper/EfficientSplit`
(one). `IndexedSeq` and `LinearSeq` still fail through `Seq`, and mutable `Map`
through mutable `Iterable` (one each). Full per-parent shapes and normalized
requested/root chains are retained in the raw report. These are hierarchy or
requested-class failures, not cascades through an unused member value type.

The eager member-dependency *work* cascade is eliminated even when the observed
error count does not change: AbstractFile no longer completes hundreds of
unused member dependencies. Focused `field_type_dependency_completes_only_its_identity`
and the source never-used edge show that a known member-type class can remain
Missing without reading its missing parent. Completing that class later still
reports the parent error; valid lazy identity is not guessed complete.

The five resolution-family attribution tables are byte-identical to #976.
They include distinct targets, materialized-prefix counts, package/object,
classloader, and unknown/multiple-candidate causes. No newly materialized
generic receiver/member blocker is observed. The complete cause rows are in
the raw report; there are 3,100 tracked local-method observations:

| First blocker | #976 observations / files | #1023 observations / files |
| --- | --- | --- |
| ImportQualifierNotFound | 2908 / 277 | 2908 / 277 |
| SymbolResolution | 27 / 7 | 27 / 7 |
| TypeNameNotFound | 83 / 31 | 83 / 31 |
| TermNameNotFound | 20 / 12 | 20 / 12 |
| MemberLookup | 62 / 28 | 62 / 28 |

For ImportQualifierNotFound, 2,446 observations have an unresolved first
segment/package (30 distinct targets), 361 a term/object materialization gap
(36 targets), and 101 lack a matching failed request. TermNameNotFound adds
eight term/object-gap observations on two targets. Materialized-prefix and
unsupported-generic attribution remain explicit; no ambiguous request is
assigned a guessed cause. Source import naming/default-import reachability
remains separate from the package/object binary adapter boundary.

## Frozen semantic cohorts and real method reach

The [120-row TSV](typer-classpath-corpus-audit-3.9.0-deferred-cohorts.tsv) retains
both original and current row text, exact source tree identities where present,
current resolution targets/outcomes, observed shells/completions, resolved
requests, first blocker, and full method status. Shell/completion columns are
operation observations, including work subsequently rolled back. They do not
claim that the enclosing method retained those semantic states.

| Cohort | Retained rows | Current result |
| --- | ---: | --- |
| #946 impure functions | 13 | SymbolResolution; zero fully typed |
| #826–#829 ordinary function types | 12 | Eight ImportQualifierNotFound, one MemberLookup, three TypeNameNotFound; zero fully typed |
| #856 immutable inferred fields | Seven direct fields + 14 enclosing method rows | Every field probe and enclosing row remains ImportQualifierNotFound |
| #858 mutable inferred fields | Seven direct fields + 10 enclosing method rows | Three field probes complete; three ImportQualifierNotFound and one TypeNameNotFound; no fully typed enclosing method |
| #880 singleton cohort | 22 | ImportQualifierNotFound; zero fully typed |
| #899 local signatures | 14 | ImportQualifierNotFound; zero fully typed |
| #929 local values | 21 | 16 ImportQualifierNotFound, four anonymous-given identity blockers, one inline-value blocker; no completed local declaration mapping |

98 rows have an exact enclosing resolver-attempt trace, 14 have a direct field
probe trace, and eight have no resolver work observed because they stop at
another semantic gate. Absence of a trace is reported as `unobserved`, rather
than inferring that a dependency exists or completed. No row changes its
first blocker or full method outcome from #976.

Representative deepest local-method observations are `SymbolLoaders.scala`
`addPrefix` (tree 510), `checkPathMatches` (579), and `simpleDesugar` (607).
Their enclosing `needCompile` attempt (tree 739) successfully crosses
AbstractFile classpath materialization and completes eight classes before
`UnstableSelectionPrefix`. These local rows inherit that first blocker and
remain untyped. They demonstrate classpath reach without claiming their own
bodies completed. The full-method denominator still measures typed local
DefDef mappings, not successful dependency loads or isolated field probes.

## Next sprint decision

| Candidate | Current measured reach | Risk / reusable work |
| --- | --- | --- |
| A. Package term / Scala object materialization | 2749 unsupported requests / 80 keys; type resource 2212 / 51, companion `$` resource 2196 / 46 | High module/companion identity risk; reuse shared Packages, canonical shell/completion contracts, transactions, resource probes |
| B. Anonymous classes | 32 observations / 15 files, unchanged | High anonymous ownership/ClassInfo/lowering risk; reuses ordinary `new`, but the same import/classpath blockers remain |
| C. External generic member adaptation | Zero successfully materialized corpus non-class members; zero attributed generic receiver/member blockers | Substitution is substantial semantic work with unmeasured current corpus reach |
| D. Lazy hierarchy / full completer engine | p95 3, max depth 4, max completed fanout 8; 807 eager hierarchy requests | Current tail already meets the gate; deferral would complicate parent/type-variable scope and cycle contracts |
| E. Another compact Typer semantic slice | Residual local value blockers: four anonymous-given observations, one inline-value observation | Smaller reach; contextual/compile-time semantics cannot be inferred from modifier flags |

**The one next sprint recommendation is A: bounded package term / Scala object
materialization.** Companion resource evidence covers 2,196 current requests on
46 keys, and the existing binary adapter makes a narrow canonical module design
possible. Start with identity/owner/companion and rollback contracts, then
measure the same frozen cohorts. A resource is not proof of module semantics;
this report does not promise that all those requests or methods will type.
Source import naming and other first-segment gaps must remain observable.

The bounded external-class completion hook is sufficient for the measured
sprint. A general CompletionId engine is not justified now: depth/fanout have
fallen, non-static generic-owner eagerness is bounded, and no new unresolved
circular-completion orchestration requirement was observed. Package modules
may expose a reusable lazy-completion requirement in the next sprint; measure
that need before generalizing to arbitrary symbols or deferring hierarchy.

## #1051 package-term and module identity audit

The [current classpath audit](typer-classpath-corpus-audit-3.9.0.md) was run
twice against Scala revision `777528f19a58e794c9954a42f433373472ec57f8` and
Temurin 21.0.8; the normalized reports matched byte-for-byte. The frozen #1046
baseline still contains 2,749 observations / 80 package-term keys and 369
first-blocker rows (361 imports, eight term names). The current resolver made
2,297 package-term requests and returned 1,791 canonical Objects across 46
verified object identities; one request returned the package
`scala.collection.mutable`, 505 were unresolved, and none errored. It completed
zero external ModuleClasses and resolved zero external module members in the
corpus (`distinct_module_targets`
and `distinct_member_targets` are both zero).

The 369 frozen rows retain exact first-blocker attribution: 358 validate an
Object/ModuleClass identity and advance past the baseline blocker, with zero
fully typed methods; the current blockers are 350 `ObjectModuleClassUnavailable`,
eight `ObjectTermReferenceDeferred`, and 11 `ImportQualifierNotFound`. No
corpus request reached an external module-member lookup after those earlier
blockers. This explains the zero member successes without treating a type
symbol, resource probe, or blocker movement as a successful member resolution.
The required positive end-to-end gate is covered separately by
`source_typer_selects_a_method_through_a_validated_external_object` and
`external_object_import_forms_resolve_the_same_module_member`, which resolve
`scala.io.Source.stdin` through the checked-in Scala 3.9.0 artifact.

All 29 successful external class target keys from the previous gate remain;
the automatic gate reports 29/75 current target keys. Deferred loading remains
within the hard limits: p95/max completion depth 3/4 and maximum completed
fanout 8. The typed-local-method denominator remains 3,297 with zero typed.
The exact 120-row cohort sidecar and frozen module request profiles remain
available in the audit report.

The next recommendation from this checkpoint is one bounded sprint for
verified object ModuleClass availability and completion. `ObjectModuleClassUnavailable`
leads the current semantic blocker ranking at 576 observations in 77 files;
the resolver has 46 authoritative object identities to reuse. Explicit
first-segment failures have greater raw volume (2,292 in 197 files) but span
26 unresolved targets and cross package/import boundaries. Scala TASTy
supertype errors account for 212 observations across 26 requested roots and
51 files; anonymous class instantiation accounts for 32 / 15 files. Generic
member adaptation has no corpus requests that reach it. The recommended module
slice must keep all these other gaps observable and retain the pinned member
fixture gate.

## Validation

- Two full pinned Temurin 21.0.8 audit runs: byte-identical normalized output,
  automatic dependency/materialization gate passed; 120 frozen cohort rows.
- `cargo test --workspace --all-targets --locked`: 5,774 passed, eight opt-in
  tests ignored by the default run.
- JDK 21 classloader opt-in tests: two passed (Object/getClass and
  ClassSpecializer SpeciesData enclosing generic identity).
- Python audit-tool regressions: 11 passed; shell runner syntax checked.
- `cargo fmt --all -- --check`, workspace clippy with `-D warnings`, workspace
  rustdoc with `-D warnings`, and `git diff --check`: passed.

Build parallelism and full-workspace test concurrency were limited to two.
The frozen #976 report is byte-for-byte identical to the pre-sprint checked
report; only the current report and its generated cohort sidecar are refreshed.

## #1073 post-parity comparison and sprint recommendation

This is an append-only checkpoint; the #976, #1023, and #1051 results and
recommendations above remain historical. The corrected pinned audit already
compares two normalized Scala 3.9.0 / Temurin 21.0.8 runs byte-for-byte. The
#1073 changes add regression assertions and this comparison, not resolver
behavior, so the audit output remains the same.

| Measure | Corrected result | Gate / interpretation |
| --- | ---: | --- |
| Package-term requests / canonical Object results | 14,157 / 13,083 | One non-object result, 1,073 unresolved, zero errors; object identity remains separate from module completion |
| External ModuleClass completion / module-member success | 17 / 58 across 9 distinct members | Positive full-corpus reach after parity; the pinned `scala.io.Source.stdin` path separately completes and resolves through the audit wrapper |
| #1051 module reprofile cohort | 576 rows / 77 files | Every frozen row has a stable source/method key and current lookup, completion, member, rollback, and blocker outcome |
| #1046 module/import cohort | 369 rows; 358 validated identities; 197 past baseline blocker; 0 fully typed | Moved first blockers are not counted as fully typed methods |
| Historical successful class target keys | 29 retained | Current audit has 13,452 class-symbol successes / 88 distinct identities |
| Dependency completion depth | median / p95 / max = 0 / 2 / 4 | p95 <= 6 |
| Maximum completed-class fanout | 8 | <= 16 |
| Typed local methods | 0 / 3,297 | Original denominator and definition retained |

The post-parity import split is 2,394 `ImportQualifierNotFound` observations
whose cause is first-segment/package unresolved (209 files, 30 exact targets),
plus 150 term/object materialization-gap observations (17 files, four targets).
These prefixes are not successfully materialized. The competing candidates
are 31 `ObjectModuleClassUnavailable` observations in four files with zero
distinct targets carrying authoritative Object provenance, 212 TASTy
unresolved-supertype failures across 26 roots / 51 files, and 32 anonymous
class instantiation observations across 15 files. Module parity is working at
the corpus boundary (17 completions and 58 member successes); the remaining
unavailable rows do not identify a grounded target for a wider module sprint.

**Recommend exactly one next sprint: a bounded source import first-segment /
package resolution slice.** It has the largest corrected attributable reach:
2,394 observations, 209 files, 30 exact targets, with four additional target
forms in the term/object materialization-gap bucket. Reuse the package and
resolver attribution paths, keep ownership boundaries explicit, and do not
add default imports. TASTy parent work and anonymous classes remain measured
alternatives with separate owners and higher scope/identity risk. Generic
member adaptation has no isolated corpus blocker that justifies implementing
it speculatively.

## #1132 Import-prefix audit and source typing rollback

The pinned full-corpus audit and the frozen #1128 reprofile each ran twice and
matched byte-for-byte on Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0, and Temurin 21.0.8.
The full audit covered 1,236 files. Its 3,297 local-method denominator is
unchanged and remains at 0 typed methods. Full-corpus `ImportQualifierNotFound`
stands at 2,501. This is a broader set than the selected frozen #1128 cohort.

| Measure | Full pinned audit | Frozen #1128 cohort, reprofiled |
| --- | ---: | ---: |
| Import blocker observations | 2,501 | 2,394 frozen; 2,342 unresolved at next segment |
| Files / target keys | Full corpus | 209 / 30 frozen; 196 / 29 unresolved |
| Distinct blocker origins | — | 904 frozen; 879 unresolved |
| External class-symbol successes | 18,169 | Not a method success metric |
| External non-class member successes | 59 | Not a method success metric |
| Distinct classes / p95 eager depth | 135 / 3 | — |
| Max fully completed classes per request | 9 (hard gate <= 16) | — |

Within the selected cohort, the 58 descendant-only `dotty` and `java` target
observations no longer stop at the package blocker. The frozen cohort has 58
advanced rows, while the unresolved-only view also contains six new import
observations outside that cohort, for a net reduction of 52. The remaining
unresolved observations include 1,618 source-package registry hits across five
qualified targets with direct class/TASTy resources. The audit reports blocker
movement only: none of the 3,297 local methods typed.

The row-level [#1128 reconciliation](typer-classpath-import-prefix-profile-1132-3.9.0-reconciliation.tsv)
joins all 2,394 frozen import identities: 2,336 still fail at the same import
tree and 58 have a matching successful package request with its selected
package identity. Those 58 enclosing methods now stop at `SymbolResolution`
(20), `StringLiteralTypingDeferred` (14), `TermNameNotFound` (9),
`TypeNameNotFound` (8), a different `ImportQualifierNotFound` import (6), or
`ExternalGenericInstantiationDeferred` (1). The separate current
[unresolved-only profile](typer-classpath-import-prefix-profile-1132-3.9.0.md)
contains 2,342 rows, including six newly observed imports outside the frozen
cohort.

The end-to-end regressions also preserve atomic rollback for a missing
intermediate package and a missing final imported member. Both leave semantic
store and typed-AST checkpoints unchanged and do not leave a `me` package
entry; the final-member case confirms the temporary loaded class identity is
removed.

**Recommend exactly one next sprint: source-owned lexical package lookup for
qualified import candidates.** Preserve current-package precedence, `_root_`
lookup, ambiguity diagnostics, and rollback. Candidates without source or
classpath evidence should continue to remain unresolved. The frozen #1128
profile remains unchanged; the full current corpus report and row sidecars are
in [the generated audit](typer-classpath-corpus-audit-3.9.0.md).


### #1180 import-profile performance checkpoint

The pinned import-prefix profile ran twice with matching normalized output.
Its 209 frozen import-bearing source files took 288.468 s and 288.037 s;
selected source parse/name/type work consumed 285.981 s and 285.707 s, while
reconciliation took about 0.3 s per run. This is not the deferred class-loading
profile. The separate full-corpus run was stopped at 75 / 1,236 files after
more than 153,000 resolver calls, so this checkpoint does not refresh the
current completed-load depth or fanout limits. The prior p95 and fanout figures
remain historical and must not be cited as measurements of the #1180 branch.
See the [#1180 import report](typer-classpath-import-prefix-profile-1180-3.9.0.md).
The follow-up targeted profile measured the first `tpd.scala` method at
6,214 ms before and 3,546 ms after adding `LoadingSession` reverse indexes;
member-lookup time fell from 4,939 ms to 2,797 ms for the same 512 requests.
The full-corpus rerun after this optimization was stopped at 250 / 1,236 files
after about 20 minutes and roughly 295,000 module-identity requests. The later
#1180 follow-up completed two pinned full-corpus runs with byte-identical
normalized reports and deferred-cohort sidecars; p95 completion depth was 0
and maximum completed fanout was 9, within the issue limits of 6 and 16. Fresh
current blocker counts and direct origins are in the [#1180 blocker sidecar](typer-classpath-corpus-audit-1180-current-blockers.tsv).
