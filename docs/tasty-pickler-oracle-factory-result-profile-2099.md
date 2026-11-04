# Factory-result consumer startup profile (#2099)

## Decision

**No-go: retain the current invocation sequence.** Three comparable warm runs
of each consumer mode show a stable command profile, but no compiler, version,
reader, or runtime invocation is demonstrably redundant. The 23 failed Scala
compiler calls per run are separate expected-failure controls. The successful
calls use different class/TASTy roots, fixtures, or output directories to
prove independent properties. Combining these controls would lose diagnostic
attribution or fresh-output evidence. Toolchain revision, JDK, JAR SHA-256,
and version checks are required provenance gates. No source behavior was
changed and no subprocess was removed.

## Environment and method

Measurements were made before any source edit on the same pinned worktree and
fixtures, with dependencies warm:

- Scala checkout revision: `777528f19a58e794c9954a42f433373472ec57f8`
- Scala compiler: 3.9.0
- JDK: Temurin 21.0.8+9
- Compiler artifact SHA-256 values: matched `tests/fixtures/tasty_pickler/provenance.json`
- Commands: `cargo test --locked -p dotty --test jvm_tasty_factory_result_scala_consumer -- --nocapture`
  with `DOTTY_REQUIRE_SCALA_TASTY_FACTORY_RESULT_CONSUMER=1`, and
  `cargo test --locked -p dotty --test jvm_tasty_reference_local_scala_consumer -- --nocapture`
  with `DOTTY_REQUIRE_SCALA_TASTY_REFERENCE_LOCAL_CONSUMER=1`.
- Each invocation enabled `DOTTY_ORACLE_TIMINGS_FILE`. Telemetry files and
  full consumer reports were retained under the local ignored
  `target/issue-2099/` directory for this run; they are not checked-in inputs.

The reported wall time is the enclosing focused Rust test duration, which also
includes Rust artifact creation before the Python child starts. The sum of
`execute` event durations is reported separately and excludes filesystem work
between commands. Toolchain-stage timers are not added to command totals.

## Three paired warm runs

| Mode | Test wall time, runs 1–3 | Mean / median | Sum of `execute` durations, runs 1–3 |
| --- | --- | --- | --- |
| ordinary | 50.08 / 46.22 / 46.22 s | 47.51 / 46.22 s | 49.51 / 46.04 / 46.06 s |
| `--reference-local` | 47.82 / 46.39 / 46.74 s | 46.98 / 46.74 s | 47.63 / 46.23 / 46.58 s |

Each run emitted 54 `execute` events with the same counts in both modes:

| Category / operation | Status | Calls per run |
| --- | --- | ---: |
| `git` revision check | success | 1 |
| `java` compiler version | success | 1 |
| `java` compile | success | 12 |
| `java` compile | failure (expected by the negative control) | 23 |
| `java` decompile | success | 3 |
| `java` runtime | success | 6 |
| `javac` version | success | 1 |
| `javac` compile | success | 6 |
| `javap` version | success | 1 |

The five `toolchain_stage` events (Scala source pin, JDK validation, cached
JAR validation, compiler artifact SHA-256, and version probes) also succeeded
once each per run. Mean elapsed time for the 23 expected-failing `java compile`
calls was 27.37 s in ordinary mode and 27.01 s in reference-local mode; those
calls exercise separate failure expectations and are intentionally preserved.

## Consumer evidence

All six runs reported the pinned Scala revision and JDK, matched compiler
artifact hashes, unchanged original artifacts after Scala use, and the expected
runtime result (`7,-9,3` for ordinary; `scala39-reference-local-constructor-flow=ok`
for reference-local). Each mode also passed renamed/reordered outputs, separate
TASTy-only resolution and classfile-only rejection probes, decompilation, and
`-from-tasty` reconstruction.

The `negative_controls` object had this same 31-key set in every report:

```text
Counter.corrupt, Counter.get.classfile_only, Counter.get.tasty_only,
Counter.misplaced, Counter.missing, Counter.uuid_mismatch,
CounterOwner.corrupt, CounterOwner.misplaced, CounterOwner.missing,
CounterOwner.read.classfile_only, CounterOwner.read.tasty_only,
CounterOwner.uuid_mismatch, CounterOwnerFactory.corrupt,
CounterOwnerFactory.create.classfile_only,
CounterOwnerFactory.create.tasty_only,
CounterOwnerFactory.makeCounter.classfile_only,
CounterOwnerFactory.makeCounter.tasty_only, CounterOwnerFactory.misplaced,
CounterOwnerFactory.missing, CounterOwnerFactory.uuid_mismatch,
helper_result_descriptor_baseline, helper_result_descriptor_mismatch,
missing_helper_declaration, missing_required_artifact, no-class-def,
no-such-method, partial_six_file_batch, wrong_internal_owner,
wrong_owner_constructor_baseline, wrong_owner_constructor_signature,
wrong_package_owner
```

The complete top-level output-evidence key sets were stable across the three
runs of each mode. The ordinary report contains 32 keys:

```text
client_source_sha256, compile_classpath_order, compiler_artifacts_sha256,
counter_classfile_only_control_rejected, counter_tasty_only_get_resolved,
factory_classfile_only_create_control_rejected,
factory_classfile_only_make_counter_control_rejected,
factory_tasty_methods_decompiled, factory_tasty_only_create_resolved,
factory_tasty_only_make_counter_resolved, from_tasty_classes, java_version,
javac_version, javap_version, jdk_release, negative_controls,
owner_classfile_only_control_rejected, owner_tasty_only_read_resolved,
per_class_sidecar_matrix_rejected, renamed_artifact_hashes,
renamed_reordered_scala_consumer, renamed_runtime_outputs,
renamed_tasty_link_uuids, runtime_outputs,
runtime_outputs_are_original_rust_classes, rust_artifact_hashes,
rust_artifacts_unchanged_after_compile_and_run, scala_compiler_version,
scala_consumer, scala_revision, scala_source_sha256, tasty_link_uuids
```

Reference-local reports contain those keys plus the six local-flow details:
`factory_create_decompile_has_block_valdef_helper_and_local_use`,
`factory_create_result`, `helper_call`, `local_declaration`, `local_type`, and
`owner_constructor_argument` (38 keys total).

Expected rejection categories remained independent: Scala `E008` not-found or
missing-sidecar errors; corrupt TASTy and UUID-out-of-sync errors; `E007` ABI
result mismatch; `E173` inaccessible-constructor error; wrong internal-owner
and wrong-package errors; plus runtime `NoSuchMethodError` and
`NoClassDefFoundError`. Successful ABI baselines accompany the two targeted
ABI mismatch controls. No negative case was skipped or merged with another.

There is no before/after comparison because the evidence did not identify a
safe optimization candidate. The observed command startup and wall times are
therefore a baseline, not a performance claim.
