# Typer audit hot-path baseline (Scala 3.9.0)

This is a bounded, serial profile for issue #1482, not a full-corpus estimate and not a measurement of successful Scala compilation. It measures the Typer audit's first-blocker method attempts and the audit postprocessing performed for the selected files. No language behavior or production code changed.

## Reproduction

The measured dotty-rs revision is `621364dbcebda0e1dd32c0efde476b27ea8a0183`. The Scala sources are pinned to `777528f19a58e794c9954a42f433373472ec57f8`; the manifest is [`typer-hot-path-profile-manifest-3.9.0.tsv`](typer-hot-path-profile-manifest-3.9.0.tsv). Runtime artifacts are Scala 3.9.0 and the JDK is Temurin 21.0.8. The host was an AMD Ryzen 9 7940HS with 29.2 GiB RAM. Rust/Cargo were 1.98.1; Cargo's `test` profile was used.

The worktree and target cache were warm. Before every sample, the runner read all selected Scala inputs, pinned Scala JARs, and JDK jmods, then checked 122,241,024 resident input bytes with `fincore`. Both detailed repetitions used `CARGO_BUILD_JOBS=1` and `RUST_TEST_THREADS=1`.

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

Each sample runs `cargo test -p dotty-typer --test local_definition_audit --locked pinned_scala39_local_definition_audit -- --ignored --nocapture`. Detailed and timing-only profiles were run from the same dotty-rs commit and binary. The script rejected any difference in selected files, root-method identities, method outcomes, match-probe identities, or match-probe outcomes. Raw verbose logs remain under `target/issue-1482-final3-{detailed,timing-only}/run-*.log`; the checked-in TSVs are the compact machine-readable record.

## Measurements

The manifest selected four files. Two stable root-method trees were attempted per non-empty file (six roots per run), and one supported match probe was selected per non-empty file (three probes per run). `compileTimeOnly.scala` was the light control and had no local definitions. The exact method identities, source lines, outcomes, match probes, and phase data are in [`typer-hot-path-profile-3.9.0/`](typer-hot-path-profile-3.9.0/).

Detailed runs took 12,286 ms and 12,486 ms wall time (mean 12,386 ms); audit-internal wall time was 12,188 ms and 12,320 ms. Timing-only took 18,447 ms wall time and 18,340 ms audit-internal time. The single timing-only sample was 6,061 ms (48.9%) slower than the detailed mean, despite emitting far less output. This is inconsistent with the prior run and cannot establish an instrumentation cost or a speedup; environmental/runtime variance dominates this one timing-only sample. Timing-only skips both `AuditResolver` collection and classloader module/load profiling.

The detailed samples emitted 3,758,439–3,758,440 log bytes; timing-only emitted 8,200 bytes. Reading the captured detailed log took 2–3 ms and saving it took 1 ms; timing-only read/write time rounded to 0 ms. Test-side diagnostic emission remains inside the measured process interval. Child CPU averaged 11.01 s user and 1.27 s system in detailed mode, versus 13.83 s and 2.34 s for timing-only. The maximum sampled aggregate process-tree RSS was 107.9 MiB detailed and 97.2 MiB timing-only. RSS is the sum of per-process resident sets, so shared mappings may be counted more than once; 100 ms sampling may miss a shorter peak.

Phase totals across the two detailed repetitions were 12,123 ms for method typing and 11,804 ms for post-typing audit accounting; parse took 353 ms, inventory 23 ms, namer 2 ms, and Typer setup 2 ms. Classpath/JDK setup took 70–98 ms per run. Cargo/test startup, process sampling, and log emission account for the remainder shown in `run-timings.tsv`.

## Three measured hot paths

These are ranked by mean inclusive time per method attempt. All attempts stop at their first typer blocker; none completed successfully. Delegated member-resolution time is nested within each method time and must not be added again.

| Rank | Root method | Mean attempt | Member calls | Delegated member time | First blocker |
| ---: | --- | ---: | ---: | ---: | --- |
| 1 | `compiler/src/dotty/tools/dotc/ast/tpd.scala:262` `tpd$.DefDef` (tree 1790) | 2,090.5 ms | 470 | 2,083.3 ms | `TypeNameNotFound` |
| 2 | `compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala:42` `GenericSignatures$.mayNeedSignature` (tree 204) | 1,909.5 ms | 570 | 1,899.6 ms | `TypeNameNotFound` |
| 3 | `compiler/src/dotty/tools/dotc/core/Types.scala:284` `Types$.Type.derivesFrom` (tree 1031) | 1,887.5 ms | 1,687 | 1,866.8 ms | `MemberLookup` |

Across the bounded sample, delegated `resolve_member` took 12,018 ms across the two detailed runs, against 12,123 ms total method-typing time. The measured `AuditResolver` exclusive member-lookup work was 56.3 ms across both runs (about 0.47% of delegated member time). Rollbacks totaled 772 calls; they covered 13,328 module-operation and 6,506 member observations. Checkpoint and rollback times were each below 1 ms per run.

**Follow-up proposal:** focus the next bounded investigation on `ClasspathSymbolResolver::resolve_member` and `resolve_member_inner` in `crates/dotty-classloader/src/resolver.rs` (around lines 1351 and 797 at the measured revision). Split `prefix_target`, `resolve_target`, scope lookup, and class/module fallback time for the three roots above. Count repeated request keys before considering reuse; any candidate cache must be request-local to the current semantic store/loading session. The current data does not justify a cross-session semantic cache or a speedup estimate.

## Attribution limits

- Classpath counters cover the selected-file audit, including its separate match-probe pass; they are not assigned to individual method roots. They report logical `ClassResource` responses: per detailed run there were 15,480 positive and 81 negative inner lookups, 7,549 negative-cache hits, and 543,524,646 returned logical bytes. Those byte counts include repeated responses. The classloader API does not expose physical JAR reads or decompression counts, so they are not inferred.
- Most resolver time is delegated `resolve_member`; class completion, TASTy completion, and module completion were not called in these selected attempts. Classpath I/O is not the dominant measured portion: `find_class` wall time was about 1.32 s per run.
- The timing-only delta combines the removal of detailed `AuditResolver` observations and classloader load/module profiling. The wrapper table separates delegated resolver time from wrapper-exclusive work in detailed mode, but the current API does not split the classloader module-profile cost into subphases.
- `postprocess_ms` is measured separately from per-method typing, but this issue does not yet attribute its time to individual accounting functions.
- This four-file sample is intentionally bounded. It is not a full-corpus runtime estimate and makes no claim about successful compilation performance.

## Checks

- `cargo fmt --all -- --check`
- `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 cargo test -p dotty-typer --test local_definition_audit --locked`
- `python3 -m unittest discover -s tools/typer-classpath-corpus-audit -p 'test_*.py'`
- Both pinned-Scala profile variants completed and passed identity/outcome comparison.
- `git diff --check`
