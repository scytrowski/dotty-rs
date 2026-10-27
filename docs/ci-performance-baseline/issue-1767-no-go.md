# Issue #1767: integration-test consolidation no-go

**Decision:** retain the existing integration-test targets. A proof of concept (POC) merged four compatible `dotty-tasty-pickler` suites into one harness, but the measured workspace build-plus-test result was slower. The POC also changed 84 test filter names by adding module prefixes. The code experiment was reverted; this report records its evidence and the no-go decision.

## Scope and method

Baseline and candidate used source revision `554d457ca52212c68be6db98ed9f17e069296893`; the candidate differed only in `dotty-tasty-pickler` test target wiring and module declarations. Both used Rust 1.98.1 (`48a229ceaefd4985c50990b14116b6d856af0985`), `x86_64-unknown-linux-gnu`, the same host, warm Cargo registry, locked dependencies, fresh target directories, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and test `opt-level=1`. No hosted full Typer corpus audit was run.

The cold-build comparison used the same process-tree RSS watcher on both states. The build command was:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=1 \
  CARGO_TARGET_DIR=<fresh-target> cargo test --workspace --all-targets --locked \
  --no-run --timings --message-format=json
```

Each state then had three suite runs using the same environment and `cargo test --workspace --all-targets --locked`, reusing that state's freshly built target directory. Wall times are measurements from this host, not CI promises.

## Workspace measurements

| Measure | Baseline | POC | Change |
| --- | ---: | ---: | ---: |
| Test-profile executables | 153 | 150 | -3 |
| Integration-test executables | 126 | 123 | -3 |
| Test-profile executable bytes | 464,668,064 | 458,774,512 | -5,893,552 (-1.27%) |
| Integration-test executable bytes | 378,463,264 | 372,569,712 | -5,893,552 (-1.56%) |
| Cold workspace compile wall time, process-tree monitored | 109.662 s | 115.026 s | +5.364 s (+4.89%) |
| Peak process-tree RSS during cold compile | 6,719,604 KiB | 6,605,416 KiB | -114,188 KiB (-1.70%) |
| Peak single-process RSS during cold compile | 1,807,492 KiB | 1,768,420 KiB | -39,072 KiB (-2.16%) |
| Full workspace test wall time, 3-run mean | 16.229 s | 16.201 s | -0.028 s (-0.17%) |
| Combined measured compile plus test mean | 125.891 s | 131.227 s | +5.336 s (+4.24%) |

Suite times were baseline **15.978, 16.320, 16.389 s** and POC **16.244, 16.126, 16.232 s**. The 0.17% suite-time difference is within run noise and does not offset the slower cold compile. A separate unmonitored baseline build took 73.434 s; it is excluded because no matching unmonitored candidate result is available.

Cargo reported selected compile-unit times of `bodies` 2.23 s, `plan_validation` 8.74 s, `projection` 2.76 s, `skeleton` 2.78 s, and candidate `emission_plans` 15.99 s. These units overlap in the Cargo critical path, so they are not summed as workspace time. The four original binaries totaled 9,358,208 bytes; the consolidated POC binary was 3,464,664 bytes. Artifact reduction did not improve the workspace critical path.

## Candidate ranking

The all-target inventory ranks measured baseline compile units and artifact sizes. The largest observed units were:

| Rank | Package / target | Unit time | Binary size | Why not combined in this POC |
| ---: | --- | ---: | ---: | --- |
| 1 | `dotty-typer` / `local_definition_audit` | 49.11 s | 15,073,464 B | Large typer audit with repository-root and class-file fixtures; distinct audit and test-selection boundary. |
| 2 | `dotty-typer` / `package_prefix_parity` | 48.94 s | 9,822,616 B | Typer parity contract with cross-crate TASTy and class-file fixtures. |
| 3 | `dotty-tasty-pickler` / `source_pipeline` | 46.41 s | 8,783,344 B | Repeated Python oracle subprocesses and a distinct source-fixture set; keep its process and fixture boundary. |
| 4 | `dotty` / `classpath_source_typing` | 38.44 s | 8,621,800 B | Root classloader and classpath integration contract, outside the pickler package. |
| 5 | `dotty` / `jvm_source_compilation` | 31.95 s | 8,518,640 B | End-to-end JVM emission consumer and JDK verification/execution gate. |

The POC suites were lower in raw timing, but formed the safest same-package experiment: `bodies` (2.23 s), `plan_validation` (8.74 s), `projection` (2.76 s), and `skeleton` (2.78 s). Together they represented four separately linked targets and 9,358,208 binary bytes. They share test support and use in-memory state; inspection found no working-directory or environment changes, global mutable state, locks, subprocesses, custom harness flags, or thread coordination. The full candidate ranking, including the selected suites and reasons for keeping other expensive targets separate, is in the compressed inventory.

## Test inventory and selection behavior

Both full workspace runs reported **7,064 passed, 0 failed, and 18 ignored**. Cargo emitted 149 harness summaries before and 146 after, matching the reduction of three targets. Per-target `--list` inventory had 1,966 integration test cases in each state. The baseline and candidate testcase/status multisets match after applying the four explicit module-prefix mappings. Thirteen ignored integration tests remained ignored in both states; the other five ignored tests are outside integration targets.

The candidate combined `bodies`, `plan_validation`, `projection`, and `skeleton`; `class_state_plan` and `source_pipeline` remained separate. Compile-time fixture references remained at their original relative paths, including `tests/fixtures/tasty_pickler/Answer.scala` and `tests/fixtures/tasty_pickler/tasty/example/Answer.tasty`. The package's manifest dependency list was unchanged. Per-target source paths, executable paths relative to each fresh target directory, binary sizes, measured unit times, manifest dependencies, directly included compile-time fixtures, testcase names/counts, ignored status, and the cost-ranked candidate list are recorded in the archive.

The 84 moved tests gained prefixes such as `bodies::` and `plan_validation::`. Their baseline-to-candidate mapping is included in the archive. Existing selection such as `cargo test -p dotty-tasty-pickler --test bodies <filter>` becomes a filter on `--test emission_plans` with a module prefix. This test-selection cost has no measured build-plus-run improvement to justify it.

The full workspace test run exercised positive and negative assertions, fixture comparisons, and the current Scala/JVM consumer tests. The POC changed test target wiring only, not compiler, fixture, Scala oracle, or JVM implementation code. The dedicated JDK 21 gate was not run for this experiment.

## Reproduce and inspect

- Full integration target and testcase inventory, measured artifact/timing/dependency/fixture data, candidate ranking, and renamed-test map: [`issue-1767-inventory.tar.gz.base64`](issue-1767-inventory.tar.gz.base64). Decode with `base64 -d`, then inspect with `tar -tzf`. SHA-256 of the decoded archive: `6f2d28d26e37795bc34df1816bc35c8273118b9157a57c92419185f721cb2e61`.
- Inventory command: deduplicate Cargo JSON artifacts by executable path, then run each executable with `--list` and `--list --ignored`.
- Build command: `cargo test --workspace --all-targets --locked --no-run --timings --message-format=json` with the variables above and a fresh `CARGO_TARGET_DIR`.
- Test command: `cargo test --workspace --all-targets --locked` with the same profile variables.
