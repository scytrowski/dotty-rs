# Real-source method acceptance sprint closeout, Scala 3.9.0

Status: **SPRINT GOAL UNMET**. The integrated real-source cohort contains 25
ordinary member methods: 10 were already fully typed at the pre-#1706 baseline,
and the final audit still types those same 10. No additional method completed
its signature, body, result conformance, owned typed `DefDef`, and exact
source-to-typed mapping. This report closes the evidence scope; it does not
claim general Scala method acceptance.

## Scope and pinned inputs

This closeout integrates #1704, #1705, and #1706 on the post-merge `main`
commit listed in [`typer-real-method-acceptance-inputs-3.9.0.tsv`](typer-real-method-acceptance-inputs-3.9.0.tsv).
The pre-sprint Tier A baseline is the frozen 25-method audit at
`bbd93722f66ce177788cdb8e63a147c510ddf74c` (after #1704 and before #1705/#1706).
The final tree is the merged #1714 `main` tree. Scala source is pinned to
`777528f19a58e794c9954a42f433373472ec57f8`; Scala 3.9.0 artifacts and their
SHA-256 hashes, Temurin JDK 21.0.8+9, Rust/Cargo versions, host, and audit
profile are recorded in the inputs TSV.

The cohort was selected from 9,388 canonical ordinary methods with bodies
found in 649 `library/src/scala` files. Tier A audits only the frozen 25
`path#tree` identities; this does not type all 9,388 candidates. The broader
1,236-file corpus audit is **NOT RUN**, as required by #1707's local-only scope.
Synthetic expression tests and isolated application successes do not enter the
ordinary-method numerator.

## Tier A result

Two independent pinned local audit invocations each ran two serial repetitions
and compared normalized reports byte-for-byte. Each selected and attempted all
25 methods, typed 10, blocked 15, excluded 0, and left 0 unattempted. The two
current reports match (SHA-256
`5f02baf960893bc5b9265481a73251a9a61ec125737bffa109eca4c6c9af203b`). The
per-method comparison is in
[`typer-real-method-acceptance-sprint-3.9.0.tsv`](typer-real-method-acceptance-sprint-3.9.0.tsv):

| Tier | Eligible methods in frozen cohort | Baseline fully typed | Final fully typed | Newly typed | Blocked at final |
| --- | ---: | ---: | ---: | ---: | ---: |
| A0 | 18 | 10 | 10 | 0 | 8 |
| A1 | 3 | 0 | 0 | 0 | 3 |
| edge controls | 4 | 0 | 0 | 0 | 4 |
| **Total** | **25** | **10** | **10** | **0** | **15** |

All 25 source identities match. All 10 previously typed methods preserve their
exact typed-tree IDs and normalized actual result shapes. The other 15 retain
the same blocker family and normalized first-error provenance. One blocked
`Tuple.toArray` row allocates different session-local `TypeId`s in the two
runs; its error variant, source tree, argument index, and blocker are unchanged.
No method advanced to a later blocker, regressed, or became newly typed.

The 10/25 result is 40% acceptance within this deliberately selected cohort;
it is not a 40% estimate for the standard library. The stretch target of at
least one newly fully typed standard-library method (5–10 aspirational) is
**UNMET**. The issue's required whole-method gate remained enabled, including
canonical signature, full RHS, result conformance, owned typed `DefDef`, and
exact source/typed index mapping.

## Function-value application and `IterableOnce`

The two frozen local roots in `IterableOnceOps.foldl` and `foldr` remain 0/2
fully typed. Their enclosing-method audit rows still use the broad
`ApplicationCalleeNotMethod` label, so that label alone does not show the
progress made by #1706. The dedicated nested regression confirms the outer
canonical `Function2.apply` now resolves and the first failing operation has
moved to the inner generic `IndexedSeq.apply` call. The two exact source roots, outer attempts, before/after status, and blocker
location are recorded in
[`typer-real-method-acceptance-function-value-3.9.0.tsv`](typer-real-method-acceptance-function-value-3.9.0.tsv).
The pinned Scala 3.9.0 oracle compiled all eight selected real source files and
the positive control; the negative control failed with E007 as expected.

## Frozen historical controls

The three-file smoke selected 31 local methods across 3 files and still typed
0. The pinned `package-term` control reconciled all 369 frozen rows from 60
files and 158 unique enclosing attempts: 334 rows had a current target
observation, 17 lacked the current target, and 18 lacked a current attempt. No
method identity was missing and none of the 369 rows fully typed. These rows
are observations, not 369 independent member attempts. The current two-run
reconciliation sidecars matched byte-for-byte. The deterministic
`object-module` run selected all 576 historical method identities across 77
files, attempted all 576, and typed 0. The reconciliation has 0 current object-module target observations, 549 rows whose frozen target
was not requested in the focused run, and 27 whose frozen attempt was not
repeated; no method identity was missing. Its current first blockers include
`TypeNameNotFound` (239), `MemberLookup` (73), `UnstableSelectionPrefix` (44),
`TermNameNotFound` (39), and `MemberNotFound` (34). This records a blocker
position, not module-resolution success. The older #1072 full-profile reprofile
contains six module completions and four module-member successes in this 576
row cohort, all rolled back; none was a committed or fully typed success. The
focused rerun therefore found no previously accepted module key to lose, but
its zero target observations do not reproduce those rolled-back operation
traces. The row-level sidecar and the preserved #1072 table make that scope
limit explicit. The paired normalized reports and reconciliation sidecars
matched byte-for-byte (report SHA-256
`90400841d5a99f52025070e6d7c561d0e5a2f05eaf0ba2e86a63505a1c9b36eb`; sidecar
SHA-256 `4240208d0bd5e45c4e1b9ee7a4ce91469c6632f0321a4905be5538886cf62033`).

Compact counts, outcome hashes, and control scopes are in
[`typer-real-method-acceptance-historical-controls-3.9.0.tsv`](typer-real-method-acceptance-historical-controls-3.9.0.tsv).
Raw multi-megabyte outputs were kept locally during the audit and are not
checked in; only compact TSVs and hashes are retained.

## Audit harness finding

The ordinary-method and focused report markers can share a line with Cargo's
`test ...` prefix. The runner previously searched for marker-only lines, so a
successful strict audit could be rejected as a missing report. The marker
extractor now accepts the actual harness output, rejects missing or empty
reports, and has four focused unit cases. This was a report-integrity defect:
it could hide successful audit output, and it is fixed without changing typer
semantics or the cohort numerator.

## Evidence-ranked next work

1. **Generic source-parent adaptation for `Either` methods.** Multiple cohort
   methods share the same `MemberLookup` / `ParentTypeAdaptation` blocker for a
   generic source receiver. Test receiver arity and conformance before widening
   support; this is the strongest shared Tier A entry point.
2. **`Tuple.toArray` application-argument conformance.** The method reaches
   `ApplicationArgumentConformanceUnsupported`; verify exact Scala array
   argument/result shapes and retain wrong-argument rejection.
3. **Nested `IndexedSeq.apply` in `IterableOnce` local loops.** #1706's outer
   function-value application is resolved, but the enclosing methods remain
   blocked at generic external member instantiation. Keep this separate from
   ordinary member-method acceptance and avoid broad classloader/typer work
   without a narrower measured slice.

The 1,236-file full corpus is **NOT RUN**. No source typing or classloader
behavior was changed by this closeout; the only code change repairs audit
report extraction.

## Reproduction

Use the pinned Scala 3.9.0 checkout, Maven artifacts, and Temurin JDK 21.0.8
listed above. The audit runner sets the Scala/JDK provenance into each generated
report; `CARGO_TARGET_DIR` and `TMPDIR` may point to local scratch directories.
Run each command serially:

```sh
export SCALA39_ROOT=/path/to/scala39-source
export JAVA_HOME=/path/to/temurin-21
export SCALA39_JDK_RELEASE=21
export COURSIER_MAVEN_ROOT=/path/to/scala39-maven
export PATH="$JAVA_HOME/bin:$PATH"
export CARGO_BUILD_JOBS=1
export RUST_TEST_THREADS=1

# The frozen 25-member strict Tier A gate (the runner itself repeats twice).
tools/typer-classpath-corpus-audit/run \
  --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --require-typed-cohort docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism target/tier-a-current.tsv

# Historical three-file smoke and Tier B package-term control.
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv \
  --verify-determinism target/three-file-smoke.md
tools/typer-classpath-corpus-audit/run \
  --focused-cohort package-term --verify-determinism target/package-term.md

# Object/module is included because #1706 changed classloader TASTy signatures.
tools/typer-classpath-corpus-audit/run \
  --focused-cohort object-module --verify-determinism target/object-module.md
```

The function-value roots use a one-line manifest and the same focused runner:

```sh
printf '%s\n' library/src/scala/collection/IterableOnce.scala > target/iterableonce.manifest
tools/typer-classpath-corpus-audit/run \
  --focused-manifest target/iterableonce.manifest \
  --verify-determinism target/iterableonce.md
```

The before/after root findings are also documented in
[`typer-function-value-application-3.9.0.md`](typer-function-value-application-3.9.0.md).
The pinned Scala compiler oracle command and E007 negative control are recorded
in [`typer-stdlib-minimal-method-cohort-3.9.0.md`](typer-stdlib-minimal-method-cohort-3.9.0.md).

## Validation

The marker-extractor tests and all 117 audit-tool Python tests pass. The
pinned Scala cohort oracle, positive control, and E007 negative control pass.
`cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`,
workspace Clippy with warnings denied, warning-free workspace docs, and
`cargo test --workspace --all-targets --locked` all pass. Focused
`dotty-typer` and `dotty-classloader` tests pass. The merged #1714 commit also
had green Rust, JDK 21, focused-audit, and Scala/TASTy CI gates. This PR's hosted
CI result is pending and will be reported after the branch is published.
