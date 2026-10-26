# Typer TASTy hot-path attribution (Scala 3.9.0)

## Scope and method

This report closes issue #1569 as an evidence-only result. It uses the bounded
four-file manifest `typer-hot-path-profile-manifest-3.9.0.tsv`, Scala checkout
revision `777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts,
Temurin JDK 21.0.8, and the same host/build profile for three serial warm
samples on fresh main (`c31bba059c00a7a13f6a80ed689f90762808100a`) and three
samples with the opt-in TASTy observer (`28fa9a50f88a207ed34d256d8654145bc5c55b41`).
Cargo/test concurrency was one. The exact six method observations, three match
probes, and their outcomes are retained in the evidence directories below.

The observer separately times TASTy parse, AST address indexing, definition
selection, parent resolution, constructor/accessor extraction, and member
projection. Parent alias/bare and constructor/member bare callbacks retain a
stable textual key, elapsed time, outcome, and each candidate classpath probe.
Resource rows include binary identity, TASTy version, size bucket, outcome,
and observation count. Timers are opt-in with classload profile collection;
there is no production-default profiling.

## Results

| Measure | Fresh main | Detailed observer | Difference |
|---|---:|---:|---:|
| Mean measured Cargo wall time | 19,442.7 ms | 19,037.0 ms | -405.7 ms (-2.1%) |
| Mean child CPU user time | 17.63 s | 17.19 s | -0.44 s |
| Mean child CPU system time | 1.75 s | 1.74 s | -0.01 s |
| Maximum sampled process-tree RSS | 117,792,768 bytes | 119,238,656 bytes | +1,445,888 bytes |
| Selected method observations | 6 | 6 | same |
| Selected match probes | 3 | 3 | same |

The wall and CPU differences are within run-to-run noise and do not indicate a
speedup or precisely quantify observer overhead. RSS sums per-process resident
sets, may double-count shared mappings, and was sampled every 100 ms. The exact
method identities/outcomes, match-probe identities/outcomes, and focused test
semantics matched between variants.

Across the three instrumented repetitions, phase totals were:

| Profile scope | Parse | AST index | Definition selection | Parent resolution | Constructor/accessor | Member projection |
|---|---:|---:|---:|---:|---:|---:|
| Source method loads | 10.3 ms | 84.2 ms | 1.2 ms | 51.1 ms | 0.02 ms | 24.16 s |
| Match-probe loads | 9.5 ms | 81.5 ms | 1.1 ms | 48.4 ms | 0.02 ms | 22.77 s |

These are inclusive attribution totals across classload observations and may
overlap; they are not disjoint CPU intervals. In the match profile the resolver
recorded 41 module-completion calls per run (123 total) and 8.575440, 8.733201,
and 8.686223 seconds of delegated completion time in runs 1–3 respectively
(25.995 seconds total).

The instrumented callback TSV records per-key outcomes and every candidate
probe. Across the three match-probe files and three repetitions, member
projection made nine unresolved bare-name callback observations. Parent
alias/bare callbacks were not invoked in this selected cohort. The key
`bare|dotty/tools/dotc/util|T` repeated within the same source and match decode
profiles; each observation probed the same three candidates, all missing. The
profile does not establish that the probes are semantically redundant across
all stores, retries, or resource snapshots, so no cache or behavior change was
made. All selected TASTy resources were version 28.9.0 and decoded
successfully; observed sizes cover 0–4 KiB, 4–64 KiB, and 64 KiB–1 MiB.
The rows distinguish logical decoder observations from physical JAR reads.

The detailed observer found member projection to be the largest measured
phase, but did not prove a safely removable operation. This is an evidence-only
closeout; a follow-up optimization would need to prove stable resolution across
the relevant store/resource lifetime and show paired improvement beyond noise.

## Retained evidence

The checked-in normalized TSVs omit the large raw run logs:

- [Fresh-main baseline evidence](typer-tasty-hotpath-evidence/baseline/)
- [Instrumented evidence](typer-tasty-hotpath-evidence/instrumented/)

`run-timings.tsv` contains per-run wall/CPU/RSS data. `methods.tsv`,
`method-outcomes.tsv`, `match-probes.tsv`, and `match-probe-counts.tsv` retain
the exact selection and outcome comparison. The instrumented directory also
contains `tasty-phases.tsv`, `match-tasty-phases.tsv`, `tasty-resources.tsv`,
and `tasty-resolution.tsv` with phase, resource, callback-key, and individual
classpath-probe observations. `variant-comparison.tsv` records the paired
summary. The normalized method identity/outcome and match-probe files compare
byte-for-byte; timing-bearing files intentionally retain their observed values.

## Reproduction inputs

- Scala source checkout: `777528f19a58e794c9954a42f433373472ec57f8`.
- Scala artifacts: Maven coordinates at version 3.9.0. SHA-256:

  | Artifact | SHA-256 |
  |---|---|
  | `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
  | `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
  | `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
  | `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
  | `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |

- JDK: Temurin 21.0.8, `JAVA_HOME=/home/vdrake6/.jdks/jdk-21`; SHA-256 of its
  `release` metadata file:
  `93b52921a5efb10ac0dcf9e3c955fa209fc83687df897d6ab034af967e2f5088`.
- Host: AMD Ryzen 9 7940HS with Radeon 780M Graphics; Cargo test profile;
  Cargo/test concurrency fixed at one.
- Instrumented implementation source: `28fa9a50f88a207ed34d256d8654145bc5c55b41`.
  The audit parser in `sequential_profile.py` additionally writes the stable
  callback/probe TSV represented in this PR.

The serial runner prebuilds the audit integration-test binary before measured
runs, uses a worktree-local temporary directory, and warms selected inputs. A
reproduction from this checkout is:

```sh
export SCALA39_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
export SCALA39_JDK_RELEASE=21
export JAVA_HOME=/home/vdrake6/.jdks/jdk-21
export COURSIER_MAVEN_ROOT=/home/vdrake6/.codex/worktrees/bdf3/dotty-rs/target/scala39-maven
export CARGO_TARGET_DIR="$PWD/target/issue-1569/cargo"
export TMPDIR="$PWD/target/issue-1569/tmp"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"
mkdir -p "$TMPDIR"

python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 3 --method-limit-per-file 2 --match-limit-per-file 1 --mode full \
  --output-dir target/issue-1569/instrumented
```

For the baseline, check out `c31bba059c00a7a13f6a80ed689f90762808100a` in a
separate worktree, use the same environment and arguments, and set
`--output-dir target/issue-1569/baseline`. On the instrumented run, also pass
`--compare-with target/issue-1569/baseline` to have the tool assert exact
selected method/probe identity and outcome parity. The tool's `--help` output
lists the complete supported options.

## Regression and focused controls

The classloader tests cover successful Scala TASTy decode and module completion,
malformed alias TASTy with callback-error provenance, classpath probe outcomes
for found/ambiguous/missing/error candidates, changed-resource validation,
separate stores, and rollback/retry paths. The bounded corpus audit checks
selected identity and outcome parity across fresh main and the observer.

## Validation

- `cargo test -p dotty-classloader --lib --locked`: 446 passed, 4 ignored.
- `cargo clippy -p dotty-classloader --all-targets --locked -- -D warnings`:
  passed.
- `cargo check -p dotty-typer --test local_definition_audit --locked`: passed.
- `cargo fmt --all -- --check`, `python3 -m py_compile
  tools/typer-classpath-corpus-audit/sequential_profile.py`, and
  `python3 tools/check-frontend-boundaries.py`: passed.
- Three serial Scala 3.9 cohort runs per variant with exact method/probe
  comparison: passed.

The bounded manifest is a focused attribution cohort, not a full-corpus
estimate. No optimization is claimed or included.
