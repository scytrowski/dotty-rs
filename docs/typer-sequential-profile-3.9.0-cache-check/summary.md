# Sequential Typer corpus profile

This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.

- dotty-rs commit: `8bf923e1028ae662bfce475b8d86b10a45c6837f`
- Scala checkout: `/home/vdrake6/dotty-rs/.worktrees/scala39-source` at `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: `3.9.0` under `/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2`
- JDK: `21.0.8` at `/home/vdrake6/.jdks/jdk-21`
- Host: `AMD Ryzen 9 7940HS w/ Radeon 780M Graphics`, 30582380 kB memory; build profile `test`; Cargo/test concurrency fixed at 1
- Manifest files: 5; profiled method observations per run: 1161; repetitions: 2
- Cargo target cache by repetition: 1=warm, 2=warm. `cold` means no compiled local_definition_audit test binary existed in the selected target directory.
- Input page-cache measurements: 1=partial-eviction (41734144 bytes resident before), 2=warm-observed (42258432 bytes resident before). Cold mode evicts only the selected Scala sources, pinned Scala JARs, and JDK jmods with POSIX_FADV_DONTNEED, then checks their resident bytes using `fincore`; it does not clear the system-wide cache.
- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup and harness/report-log overhead.
- Classpath setup includes JDK module and Scala JAR adapter construction. No artifact decode timing is reported separately: class/module completion timers are inclusive at the resolver boundary.
- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. They do not expose all lookup cache misses; those are not inferred.
- Rollback/retry observations are recorded by existing resolver rows. Time spent in failed probes is included in method and resolver inclusive timing and is not independently subtractable.
- Semantic/report ratio compares `typer_setup + typing` against generation of all profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.

## Repetition timings

See [`run-timings.tsv`](run-timings.tsv).

Median / p95 selected-file wall time: 27 / 251802 ms. Across repetitions, source typer setup plus method typing took 366310 ms; profile report generation took about 24 ms (semantic-to-profile-report ratio 15262.92:1).

## Slowest files

| Mean ms | File |
| ---: | --- |
| 235834.0 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` |
| 475.5 | `compiler/src/dotty/tools/dotc/core/Types.scala` |
| 27.5 | `compiler/src/dotty/tools/dotc/core/Contexts.scala` |
| 2.0 | `library/src/scala/Predef.scala` |
| 0.0 | `library/src/scala/annotation/compileTimeOnly.scala` |

## Slowest methods

Method timings are averaged by stable file/tree identity across repetitions.

| Mean ms | Max ms | Runs | File | Tree | Method |
| ---: | ---: | ---: | --- | ---: | --- |
| 4423.5 | 5305 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 63 | `dotty.tools.dotc.ast.tpd$.ta` |
| 1144.0 | 1495 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1469 | `dotty.tools.dotc.ast.tpd$.DefDef` |
| 1140.5 | 1487 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1239 | `dotty.tools.dotc.ast.tpd$.Bind` |
| 1133.5 | 1474 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 3420 | `dotty.tools.dotc.ast.tpd$.localCtx` |
| 1130.5 | 1469 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1174 | `dotty.tools.dotc.ast.tpd$.LambdaTypeTree` |
| 1125.0 | 1457 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 2429 | `dotty.tools.dotc.ast.tpd$.Export` |
| 1120.5 | 1448 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 2738 | `dotty.tools.dotc.ast.tpd$.ref` |
| 1112.5 | 1431 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 2466 | `dotty.tools.dotc.ast.tpd$.Annotated` |
| 1111.0 | 1430 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1790 | `dotty.tools.dotc.ast.tpd$.DefDef` |
| 1108.0 | 1419 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 2448 | `dotty.tools.dotc.ast.tpd$.PackageDef` |

## Phase time

| Phase | Total ms | Share of measured per-file phases |
| --- | ---: | ---: |
| `typing_ms` | 366310 | 77.5% |
| `postprocess_ms` | 105977 | 22.4% |
| `parse_ms` | 349 | 0.1% |
| `inventory_ms` | 27 | 0.0% |
| `namer_ms` | 2 | 0.0% |
| `typer_setup_ms` | 0 | 0.0% |

## Resolver operation families

Durations are wrapper-inclusive; nested work may overlap.

| Operation | Calls | Inclusive ms |
| --- | ---: | ---: |
| `member` | 212018 | 29117.347 |
| `module_identity` | 211970 | 164.558 |
| `package` | 112178 | 121.474 |
| `class_completion` | 54 | 0.002 |
| `classfile_completion` | 0 | 0.000 |
| `module_completion` | 0 | 0.000 |
| `tasty_completion` | 0 | 0.000 |

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
| `class_completion_calls` | 54 |
| `class_completion_inner_us` | 22 |
| `classfile_completion_calls` | 0 |
| `classfile_completion_inner_us` | 0 |
| `external_member_errors` | 414 |
| `external_member_requests` | 212018 |
| `external_member_successes` | 106002 |
| `external_member_unresolved` | 105600 |
| `external_package_errors` | 0 |
| `external_package_requests` | 112178 |
| `external_package_successes` | 450 |
| `external_package_unresolved` | 111728 |
| `resolver_member_requests` | 212018 |
| `tasty_completion_calls` | 0 |
| `tasty_completion_inner_us` | 0 |
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

1. **SourceTyper method typing**: 2322 observed items and 366310 ms. Inspect the recorded hottest method identities first; do not share store/session semantic IDs across files.
2. **post-typing audit accounting**: 10 observed items and 105977 ms. Break down repeated source/blocker projections while preserving per-row identities and rollback coverage.
3. **resolver member lookup**: 212018 observed items and 29117 ms. Investigate immutable resource/query reuse only; keep SymbolId, TypeId, ScopeId, interned names, and transaction state local to each SemanticStore/LoadingSession.

For this frozen baseline, per-method resolver durations were emitted in whole milliseconds and then aggregated; zero means below per-method millisecond resolution, not a measured exact zero. The opt-in log now also emits microseconds for future reproductions.
The current implementation cannot attribute classfile parsing, TASTy decoding, shell creation, and semantic completion as disjoint CPU intervals. Do not add their inclusive timers together or infer a safe cross-store semantic cache from reuse counts.
