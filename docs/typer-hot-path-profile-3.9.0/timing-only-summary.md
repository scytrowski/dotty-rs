# Sequential Typer corpus profile

This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.

- dotty-rs commit: `7e38430ba87ee6bc45990b561bcc354b5707e12e`
- Scala checkout: `/home/vdrake6/dotty-rs/.worktrees/scala39-source` at `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: `3.9.0` under `/home/vdrake6/dotty-rs/target-fs2/coursier/https/repo1.maven.org/maven2`
- JDK: `21.0.8` at `/home/vdrake6/.jdks/jdk-21`
- Host: `AMD Ryzen 9 7940HS w/ Radeon 780M Graphics`, 30582380 kB memory; build profile `test`; Cargo/test concurrency fixed at 1
- Manifest files: 4; profiled method observations per run: 6; repetitions: 1
- Timing mode: `timing_only`; root method limit per file: `2`. Exact path/tree/method identities and outcomes are in [`methods.tsv`](methods.tsv) and [`method-outcomes.tsv`](method-outcomes.tsv); match-probe selection is in [`match-probes.tsv`](match-probes.tsv). Wrapper, rollback, and classpath-resource data are in [`audit-wrapper.tsv`](audit-wrapper.tsv), [`rollback.tsv`](rollback.tsv), and [`classpath-resources.tsv`](classpath-resources.tsv).
- Cargo target cache by repetition: 1=warm. `cold` means no compiled local_definition_audit test binary existed in the selected target directory.
- Input page-cache measurements: 1=warm-read-through (warmup 16 ms; resident bytes before/after setup unknown/122241024). Warm mode reads the selected Scala sources, pinned Scala JARs, and JDK jmods before each sample, then records residency with `fincore`. Cold mode evicts only these selected inputs with POSIX_FADV_DONTNEED; it does not clear the system-wide cache.
- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup, sampler overhead, and test-side log emission. Captured bytes plus the Python log read/write times are recorded in `run-timings.tsv`; the cost of emitting each diagnostic line remains inside the process timing.
- Process measurements: child CPU user/system mean 10.53/1.38 s; maximum sampled aggregate Cargo-process-tree RSS 97.0 MiB. RSS sums per-process resident sets and can double-count shared mappings; 100 ms sampling may miss short peaks. CPU comes from child process accounting.
- Classpath setup includes JDK module and Scala JAR adapter construction. No artifact decode timing is reported separately: class/module completion timers are inclusive at the resolver boundary.
- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. They do not expose all lookup cache misses; those are not inferred.
- `audit-wrapper.tsv` separates the measured AuditResolver wrapper interval from delegated resolver time; `exclusive_us` is the difference and includes observation bookkeeping/formatting. Timing-only runs bypass AuditResolver and report no wrapper rows. Rollback counts and affected observation-range lengths are in `rollback.tsv`.
- `classpath-resources.tsv` counts logical resource responses and returned bytes, not physical JAR reads/decompressions: the classloader interface does not expose those physical events. `inner_wall_us` measures the aggregate ClassPathEntry lookup interval only in detailed mode; cached negative requests are counted separately.
- Semantic/report ratio compares `typer_setup + typing` against generation of all profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.

## Repetition timings

See [`run-timings.tsv`](run-timings.tsv).

Median / p95 selected-file wall time: 3805 / 4163 ms. Across repetitions, source typer setup plus method typing took 5887 ms; profile report generation took about 2 ms (semantic-to-profile-report ratio 2943.50:1).

## Slowest files

| Mean ms | File |
| ---: | --- |
| 4163.0 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` |
| 3841.0 | `compiler/src/dotty/tools/dotc/core/Types.scala` |
| 3805.0 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` |
| 0.0 | `library/src/scala/annotation/compileTimeOnly.scala` |

## Slowest methods

Method timings are averaged by stable file/tree identity across repetitions.

| Mean ms | Max ms | Runs | File | Tree | Method |
| ---: | ---: | ---: | --- | ---: | --- |
| 2046.0 | 2046 | 1 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1790 | `dotty.tools.dotc.ast.tpd$.DefDef` |
| 1856.0 | 1856 | 1 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 204 | `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` |
| 1823.0 | 1823 | 1 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 1031 | `dotty.tools.dotc.core.Types$.Type.derivesFrom` |
| 91.0 | 91 | 1 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 1723 | `dotty.tools.backend.jvm.GenericSignatures$.javaSig0` |
| 68.0 | 68 | 1 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1971 | `dotty.tools.dotc.ast.tpd$.ClassDef` |
| 0.0 | 0 | 1 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 349 | `dotty.tools.dotc.core.Types$.Type.testProvisional` |

## Phase time

| Phase | Total ms | Share of measured per-file phases |
| --- | ---: | ---: |
| `typing_ms` | 5886 | 49.9% |
| `postprocess_ms` | 5727 | 48.5% |
| `parse_ms` | 179 | 1.5% |
| `inventory_ms` | 11 | 0.1% |
| `namer_ms` | 1 | 0.0% |
| `typer_setup_ms` | 1 | 0.0% |

## Resolver operation families

Durations measure delegated resolver calls; nested class/TASTy loading remains inclusive inside those calls. AuditResolver wrapper time is reported separately.

| Operation | Calls | Delegated ms |
| --- | ---: | ---: |
| `class_completion` | 0 | 0.000 |
| `classfile_completion` | 0 | 0.000 |
| `member` | 0 | 0.000 |
| `module_completion` | 0 | 0.000 |
| `module_identity` | 0 | 0.000 |
| `package` | 0 | 0.000 |
| `tasty_completion` | 0 | 0.000 |

## Classpath load counts

| Observation | Count across runs |
| --- | ---: |
| `completed_cache_hits` | 0 |
| `full_completions` | 0 |
| `hierarchy_loads` | 0 |
| `identity_shell_creations` | 0 |
| `identity_shell_reuses` | 0 |
| `member_type_references` | 0 |
| `on_demand_completions` | 0 |
| `top_level_class_loads` | 0 |

## Resolver request counts

| Family | Count across runs |
| --- | ---: |
| `class_completion_calls` | 0 |
| `class_completion_inner_us` | 0 |
| `classfile_completion_calls` | 0 |
| `classfile_completion_inner_us` | 0 |
| `external_member_errors` | 0 |
| `external_member_requests` | 0 |
| `external_member_successes` | 0 |
| `external_member_unresolved` | 0 |
| `external_package_errors` | 0 |
| `external_package_requests` | 0 |
| `external_package_successes` | 0 |
| `external_package_unresolved` | 0 |
| `resolver_member_requests` | 0 |
| `tasty_completion_calls` | 0 |
| `tasty_completion_inner_us` | 0 |
| `typer_module_completion_resolver_calls` | 0 |
| `typer_module_identity_resolver_calls` | 0 |

## AuditResolver wrapper time

Times include wrapper bookkeeping and exclude the delegated interval in the `exclusive_us` column.

| Operation | Calls | Wall ms | Delegated ms | Exclusive ms |
| --- | ---: | ---: | ---: | ---: |

## Largest classpath request roots

Existing resolver observations sorted these by completed-class fanout; paths and method/tree identities are unchanged.

| Root | File and method | Classes touched | Completions | Cache hits |
| --- | --- | ---: | ---: | ---: |

## Three confirmed hot method roots

Ranked by mean inclusive method-attempt time. These attempts stop at the first typer blocker, so they characterize current audit work rather than successful full-method typing. Member resolver time is nested within method time and is shown only as attribution, not added again.

| Rank | Mean inclusive ms | Member calls | Delegated member ms | Exact root |
| ---: | ---: | ---: | ---: | --- |
| 1 | 2046.0 | 0.0 | 0.0 | `compiler/src/dotty/tools/dotc/ast/tpd.scala:262` `dotty.tools.dotc.ast.tpd$.DefDef` (tree 1790) |
| 2 | 1856.0 | 0.0 | 0.0 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala:42` `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` (tree 204) |
| 3 | 1823.0 | 0.0 | 0.0 | `compiler/src/dotty/tools/dotc/core/Types.scala:284` `dotty.tools.dotc.core.Types$.Type.derivesFrom` (tree 1031) |

All three hot roots spend nearly all observed method time in delegated `resolve_member`; this bounded profile therefore supports a focused follow-up that attributes resolver-internal work (prefix/package traversal, classpath lookup, and member selection) per request family. Recheck repeated request keys before proposing request-local reuse. Do not add cross-session semantic caches or claim an optimization speedup from this baseline.

Per-method resolver durations are emitted in microseconds. A zero call count means that operation did not run for that method; zero elapsed time with calls is below timer resolution, not proof of zero cost.
The current implementation cannot attribute classfile parsing, TASTy decoding, shell creation, and semantic completion as disjoint CPU intervals. Do not add their inclusive timers together or infer a safe cross-store semantic cache from reuse counts.

## Instrumentation variant comparison

`variant-comparison.tsv` records detailed versus timing-only wall time. Exact method identities, method outcomes, match-probe identities, and match outcomes were checked for equality against the comparison directory. This is an observer-effect comparison, not a code optimization speedup.
