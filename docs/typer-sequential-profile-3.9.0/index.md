# Sequential Typer corpus profile

This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.

- dotty-rs commit: `eafa385e508aa0eaa0710342b656d03d2df6acc0`
- Scala checkout: `/home/vdrake6/dotty-rs/.worktrees/scala39-source` at `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: `3.9.0` under `/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2`
- JDK: `21.0.8` at `/home/vdrake6/.jdks/jdk-21`
- Host: `AMD Ryzen 9 7940HS w/ Radeon 780M Graphics`, 30582380 kB memory; build profile `test`; Cargo/test concurrency fixed at 1
- Manifest files: 5; profiled method observations per run: 1161; repetitions: 2
- Both timed repetitions used the already-built Cargo target. A cold Cargo build was not measured separately; OS page-cache state was not controlled and cold/warm filesystem time is not compared.
- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup and harness/report-log overhead.
- Classpath setup includes JDK module and Scala JAR adapter construction. No artifact decode timing is reported separately: class/module completion timers are inclusive at the resolver boundary.
- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. They do not expose all lookup cache misses; those are not inferred.
- Rollback/retry observations are recorded by existing resolver rows. Time spent in failed probes is included in method and resolver inclusive timing and is not independently subtractable.
- Semantic/report ratio compares `typer_setup + typing` against generation of these profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.

## Repetition timings

See [`run-timings.tsv`](run-timings.tsv).

Median / p95 selected-file wall time: 26 / 222679 ms. Across repetitions, source typer setup plus method typing took 338855 ms; profile report generation took about 11 ms (semantic-to-profile-report ratio 30805.00:1).

## Slowest files

| Mean ms | File |
| ---: | --- |
| 222314.5 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` |
| 483.0 | `compiler/src/dotty/tools/dotc/core/Types.scala` |
| 26.0 | `compiler/src/dotty/tools/dotc/core/Contexts.scala` |
| 1.0 | `library/src/scala/Predef.scala` |
| 0.0 | `library/src/scala/annotation/compileTimeOnly.scala` |

## Slowest methods

Method timings are averaged by stable file/tree identity across repetitions.

| Mean ms | Max ms | Runs | File | Tree | Method |
| ---: | ---: | ---: | --- | ---: | --- |
| 3535.5 | 3542 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 63 | `dotty.tools.dotc.ast.tpd$.ta` |
| 1056.5 | 1322 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4483 | `dotty.tools.dotc.ast.tpd$.TypedTreeCopier.TypeTree` |
| 1048.5 | 1300 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4451 | `dotty.tools.dotc.ast.tpd$.TypedTreeCopier.Try` |
| 1032.0 | 1268 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4422 | `dotty.tools.dotc.ast.tpd$.TypedTreeCopier.CaseDef` |
| 1022.5 | 1254 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4544 | `dotty.tools.dotc.ast.tpd$.TimeTravellingTreeCopier.Apply` |
| 903.5 | 1017 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1154 | `dotty.tools.dotc.ast.tpd$.ByNameTypeTree` |
| 877.5 | 961 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4568 | `dotty.tools.dotc.ast.tpd$.TimeTravellingTreeCopier.TypeApply` |
| 865.5 | 935 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 5236 | `dotty.tools.dotc.ast.tpd$.TreeOps.appliedTo` |
| 847.5 | 897 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4395 | `dotty.tools.dotc.ast.tpd$.TypedTreeCopier.Closure` |
| 844.0 | 894 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 4637 | `dotty.tools.dotc.ast.tpd$.skipTransform` |

## Phase time

| Phase | Total ms | Share of measured per-file phases |
| --- | ---: | ---: |
| `typing_ms` | 338855 | 76.0% |
| `postprocess_ms` | 106409 | 23.9% |
| `parse_ms` | 347 | 0.1% |
| `inventory_ms` | 24 | 0.0% |
| `namer_ms` | 2 | 0.0% |
| `typer_setup_ms` | 0 | 0.0% |

## Resolver operation families

Durations are wrapper-inclusive; nested work may overlap.

| Operation | Calls | Inclusive ms |
| --- | ---: | ---: |
| `member` | 212018 | 25616.000 |
| `package` | 112178 | 4.000 |
| `class_completion` | not captured | below 1 ms per method |
| `module_completion` | 0 | 0.000 |
| `module_identity` | 211970 | below 1 ms per method |
| `classfile_completion` | not captured | not captured |
| `tasty_completion` | not captured | not captured |

## Classpath load counts

| Observation | Count across runs |
| --- | ---: |
| `completed_cache_hits` | 0 |
| `full_completions` | 32 |
| `hierarchy_loads` | 16 |
| `identity_shell_creations` | 66 |
| `identity_shell_reuses` | 138 |
| `member_type_references` | 236 |
| `on_demand_completions` | 0 |
| `top_level_class_loads` | 32 |

## Resolver request counts

| Family | Count across runs |
| --- | ---: |
| `external_member_errors` | 414 |
| `external_member_requests` | 212018 |
| `external_member_successes` | 106002 |
| `external_member_unresolved` | 105600 |
| `external_package_errors` | 0 |
| `external_package_requests` | 112178 |
| `external_package_successes` | 450 |
| `external_package_unresolved` | 111728 |
| `resolver_member_requests` | 212018 |
| `typer_module_completion_resolver_calls` | 0 |
| `typer_module_identity_resolver_calls` | 211970 |

## Largest classpath request roots

Existing resolver observations sorted these by completed-class fanout; paths and method/tree identities are unchanged.

| Root | File and method | Classes touched | Completions | Cache hits |
| --- | --- | ---: | ---: | ---: |
| `scala/Function2` | `compiler/src/dotty/tools/dotc/ast/tpd.scala::compiler/src/dotty/tools/dotc/ast/tpd.scala#tree=4775::dotty.tools.dotc.ast.tpd$.TreeOps.find` (run 1) | 8 | 2 | 0 |
| `scala/Function2` | `compiler/src/dotty/tools/dotc/ast/tpd.scala::compiler/src/dotty/tools/dotc/ast/tpd.scala#tree=4775::dotty.tools.dotc.ast.tpd$.TreeOps.find` (run 2) | 8 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Contexts.scala::compiler/src/dotty/tools/dotc/core/Contexts.scala#tree=661::dotty.tools.dotc.core.Contexts$.Context.printer` (run 1) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Contexts.scala::compiler/src/dotty/tools/dotc/core/Contexts.scala#tree=661::dotty.tools.dotc.core.Contexts$.Context.printer` (run 2) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=11823::dotty.tools.dotc.core.Types$.RecType$.apply` (run 1) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=11823::dotty.tools.dotc.core.Types$.RecType$.apply` (run 2) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=2096::dotty.tools.dotc.core.Types$.Type.containsWildcardTypes` (run 1) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=2096::dotty.tools.dotc.core.Types$.Type.containsWildcardTypes` (run 2) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=2140::dotty.tools.dotc.core.Types$.Type.forallParts` (run 1) | 7 | 2 | 0 |
| `scala/Function1` | `compiler/src/dotty/tools/dotc/core/Types.scala::compiler/src/dotty/tools/dotc/core/Types.scala#tree=2140::dotty.tools.dotc.core.Types$.Type.forallParts` (run 2) | 7 | 2 | 0 |

## Bounded follow-up candidates

Costs are observed in this five-file cohort; repeated counts and durations do not imply a safe cache or a projected speedup.

1. **SourceTyper method typing**: 2322 observed items and 338855 ms. Inspect the recorded hottest method identities first; do not share store/session semantic IDs across files.
2. **post-typing audit accounting**: 10 observed items and 106409 ms. Break down repeated source/blocker projections while preserving per-row identities and rollback coverage.
3. **resolver member lookup**: 212018 observed items and 25616 ms. Investigate immutable resource/query reuse only; keep SymbolId, TypeId, ScopeId, interned names, and transaction state local to each SemanticStore/LoadingSession.

For this frozen baseline, per-method resolver durations were emitted in whole milliseconds and then aggregated; zero means below per-method millisecond resolution, not a measured exact zero. The opt-in log now also emits microseconds for future reproductions.
The current implementation cannot attribute classfile parsing, TASTy decoding, shell creation, and semantic completion as disjoint CPU intervals. Do not add their inclusive timers together or infer a safe cross-store semantic cache from reuse counts.

### High-resolution resolver check

After adding microsecond timers, a third single serial pass used the same five-file manifest at dotty-rs commit `b32800dd1`. It took 220.155 s Cargo wall time and 219.535 s inside audit setup and source processing. This supplemental pass is separate from the two-run timing baseline above. It measured 106,009 member requests at 12.726 s, 105,985 Object→ModuleClass identity requests at 70.634 ms, and 56,089 package requests at 28.969 ms. The loader profile contained 16 full class completions, 33 shell creations, 69 shell reuses, and zero completed-cache hits. The direct `complete_class` hook saw 27 calls / 2 μs, but none came from classfile or TASTy symbols. Therefore the 16 loads initiated within member resolution still lack a reliable classfile-versus-TASTy time split; the zero direct-hook counters must not be read as zero classpath loads. See [`resolver-microsecond-check.tsv`](resolver-microsecond-check.tsv).
