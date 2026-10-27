# Issue #1767: integration-test consolidation no-go

**Decision:** keep the existing integration-test target structure. The proof of concept (POC) consolidated four compatible `dotty-tasty-pickler` suites into one harness, but the controlled workspace build-plus-test result was slower. The consolidation also changed 84 test names by adding module prefixes, changing existing filter strings. The POC was reverted.

## Scope and method

The before/after source was pinned to `554d457ca52212c68be6db98ed9f17e069296893`; the only candidate source changes were the POC target declarations and module imports in `crates/dotty-tasty-pickler`. Both runs used Rust 1.98.1 (`48a229ceaefd4985c50990b14116b6d856af0985`), `x86_64-unknown-linux-gnu`, the same host and warm Cargo registry, fresh target directories, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_TEST_DEBUG=0`, test profile `opt-level=1`, and locked dependencies. No hosted full Typer corpus audit was run.

The monitored cold-build pass used the same process-tree RSS watcher on both source states. Commands were equivalent to:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_OPT_LEVEL=1 \
  CARGO_TARGET_DIR=<fresh-target> cargo test --workspace --all-targets --locked \
  --no-run --timings --message-format=json
```

Suite execution used the same variables and `cargo test --workspace --all-targets --locked`; the three runs per state reused that state's freshly built target directory. Wall times below are measurements from this host, not CI promises.

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

Suite times were baseline **15.978, 16.320, 16.389 s** and POC **16.244, 16.126, 16.232 s**. The tiny suite-time difference is within normal run noise and does not offset the slower cold compile. A separate unmonitored baseline build took 73.434 s; it is excluded from the comparison because there is no matching unmonitored candidate result.

Cargo's selected compile-unit timings were `bodies` 2.23 s, `plan_validation` 8.74 s, `projection` 2.76 s, `skeleton` 2.78 s, and candidate `emission_plans` 15.99 s. These units overlap in the Cargo critical path and are not summed as workspace time. The four original binaries totaled 9,358,208 bytes; the consolidated POC binary was 3,464,664 bytes. It saves artifacts, but does not improve the measured workspace critical path.

The cost-ranked inventory records all 126 baseline integration targets and 123 POC targets, with 125 and 122 measured Cargo test-unit durations respectively. It also records each package/target, testcase and ignored counts, source path, executable path relative to its fresh target directory, binary bytes, package manifest dependencies, and direct compile-time fixture includes. The raw timing leader rows show `local_definition_audit` (49.11 s, 15,073,464 bytes, 129 tests), `package_prefix_parity` (48.94 s, 9,822,616 bytes, 14 tests), and `source_pipeline` (46.41 s, 8,783,344 bytes, 60 tests). They were not selected for consolidation: the typer audits/parity target carry separate typer and binary-fixture contracts, while `source_pipeline` runs repeated Python oracle subprocesses and uses a distinct source-fixture set. Combining them would enlarge the harness and alter test/process selection boundaries. The four selected suites were lower in raw timing, but were the safe same-package candidate: together they accounted for four separately linked targets, 9,358,208 bytes, and shared support while using only in-memory state. The archive includes the cost-ranked candidates and reasons for retaining them separately.

## Test inventory and selection behavior

Both full workspace runs reported **7,064 passed, 0 failed, and 18 ignored**. Cargo emitted 149 harness summaries before and 146 after, exactly reflecting the three-target reduction. Integration-target `--list` inventory contained 1,966 test cases in each state; the baseline and POC testcase/status multisets match after removing the four intentional module prefixes. Thirteen ignored integration tests remained ignored in both states; the other five ignored tests are outside integration targets.

The POC combined `bodies`, `plan_validation`, `projection`, and `skeleton`; `class_state_plan` and `source_pipeline` remained separate. All four suites used in-memory test state and the package's existing support module. They did not change the process working directory, environment, global mutable state, locks, subprocesses, thread behavior, or harness flags. Existing compile-time fixtures remained at their original relative paths: `tests/fixtures/tasty_pickler/Answer.scala` and `tests/fixtures/tasty_pickler/tasty/example/Answer.tasty`. Direct package/test dependency closure was unchanged: `dotty-core`, `dotty-tasty`, `dotty-lexer`, `dotty-parser`, `dotty-namer`, and `dotty-typer` (plus the package's existing transitive dependencies).

The 84 moved tests gained prefixes such as `bodies::` and `plan_validation::`. Their names and mapping are included in the inventory archive, along with every integration target, testcase, ignored status, target source/executable path, artifact size, measured Cargo unit time, package manifest dependency list, direct compile-time fixture includes, and candidate ranking. This changes existing `cargo test -p dotty-tasty-pickler --test bodies <filter>` selection commands into filters on `--test emission_plans` with a module prefix; the documented mapping preserves traceability, but the behavior is a developer-ergonomics cost without a measured build-plus-run win.

The full workspace test run covered positive and negative assertions, fixture comparisons, and the current Scala/JVM consumer tests. The POC touched only test target wiring; it did not alter compiler, fixture, Scala-oracle, or JVM implementation code, so no separate JDK gate was triggered by this experiment.

## Reproduce and inspect

- Exact target/testcase inventory and rename map: [`issue-1767-inventory.tar.gz.base64`](issue-1767-inventory.tar.gz.base64) (decode with `base64 -d`, then inspect with `tar -tzf`). SHA-256 of decoded archive: `6f2d28d26e37795bc34df1816bc35c8273118b9157a57c92419185f721cb2e61`.
- Inventory command: build each target executable from Cargo JSON artifacts, then run `<executable> --list` and `<executable> --list --ignored`; deduplicate by executable path.
- Build command: `cargo test --workspace --all-targets --locked --no-run --timings --message-format=json` with the variables above and a fresh `CARGO_TARGET_DIR`.
- Test command: `cargo test --workspace --all-targets --locked` with the same profile variables.
