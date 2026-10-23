# Sequential Typer corpus profile

This focused profile is serial and opt-in. Times below describe the selected manifest only; they do not estimate full-corpus duration.

- dotty-rs commit: `d7dc059ceecc38885db05a4ce3a3b93caff8b119`
- Scala checkout: `/tmp/scala3-3.9.0` at `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: `3.9.0` under `/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2`
- JDK: `21.0.8` at `/home/vdrake6/.jdks/jdk-21`
- Host: `AMD Ryzen 9 7940HS w/ Radeon 780M Graphics`, 30582380 kB memory; build profile `test`; Cargo/test concurrency fixed at 1
- Manifest files: 4; profiled method observations per run: 6; repetitions: 2
- Exact audit integration-test binary prebuilt outside measured samples in 33 ms; measured invocations that rebuild are rejected.
- Timing mode: `cheap_counters`; root method limit per file: `2`. Exact method identities/outcomes and match-probe selection are in [`methods.tsv`](methods.tsv), [`method-outcomes.tsv`](method-outcomes.tsv), and [`match-probes.tsv`](match-probes.tsv). A/B counters are in [`cheap-resolver.tsv`](cheap-resolver.tsv); full-mode wrapper, rollback, and classpath data are in [`audit-wrapper.tsv`](audit-wrapper.tsv), [`rollback.tsv`](rollback.tsv), and [`classpath-resources.tsv`](classpath-resources.tsv).
- Cargo target cache by repetition: 1=warm, 2=warm. `cold` means no compiled local_definition_audit test binary existed in the selected target directory.
- Input page-cache measurements: 1=warm-read-through (warmup 14 ms; resident bytes before/after setup unknown/122241024), 2=warm-read-through (warmup 14 ms; resident bytes before/after setup unknown/122241024). Warm mode reads the selected Scala sources, pinned Scala JARs, and JDK jmods before each sample, then records residency with `fincore`. Cold mode evicts only these selected inputs with POSIX_FADV_DONTNEED; it does not clear the system-wide cache.
- `runner_overhead_ms` is cargo wall time minus the test's measured audit setup and source loop. It includes Cargo/test startup, sampler overhead, and test-side log emission. Captured bytes plus the Python log read/write times are recorded in `run-timings.tsv`; the cost of emitting each diagnostic line remains inside the process timing.
- Process measurements: child CPU user/system mean 10.70/1.25 s; maximum sampled aggregate Cargo-process-tree RSS 99.3 MiB. RSS sums per-process resident sets and can double-count shared mappings; 100 ms sampling may miss short peaks. CPU comes from child process accounting.
- Classpath setup includes JDK module and Scala JAR adapter construction. No artifact decode timing is reported separately: class/module completion timers are inclusive at the resolver boundary.
- Cache evidence: classloader profiles report completed-cache hits, full completions, identity shell creation/reuse, and hierarchy requests. They do not expose all lookup cache misses; those are not inferred.
- `audit-wrapper.tsv` separates full AuditResolver wrapper time from delegated resolver time; `exclusive_us` includes detailed observation work. Semantic-only mode bypasses audit collection; cheap-counters mode records only fixed operation/outcome counters and timings in `cheap-resolver.tsv`. Rollback detail is available only in full mode.
- `classpath-resources.tsv` counts logical resource responses and returned bytes, not physical JAR reads/decompressions: the classloader interface does not expose those physical events. `inner_wall_us` measures the aggregate ClassPathEntry lookup interval only in detailed mode; cached negative requests are counted separately.
- Semantic/report ratio compares `typer_setup + typing` against generation of all profile TSV/Markdown artifacts. It does not time the full-corpus normalized report writer, which this focused run does not invoke.

## Repetition timings

See [`run-timings.tsv`](run-timings.tsv).

Median / p95 selected-file wall time: 3818 / 4220 ms. Across repetitions, source typer setup plus method typing took 11799 ms; profile report generation took about 2 ms (semantic-to-profile-report ratio 5899.50:1).

## Slowest files

| Mean ms | File |
| ---: | --- |
| 4198.0 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` |
| 3826.0 | `compiler/src/dotty/tools/dotc/core/Types.scala` |
| 3817.5 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` |
| 0.0 | `library/src/scala/annotation/compileTimeOnly.scala` |

## Slowest methods

Method timings are averaged by stable file/tree identity across repetitions.

| Mean ms | Max ms | Runs | File | Tree | Method |
| ---: | ---: | ---: | --- | ---: | --- |
| 2055.5 | 2067 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1790 | `dotty.tools.dotc.ast.tpd$.DefDef` |
| 1865.0 | 1867 | 2 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 204 | `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` |
| 1818.5 | 1820 | 2 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 1031 | `dotty.tools.dotc.core.Types$.Type.derivesFrom` |
| 90.5 | 91 | 2 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala` | 1723 | `dotty.tools.backend.jvm.GenericSignatures$.javaSig0` |
| 67.5 | 68 | 2 | `compiler/src/dotty/tools/dotc/ast/tpd.scala` | 1971 | `dotty.tools.dotc.ast.tpd$.ClassDef` |
| 0.0 | 0 | 2 | `compiler/src/dotty/tools/dotc/core/Types.scala` | 349 | `dotty.tools.dotc.core.Types$.Type.testProvisional` |

## Phase time

| Phase | Total ms | Share of measured per-file phases |
| --- | ---: | ---: |
| `typing_ms` | 11797 | 49.8% |
| `match_ms` | 11480 | 48.5% |
| `parse_ms` | 348 | 1.5% |
| `inventory_ms` | 23 | 0.1% |
| `postprocess_ms` | 16 | 0.1% |
| `namer_ms` | 2 | 0.0% |
| `typer_setup_ms` | 2 | 0.0% |

## Resolver operation families

Detailed resolver counters and timings are not captured outside full mode; cheap boundary counters, when selected, are listed separately.

| Operation | Calls | Delegated ms |
| --- | ---: | ---: |
| `class_completion` | not captured | not captured |
| `classfile_completion` | not captured | not captured |
| `member` | not captured | not captured |
| `module_completion` | not captured | not captured |
| `module_identity` | not captured | not captured |
| `package` | not captured | not captured |
| `tasty_completion` | not captured | not captured |

## Cheap counter adapter

Mode B records resolver operation counts, delegate/wrapper durations, and coarse result categories without request identities or transaction/event strings. Empty rows mean the selected mode did not collect cheap counters.

| Scope | Operation | Calls | Wall µs | Delegated µs | Exclusive µs | Ok | None | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `match` | `checkpoint` | 1112 | 63 | 20 | 43 | 1112 | 0 | 0 |
| `match` | `member_lookup` | 5434 | 11461612 | 11461387 | 225 | 172 | 5260 | 2 |
| `match` | `module_identity` | 10066 | 7206 | 6802 | 404 | 10066 | 0 | 0 |
| `match` | `package_lookup` | 1128 | 772 | 728 | 44 | 340 | 788 | 0 |
| `match` | `package_object_collision` | 34 | 28 | 26 | 2 | 0 | 34 | 0 |
| `match` | `rollback_to` | 370 | 126 | 113 | 13 | 370 | 0 | 0 |
| `source` | `checkpoint` | 1948 | 111 | 36 | 75 | 1948 | 0 | 0 |
| `source` | `member_lookup` | 7626 | 11775905 | 11775594 | 311 | 322 | 7302 | 2 |
| `source` | `module_identity` | 13328 | 9603 | 9068 | 535 | 13328 | 0 | 0 |
| `source` | `package_lookup` | 1560 | 1729 | 1668 | 61 | 248 | 1312 | 0 |
| `source` | `package_object_collision` | 64 | 120 | 117 | 3 | 0 | 64 | 0 |
| `source` | `rollback_to` | 772 | 227 | 198 | 29 | 772 | 0 | 0 |

## Match-probe pass attribution

Match selector and case typing run in a separate SourceTyper pass. These resolver and classpath observations are reported separately from root-method typing.

| Operation | Calls | Delegated µs |
| --- | ---: | ---: |
| `package` | not captured | not captured |
| `member` | not captured | not captured |
| `module_identity` | not captured | not captured |
| `module_completion` | not captured | not captured |
| `class_completion` | not captured | not captured |
| `classfile_completion` | not captured | not captured |
| `tasty_completion` | not captured | not captured |

- Rollback/checkpoint-range metrics: not captured.
- Match-pass classpath responses: not captured.
- Per-file phase and resolver rows: [`match-phases.tsv`](match-phases.tsv), [`match-resolver.tsv`](match-resolver.tsv), and [`match-resolver-wrappers.tsv`](match-resolver-wrappers.tsv).

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

## Full-mode AuditResolver subphases

These separately timed wrapper windows attribute request preparation/identity, result classification, event collection, and transaction bookkeeping. They are inclusive across resolver operations that can nest; do not sum them with delegated resolver intervals or wrapper totals.

| Scope | Stage | Calls | Elapsed ms |
| --- | --- | ---: | ---: |

## Largest classpath request roots

Existing resolver observations sorted these by completed-class fanout; paths and method/tree identities are unchanged.

| Root | File and method | Classes touched | Completions | Cache hits |
| --- | --- | ---: | ---: | ---: |

## Three measured method roots

Ranked by mean inclusive method-attempt time. These attempts stop at the first typer blocker, so they characterize current audit work rather than successful full-method typing. Detailed per-method attribution is only available in full mode.

| Rank | Mean inclusive ms | Member calls | Delegated member ms | Exact root |
| ---: | ---: | ---: | ---: | --- |
| 1 | 2055.5 | not captured | not captured | `compiler/src/dotty/tools/dotc/ast/tpd.scala:262` `dotty.tools.dotc.ast.tpd$.DefDef` (tree 1790) |
| 2 | 1865.0 | not captured | not captured | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala:42` `dotty.tools.backend.jvm.GenericSignatures$.mayNeedSignature` (tree 204) |
| 3 | 1818.5 | not captured | not captured | `compiler/src/dotty/tools/dotc/core/Types.scala:284` `dotty.tools.dotc.core.Types$.Type.derivesFrom` (tree 1031) |

Per-method detailed resolver attribution is not collected in semantic-only or cheap-counters mode.
Classloader load and AuditResolver detailed observations are full-mode-only; cheap counters contain resolver boundary measurements only.
