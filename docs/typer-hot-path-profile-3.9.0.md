# Typer audit hot-path baseline (Scala 3.9.0)

This is a bounded, serial profile for issue #1482. It measures the audit's first-blocker root-method attempts and its separate match-selector/case-typing pass on four selected files. It is not a full-corpus estimate or a measurement of successful Scala compilation. No language behavior or production code changed.

## Reproduction

The measured dotty-rs revision is `4a5f205b60074b678ee9b9eef80d3e7d757befd9`. Scala sources are pinned to `777528f19a58e794c9954a42f433373472ec57f8`; the manifest is [`typer-hot-path-profile-manifest-3.9.0.tsv`](typer-hot-path-profile-manifest-3.9.0.tsv). Runtime artifacts are Scala 3.9.0 and Temurin 21.0.8. The host was an AMD Ryzen 9 7940HS with 29.2 GiB RAM; Rust/Cargo were 1.98.1. Cargo and test concurrency were fixed at one.

The profile runner builds the exact `local_definition_audit` integration-test binary with `cargo test ... --no-run` before starting measured samples. It records prebuild duration/status in `prebuild.tsv`; each measured sample is rejected if Cargo output contains `Compiling`. Both detailed repetitions and the timing-only sample used the warm Cargo target cache. Input page-cache warmup read the selected Scala sources, pinned Scala JARs, and JDK jmods, with 122,241,024 resident bytes recorded after setup.

```sh
SCALA39_ROOT=/path/to/scala39-source \
JAVA_HOME=/path/to/temurin-21.0.8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/repo1.maven.org/maven2 \
CARGO_TARGET_DIR="$PWD/target" \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 2 --method-limit-per-file 2 --match-limit-per-file 1 \
  --output-dir target/issue-1482-detailed

SCALA39_ROOT=/path/to/scala39-source \
JAVA_HOME=/path/to/temurin-21.0.8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/repo1.maven.org/maven2 \
CARGO_TARGET_DIR="$PWD/target" \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 1 --method-limit-per-file 2 --match-limit-per-file 1 \
  --timing-only --compare-with target/issue-1482-detailed \
  --output-dir target/issue-1482-timing-only
```

The measured command is `cargo test -p dotty-typer --test local_definition_audit --locked pinned_scala39_local_definition_audit -- --ignored --nocapture`. The exact measured binary is prebuilt outside the sample interval. Timing-only and detailed runs were checked for matching selected files, method identities/outcomes, and match-probe identities/outcomes. Compact checked-in records are in [`typer-hot-path-profile-3.9.0/`](typer-hot-path-profile-3.9.0/); raw logs and the captured multi-megabyte detailed output remain under ignored `target/issue-1482-review-{detailed,timing-only}/` paths.

## Measurements

The manifest selected four files, six root-method observations per run, and three match probes per run. `compileTimeOnly.scala` was the light control and had no local definitions. The detailed samples took 12,191 ms and 12,184 ms wall time (mean 12,187.5 ms); audit-internal wall time was 12,079 ms and 12,069 ms. The timing-only sample took 12,083 ms wall time and 12,018 ms audit-internal time. All measured runs used a warm Cargo cache and had no rebuild. The exact prebuilds took 32 ms (detailed) and 31 ms (timing-only), outside their measured intervals.

Detailed samples emitted 3,763,527 log bytes each; timing-only emitted 8,864 bytes. Reading/saving the captured detailed log took 2/1 ms per sample. Child CPU averaged 10.75 s user and 1.35 s system in detailed mode, versus 10.69 s and 1.33 s in timing-only. Maximum sampled aggregate process-tree RSS was 109.3 MiB detailed and 99.2 MiB timing-only. This is one timing-only sample, so the difference is not a stable speedup estimate. RSS sums per-process resident sets and may double-count shared mappings; 100 ms sampling may miss shorter peaks.

Across the two detailed repetitions, root-method typing took 11,950 ms total and the independent match pass took 11,581 ms total. Parse took 348 ms; true audit postprocessing took 26 ms. Thus the previously reported ~11.7 s “postprocess” was mostly match typing mislabeled under that phase; it is now independently timed and attributed. The match pass spent 11,511,424 µs in delegated member lookups across the two runs, while its measured AuditResolver wrapper-exclusive time was 47,050 µs. It made 5,434 member calls and 10,066 module-identity calls. Match-pass resolver and classpath rows are separate from root-method resolver rows in [`match-resolver.tsv`](typer-hot-path-profile-3.9.0/match-resolver.tsv) and [`match-resolver-wrappers.tsv`](typer-hot-path-profile-3.9.0/match-resolver-wrappers.tsv).

The first-blocker root methods remain dominated by delegated member resolution:

| Root method | Mean attempt | Member calls | Delegated member time | First blocker |
| --- | ---: | ---: | ---: | --- |
| `compiler/src/dotty/tools/dotc/ast/tpd.scala:262` `tpd$.DefDef` (tree 1790) | 2,063.5 ms | 470 | 2,056.7 ms | `TypeNameNotFound` |
| `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala:42` `GenericSignatures$.mayNeedSignature` (tree 204) | 1,874.0 ms | 570 | 1,864.5 ms | `TypeNameNotFound` |
| `compiler/src/dotty/tools/dotc/core/Types.scala:284` `Types$.Type.derivesFrom` (tree 1031) | 1,866.5 ms | 1,687 | 1,843.7 ms | `MemberLookup` |

The delegated resolver duration is nested inside method time and must not be added again. Across these measured roots, AuditResolver wrapper-exclusive member lookup was about 47 ms total, so wrapper bookkeeping does not explain the multi-second attempts. The separate match pass also spends most of its observed time in delegated member lookup. Classpath `find_class` time was about 1.19 s across two match passes, and the classpath API reports logical resources/bytes rather than physical archive reads or decompressions.

**Follow-up proposal:** profile resolver-internal work in `ClasspathSymbolResolver::resolve_member` / `resolve_member_inner` in `crates/dotty-classloader/src/resolver.rs`, separating prefix/package traversal, classpath lookup, and member selection for the measured roots and match pass. Count repeated request keys before considering reuse. These data do not justify a cross-session semantic cache or claim any optimization speedup.

## Attribution limits

- This four-file sample is intentionally bounded. It does not estimate full-corpus runtime or successful compilation performance.
- Detailed resolver and classloader instrumentation produces multi-megabyte diagnostic output; timing-only omits those counters by design. The single timing-only comparison is an observer-effect estimate, not a performance claim.
- Classpath counters report logical resource responses and returned bytes, not physical JAR reads/decompressions. Nested class/TASTy loading remains inclusive inside resolver calls.
- The current implementation does not split classfile parsing, TASTy decoding, shell creation, and semantic completion into disjoint CPU intervals. Do not add inclusive timers together or infer safe cross-store caching from reuse counts.

## Checks

- `cargo fmt --all -- --check`
- `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 cargo test -p dotty-typer --test local_definition_audit --locked`
- `python3 -m unittest discover -s tools/typer-classpath-corpus-audit -p 'test_*.py'`
- Both pinned-Scala profile variants completed; exact identities and outcomes matched, and neither measured sample rebuilt the binary.
- `git diff --check`
