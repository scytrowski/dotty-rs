# Sequential Typer corpus profile

This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.

- dotty-rs commit: `621364dbcebda0e1dd32c0efde476b27ea8a0183`
- Scala checkout: `/tmp/scala3-3.9.0` at `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: `3.9.0` under `/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2`
- JDK: `21.0.8` at `/home/vdrake6/.jdks/jdk-21`
- Host: `AMD Ryzen 9 7940HS w/ Radeon 780M Graphics`, 30582380 kB memory; build profile `test`; Cargo/test concurrency fixed at 1
- Manifest files: 4; profiled method observations per run: 6; repetitions: 1
- Timing mode: `timing_only`; root method limit per file: `2`. Exact path/tree/method identities and outcomes are in [`methods.tsv`](methods.tsv) and [`method-outcomes.tsv`](method-outcomes.tsv); match-probe selection is in [`match-probes.tsv`](match-probes.tsv). Wrapper, rollback, and classpath-resource data are in [`audit-wrapper.tsv`](audit-wrapper.tsv), [`rollback.tsv`](rollback.tsv), and [`classpath-resources.tsv`](classpath-resources.tsv).
- Cargo target cache by repetition: 1=warm. `cold` means no compiled local_definition_audit test binary existed in the selected target directory.
- Input page-cache measurements: 1=warm-read-through (warmup 16 ms; resident bytes before/after setup unknown/122241024). Warm mode reads the selected Scala sources, pinned Scala JARs, and JDK jmods before each sample, then records residency with `fincore`. Cold mode evicts only these selected inputs with POSIX_FADV_DONTNEED; it does not clear the system-wide cache.
- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup, sampler overhead, and test-side log emission. Captured bytes plus the Python log read/write times are recorded in `run-timings.tsv`; the cost of emitting each diagnostic line remains inside the process timing.
- Process measurements: child CPU user/system mean 13.83/2.34 s; maximum sampled aggregate Cargo-process-tree RSS 97.2 MiB. RSS sums per-process resident sets and can double-count shared mappings; 100 ms sampling may miss short peaks. CPU comes from child process accounting.
- Classpath setup includes JDK module and Scala JAR adapter construction. No artifact decode timing is reported separately: class/module completion timers are inclusive at the resolver boundary.
- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. They do not expose all lookup cache misses; those are not inferred.
- `audit-wrapper.tsv` separates the measured AuditResolver wrapper interval from delegated resolver time; `exclusive_us` is the difference and includes observation bookkeeping/formatting. Timing-only runs bypass AuditResolver and report no wrapper rows. Rollback counts and affected observation-range lengths are in `rollback.tsv`.
- `classpath-resources.tsv` counts logical resource responses and returned bytes, not physical JAR reads/decompressions: the classloader interface does not expose those physical events. `inner_wall_us` measures the aggregate ClassPathEntry lookup interval only in detailed mode; cached negative requests are counted separately.
- Semantic/report ratio compares `typer_setup + typing` against generation of all profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.

## Repetition timings

See [`run-timings.tsv`](run-timings.tsv).

Median / p95 selected-file wall time: 4415 / 7436 ms. Across repetitions, source typer setup plus method typing took 8760 ms; profile report generation took about 1 ms (semantic-to-profile-report ratio 8760.00:1).

## Slowest files

| Mean ms | File |
| ---: | --- |
| 7436.0 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` |
| 6397.0 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` |
| 4415.0 | `compiler/src/dotty/tools/dotc/core/Types.scala` |
| 0.0 | `library/src/scala/annotation/compileTimeOnly.scala` |

## Slowest methods

Method timings are averaged by stable file/tree identity across repetitions.

| Mean ms | Max ms | Runs | File | Tree | Method |
| ---: | ---: | ---: | --- | ---: | --- |
| 4359.0 | 4359 | 1 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1790 | `dotty.tools.dotc.ast.tpd$.DefDef` |
| 2252.0 | 2252 | 1 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 1031 | `dotty.tools.dotc.core.Types$.Type.derivesFrom` |
| 1975.0 | 1975 | 1 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 204 | `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` |
| 94.0 | 94 | 1 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 1723 | `dotty.tools.backend.jvm.GenericSignatures$.javaSig0` |
| 76.0 | 76 | 1 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1971 | `dotty.tools.dotc.ast.tpd$.ClassDef` |
| 0.0 | 0 | 1 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 349 | `dotty.tools.dotc.core.Types$.Type.testProvisional` |

## Phase time

| Phase | Total ms | Share of measured per-file phases |
| --- | ---: | ---: |
| `postprocess_ms` | 9176 | 50.3% |
| `typing_ms` | 8758 | 48.0% |
| `parse_ms` | 277 | 1.5% |
| `inventory_ms` | 23 | 0.1% |
| `namer_ms` | 2 | 0.0% |
| `typer_setup_ms` | 2 | 0.0% |

## Resolver operation families

Resolver counters and timings were not captured in timing-only mode.

| Operation | Calls | Delegated ms |
| --- | ---: | ---: |
| `class_completion` | not captured | not captured |
| `classfile_completion` | not captured | not captured |
| `member` | not captured | not captured |
| `module_completion` | not captured | not captured |
| `module_identity` | not captured | not captured |
| `package` | not captured | not captured |
| `tasty_completion` | not captured | not captured |

## Classpath load counts

| Observation | Count across runs |
| --- | ---: |

## Resolver request counts

| Family | Count across runs |
| --- | ---: |

## AuditResolver wrapper time

Times include wrapper bookkeeping and exclude the delegated interval in the `exclusive_us` column.

| Operation | Calls | Wall ms | Delegated ms | Exclusive ms |
| --- | ---: | ---: | ---: | ---: |

## Largest classpath request roots

Existing resolver observations sorted these by completed-class fanout; paths and method/tree identities are unchanged.

| Root | File and method | Classes touched | Completions | Cache hits |
| --- | --- | ---: | ---: | ---: |

## Three measured method roots

Ranked by mean inclusive method-attempt time. These attempts stop at the first typer blocker, so they characterize current audit work rather than successful full-method typing. Resolver attribution was not captured in timing-only mode.

| Rank | Mean inclusive ms | Member calls | Delegated member ms | Exact root |
| ---: | ---: | ---: | ---: | --- |
| 1 | 4359.0 | not captured | not captured | `compiler/src/dotty/tools/dotc/ast/tpd.scala:262` `dotty.tools.dotc.ast.tpd$.DefDef` (tree 1790) |
| 2 | 2252.0 | not captured | not captured | `compiler/src/dotty/tools/dotc/core/Types.scala:284` `dotty.tools.dotc.core.Types$.Type.derivesFrom` (tree 1031) |
| 3 | 1975.0 | not captured | not captured | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala:42` `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` (tree 204) |

Resolver per-method call counts and durations are not available in timing-only mode.
Classloader and AuditResolver counters are omitted in timing-only mode by design.
