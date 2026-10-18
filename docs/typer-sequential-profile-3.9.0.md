# Reproducible serial Typer profiling

Issue [#1280](https://github.com/scytrowski/dotty-rs/issues/1280) adds a bounded, opt-in profile for investigating Typer corpus cost without increasing concurrency. The checked-in [manifest](typer-sequential-profile-manifest-3.9.0.tsv) selects five canonical files from the pinned Scala 3.9.0 compiler and library trees. Every path must exist under the pinned checkout; the runner rejects missing, duplicate, traversal, or out-of-root paths.

## Reproduce

Provide the pinned Scala checkout, JDK 21.0.8, and the Scala 3.9.0 artifact cache. The profile command sets both `CARGO_BUILD_JOBS=1` and `RUST_TEST_THREADS=1` itself.

```sh
export SCALA39_ROOT=/path/to/scala-3.9.0
export JAVA_HOME=/path/to/jdk-21.0.8
export SCALA39_JDK_RELEASE=21
export COURSIER_MAVEN_ROOT=/path/to/coursier/repo1.maven.org/maven2
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --runs 2 \
  --output-dir target/typer-sequential-profile
```

The runner checks the Scala checkout SHA (`777528f19a58e794c9954a42f433373472ec57f8`), exact JDK version, pinned Scala artifact files, and manifest paths before starting Cargo. It runs only the ignored `pinned_scala39_local_definition_audit` test, filtered to those five files. Each pass is serial. The default is two passes; at most three are accepted. Do not add `--verify-determinism`, which runs the full audit twice.

For one additional high-resolution resolver pass after the repeated timing baseline, use the same command with `--runs 1` and a separate output directory. Resolver microseconds are diagnostic and should be reported separately from the two-run whole-millisecond baseline.

Raw logs are retained under the selected output directory for local analysis and are intentionally not checked in. To regenerate the compact reports from logs without running the audit again:

```sh
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --summarize-existing \
  --output-dir target/typer-sequential-profile
```

The report code checks that the repeated runs cover the same manifest paths and exact file/tree/method identities. It sorts timing tables by descending cost with stable file/tree tie-breakers. The opt-in test instrumentation is disabled unless `DOTTY_AUDIT_PROFILE_FILES` is set; normalized audit reports and determinism sidecars never include timers.

To measure cold then warm input page-cache behavior in one serial invocation, add `--cold-inputs-first-run`. This option evicts only the selected Scala sources, pinned Scala JARs, and JDK `.jmod` inputs before the first run using `POSIX_FADV_DONTNEED`, then checks resident bytes with util-linux `fincore`. The second run records the resulting warm residency. It does not clear the system-wide page cache. The default profile command does not evict input files.

## Frozen measurement

The [profile summary](typer-sequential-profile-3.9.0/index.md) and its TSV sidecars record two serial passes on the same five-file manifest, measured at dotty-rs commit `eafa385e508aa0eaa0710342b656d03d2df6acc0`.

The machine was an AMD Ryzen 9 7940HS mini PC with 30.6 GB reported RAM, Rust test profile, JDK 21.0.8, Scala 3.9.0 artifacts, and Cargo/test concurrency set to one. The Scala checkout was at the pinned SHA above. The two Cargo wall times were 223.908 s and 223.187 s; the in-test audit/setup times were 223.291 s and 222.544 s. The difference was 617 ms and 643 ms, including Cargo/test startup, harness, and writing the verbose raw profile log. Classpath/JDK/JAR setup took 90 ms and 73 ms; enumerating all corpus paths before filtering took 3 ms in both runs. Reading selected source files rounded to 0 ms at the current millisecond resolution.

Both measured runs used an already-built Cargo target. A cold Cargo build was not measured separately. The operating system page cache was not controlled, so cold and warm filesystem timings are unavailable. This profiling run did not invoke full normalized audit-report generation; the summary records the measured time to write this profile's own reports separately.

### Cold/warm cache follow-up

The [Cargo target cache report](typer-sequential-profile-3.9.0-build-cache-check/summary.md) and [input page-cache report](typer-sequential-profile-3.9.0-cache-check/summary.md) record two bounded follow-ups on the same five-file manifest and machine. With an empty Cargo target, the first run took 299.287 s wall / 223.298 s in the audit; the second run with the compiled target took 222.602 s / 221.952 s. The 76.685 s wall difference is Cargo compilation and startup, while measured audit time differed by 1.346 s.

A separate pair requested eviction only for the selected Scala sources, Scala JARs, and JDK jmods, then measured residency with `fincore`. The first run took 252.582 s and the second, after those inputs had been read, took 220.461 s. Eviction was partial: 41,734,144 bytes were resident before the request and 35,147,776 bytes remained afterward. The report labels this `partial-eviction`; it does not present it as a fully cold filesystem run. Global page-cache clearing was avoided. The complete profile writer timer now includes all TSV sidecars and the summary; this run measured 24 ms. The earlier 11 ms figure stopped before those outputs were written and is superseded.

The largest cost in this cohort was `tpd.scala` at a mean 222.3 s per pass. Across the two passes, method typing accounted for 338.855 s (76.0% of measured per-file phase time), and post-typing audit accounting accounted for 106.409 s (23.9%). The slowest stable method identity was `tpd.scala#tree=63`, `dotty.tools.dotc.ast.tpd$.ta`, at a mean 3.536 s per attempt. Generating the compact profile report took about 11 ms. This is a ratio for the focused profile report, not a measurement of the full-corpus report writer.

The resolver observed 212,018 member requests with 25.616 s of inclusive lookup time, and 112,178 package requests with 4 ms. Across the two runs, class-load profiles recorded 32 full completions, 66 identity-shell creations, 138 reuses, and zero completed-cache hits. The largest request roots and their source method identities are listed in `request-roots.tsv`.

These numbers describe a deliberately expensive five-file sample, not a full-corpus duration estimate. `tpd.scala` alone dominates this sample. The historical #1180 import-prefix profile selected 209 different files and took 288.468 s / 288.037 s; it spent about 286 s per pass in parsing, naming, and typing and about 0.3 s in reconciliation. Those import-prefix timings are a different workload and do not measure the full audit's bottleneck. The full pinned audit processes 1,236 files and has previously taken around 40 minutes per pass. This issue did not run another full audit.

## Findings and safe boundaries

The three measured follow-up candidates are:

1. SourceTyper method typing: 338.855 s across the selected methods. Start with the stable hot method identities and measure a narrow internal phase before optimizing. Any semantic cache must remain scoped to its `SemanticStore` and `LoadingSession`.
2. Post-typing audit accounting: 106.409 s. Break down source-function projection, blocker attribution, and probe work before changing it. Preserve each baseline row, diagnostic category, and rollback behavior.
3. Resolver member lookup: 212,018 requests and 25.616 s inclusive time. Investigate reuse only at immutable classpath-resource/query boundaries. Do not reuse `SymbolId`, `TypeId`, `ScopeId`, `NameInterner`, source-package ownership, or transaction state across stores or sessions.

Classloader profiles expose completed-cache hits, full completions, identity-shell create/reuse counts, hierarchy requests, and stable load roots. They do not expose every lookup miss. The initial frozen run predates format-specific class-completion counters and emitted resolver durations only in whole milliseconds per method; therefore sub-millisecond resolver totals appear as zero in that snapshot and are labeled as below resolution, not exact zero. A third pass with microsecond instrumentation measured member, package, and Object→ModuleClass costs. Its 16 full class completions occurred within member resolution; the direct completion hook saw no classfile or TASTy symbols, so those 16 resources still have no reliable format split. The direct hook counters do not represent all classpath loads. Classfile parsing, TASTy decoding, identity-shell work, and semantic completion remain inclusive nested operations; their durations must not be added together as if disjoint.

The profile counters are diagnostic evidence only. A repeated request or cache hit count does not prove that session-specific symbols or types are safe to cache across source files. No semantic behavior or audit baseline rows changed in this issue.
