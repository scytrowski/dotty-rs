# Typer resolver and semantic sprint closeout evidence (Scala 3.9.0)

Issue: [#1530](https://github.com/scytrowski/dotty-rs/issues/1530)

## Integration status

The profiling and semantic increments are integrated on `main`:

| Issue | PR | Status |
| --- | --- | --- |
| #1525 delegated resolver profile | [#1542](https://github.com/scytrowski/dotty-rs/pull/1542) | merged |
| #1526 measured classpath optimization | [#1546](https://github.com/scytrowski/dotty-rs/pull/1546) | merged |
| #1529 default type imports | [#1549](https://github.com/scytrowski/dotty-rs/pull/1549) | merged |
| #1528 List completion fanout safety | no PR found | **open; blocks full sprint closure** |

This report closes the available integration evidence for #1530. It does not
claim the whole sprint is complete while #1528 remains open and the last known
completed-class fanout is above its limit.

## Revisions and pinned inputs

The source comparison anchors are kept separate because they answer different
questions:

| Purpose | dotty-rs revision |
| --- | --- |
| #1525 pre-profile source baseline | `a3c0131f0faa976dd91db58bbb8b3d9fca28aaec` |
| #1525 measured profile implementation | `47d6f4b26c1a8d57273bae6d0d005e035781e894` |
| #1526 instrumented, cache-disabled comparison base | `8c72ec83222aeb75ef597b2d396ef4db20bab123` |
| Before the #1529 semantic increment | `dac7f43b0ef76f25a66ea4fd585214991421af06` |
| Final source revision measured here | `8c045cc5c6518176d6ae544d93ae1e7824217d68` |
| Latest prior full-audit evidence (not rerun here) | `2f6795113387f40f43971b57b1451ee13ad3ec02` |

The pinned Scala checkout is `777528f19a58e794c9954a42f433373472ec57f8`
(Scala 3.9.0). The hot-path manifest SHA-256 is
`ccbe566104090fa31cf25ca37bfc1e6563ec755be4efb878a8595bd464a428e0`; the
three-file smoke manifest SHA-256 is
`fd0e0d5c7b327f5703716abaaad36387244c204f48a43b30d282d3df6c150594`.
All eight compiler/library JAR hashes and the measured JDK bundle hash were
verified against the pinned #1525 profile inputs:

| Input | SHA-256 |
| --- | --- |
| Temurin JDK 21.0.8+9 Linux x64 bundle | `f2dc5418092c43003db8f9005c4a286e1c0104fea96ccdd49e8ebd037cac9219` |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

The final profile ran on an AMD Ryzen 9 7940HS under Linux x86_64 with Rust
1.98.1, Cargo test profile, one Cargo build job, one Rust test thread, warm
Cargo artifacts, and warm selected source/JAR/JDK inputs. The full commands
and per-sample measurements are in
[`typer-resolver-semantic-sprint-timing-3.9.0.tsv`](typer-resolver-semantic-sprint-timing-3.9.0.tsv).

## Bounded performance profile

The existing #1525 four-file manifest selected six source roots and three
match probes. The audit binary was built before measurement. Three warm,
serial samples on the final source revision took 10,673, 11,076, and 10,771 ms
Cargo wall time (mean 10,840 ms, population standard deviation 172 ms, range
3.7% of the mean). Child CPU averaged 9.58 s user and 1.14 s system. The
100 ms process-tree sampler observed a maximum 122,822,656 bytes (117.1 MiB);
it may miss shorter peaks and can count shared mappings more than once.

The #1526 cached profile averaged 10,976 ms on the same selection and host.
Its 10,874–11,076 ms run range overlaps the final 10,673–11,076 ms range. The
final samples show no material slowdown, but the additional 1.2% mean decrease
is not evidence of a further speedup. Compared with the #1526 cache-disabled
12,621 ms baseline, the final mean remains 14.1% lower; that difference
includes the measured #1526 optimization and must not be attributed to this
closeout or to #1529.

The exact six method outcomes and three selector outcomes had the same
SHA-256 `6ffdafdf5620fb2015f9a0983802d795676dba514e2aa54bec93e6e5c98eb286`
in all three repetitions. One of the six method blockers differs from the
post-#1526 snapshot for the intended #1529 reason: `tpd$.ClassDef` at
`compiler/src/dotty/tools/dotc/ast/tpd.scala#tree=1971` advanced from
`TypeNameNotFound` to `SourceAnnotationClassDeferred` after the default
`java.lang` type became resolvable. The three hot method roots did not move:

| Root (`path#tree`) | Member lookups / sample | Mean method time | Mean delegated member time | First blocker |
| --- | ---: | ---: | ---: | --- |
| `tpd.scala#tree=1790` `tpd$.DefDef` | 472 | 1,904 ms | 1,896 ms | `TypeNameNotFound` |
| `GenericSignatures.scala#tree=204` `GenericSignatures$.mayNeedSignature` | 572 | 1,682 ms | 1,671 ms | `TypeNameNotFound` |
| `Types.scala#tree=1031` `Types$.Type.derivesFrom` | 1,687 | 1,623 ms | 1,602 ms | `MemberLookup` |

Per-root method and selector identities, first blockers, delegated times,
retry counts, and the most repeated stable request shapes are in
[`typer-resolver-semantic-sprint-profile-roots-3.9.0.tsv`](typer-resolver-semantic-sprint-profile-roots-3.9.0.tsv).
Its `first_sample_member_inner_us` values are from the first profile run; the
three-run means are summarized in the table above and retained in the timing
TSV.

The separate match pass made 2,719 member and 566 package requests per sample;
it observed 86 successful, 2,632 unresolved, and one failed member lookup,
with 549 unique request keys and 2,529 retries after failed lookups. The source
pass made 3,806 member and 787 package requests per sample; its stable-key
profile recorded 162 successful, 3,643 unresolved, and one failed member
lookup, with 755 unique keys and 3,629 retries after failed lookups. These are
the audit wrapper's root observations; they are distinct from classpath
resource responses.

Across each sample the classpath adapter reported 23,107 logical requests,
15,464 positive responses, 87 negative responses, no errors, and
542,514,089 returned logical bytes. Archive instrumentation separately
recorded 28,852 JAR lookups, 2,832 ZIP entry reads, 27,410 cache hits, 1,442
misses, zero evictions, and 1,442 retained entries / 4,151,143 bytes per
sample. OS-level filesystem reads are not measured because the JARs are already
loaded into the classpath adapter. Resolver cache counters recorded 2,398
positive module hits, 343 negative member hits, 41 module-cache misses, and
41 completion attempts (40 successes, one error) in the match pass. TASTy
decode averaged about 3.86 s per sample, while measured resource lookup
averaged about 31 ms. The decode interval includes callback/projection work
that is not separately measured; inclusive resolver and nested classloader
times must not be added together. Audit rollback bookkeeping recorded 386
rollback events and 974 checkpoints per sample, with 6,638 operation and
3,240 member observations scanned.

The selected profile is intentionally a diagnostic sample, not successful
compilation: the six root methods and three match selectors still stop at
their first blockers. ZIP entry reads and cache behavior are measured, but OS
filesystem reads, decompression counts, and process-level thermal data are
not. Inclusive resolver and nested classloader timings overlap, so they are
reported separately rather than summed.

## Focused semantic and frozen-cohort results

The 60-file `package-term` cohort replays 369 frozen method identities. On the
baseline before #1529, 63 of its first-blocker rows were `TypeNameNotFound`; on
the final revision, 30 remain. Thirteen of the 27 exact source roots in the
#1529 diagnosis advanced past that blocker, 14 stayed at it, and none became
fully typed. The selected denominator stayed at 369 methods, 297 target
observations, and zero missing methods. All 27 method identities, spans,
previous outcomes, and final outcomes are recorded in
[`typer-resolver-semantic-sprint-blocker-movement-3.9.0.tsv`](typer-resolver-semantic-sprint-blocker-movement-3.9.0.tsv).
The two normalized package-term reports and the two reconciliation TSVs were
byte-identical; their SHA-256 values are `614467155a91a4a74ae32c01ad9ec168e168cfb7690a582cfa689d88c2f7d355`
and `968dc040036cac3b9a90be1a19656e916eb50829c046efa15e71bb420757fee5`.

The checked-in three-file smoke gate still covers 31 method identities and
types zero completely. Its normalized result changed in exactly three
`StringParsers.scala` methods because #1529 now resolves `scala.Option` and
`java.lang.String`: their blockers moved from `TypeNameNotFound` to
`TermNameNotFound` or `MemberNotFound`. The report gained two package requests
and seven member requests, and class-symbol successes increased from 17 to
22; no earlier successful target was removed. The focused baseline was
updated to this deliberate semantic result and rechecked with
`verify_focused_gate.py`. Its SHA-256 is
`17abcbafac966d37132ef54e0855fc937959497c1bdb17c79046116934083245`.

The frozen `object-module` cohort also ran twice on the final revision: 77
files, 576 method identities, zero typed methods, and zero missing methods.
Its normalized report SHA-256 is
`8d5b9de0499e727d81db3c9178724e1a759f3c57a0e3a14f858955db42c6d25e`; its
cohort sidecar SHA-256 is
`74a6bc793c6fc271cb69878da61b0a4c95ba64c9d337f721204be0a30ffa50cb`.
The current full blocker distribution for `package-term` is retained in the
focused report's deterministic sidecar; its largest remaining categories are
144 `ImportQualifierNotFound`, 97 `SymbolResolution`, and 30
`TypeNameNotFound` rows.

The pinned Scala 3.9 import oracle passed on the final source revision. It
covered default `List`, `Option`, `String`, and `Character`, qualified and
shadowed types, collection imports, plus negative controls for unknown types,
missing members, and class-as-term imports.

### Reproduction commands

The local source checkout, JDK, and Coursier cache paths below refer to the
pinned inputs in the preceding table. The profile script prebuilds the audit
binary before its three measured samples:

```sh
JAVA_HOME=target/temurin21-1525 \
SCALA39_ROOT=target/dotty-scala39-1525 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=target/scala39-maven \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 3 --method-limit-per-file 2 --match-limit-per-file 1 \
  --output-dir target/issue-1530-main-profile-final
```

The same pinned environment ran the three-file smoke and two frozen cohorts
with `--verify-determinism`:

```sh
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv \
  --verify-determinism target/issue-1530-smoke.md
tools/typer-classpath-corpus-audit/run \
  --focused-cohort package-term --verify-determinism \
  target/issue-1530-package-term.md
tools/typer-classpath-corpus-audit/run \
  --focused-cohort object-module --verify-determinism \
  target/issue-1530-object-module.md
python3 tools/typer-classpath-corpus-audit/verify_focused_gate.py \
  --report target/issue-1530-smoke.md \
  --baseline docs/typer-focused-smoke-baseline-3.9.0.txt
```

For `scala39_import_oracle.py`, `SCALA39_CLASSPATH` is the colon-separated
list of the eight pinned JARs in the input table. The temporary raw focused
reports and profile logs stay under ignored `target/` paths; only compact
hashes, timings, and blocker/gate tables are committed.

## Classloader safety gate and full-corpus policy

The latest prior full-corpus comparison is the local report at
`2f6795113387f40f43971b57b1451ee13ad3ec02`, compared with its frozen baseline
`4025adf3092cfe4a0e51597acdfc91fcf5359746`. It predates #1525–#1529; its
target-retention and fanout results do not establish final-revision status. At
that earlier comparison it retained 157/157 successful target keys and
reported dependency p95 depth 0, maximum depth 8, and maximum completed-class
fanout 29 against the limit of 16. Four
`scala.collection.immutable.List` requests from `ListMap.addOne`,
`ListBuffer.removeAfter`, and `TrieMap.LNode` insertion/removal each visited
63 class identities. Visited identities, completed classes, and recursion
depth are separate measurements.

Those full-corpus values are the last recorded state before this sprint and
are **not refreshed at the final revision**. The
final focused package-term, object-module, smoke, and selected profile runs
provide bounded validation for the merged imports and resolver changes. A
full 1,236-file run was not warranted for this closeout, remains local-only,
and is recorded as **NOT RUN**, not PASS. The prior fanout violation remains
visible and #1528 stays open; the compact gate table in
[`typer-resolver-semantic-sprint-classloader-gates-3.9.0.tsv`](typer-resolver-semantic-sprint-classloader-gates-3.9.0.tsv)
marks it blocked instead of treating old evidence as a final-revision pass.

## Recommendation and validation

The next highest-impact bounded work is to complete #1528: reproduce the four
exact List roots with a focused local classloader profile, explain which edges
require completion, and reduce fanout only where the classpath evidence proves
that work is unnecessary. This addresses the known 29/16 safety violation
before any broader cache or semantic rewrite. After that gate is understood,
the remaining 30 `TypeNameNotFound` observations are a better-supported
semantic cohort for a later focused issue than another broad resolver refactor.

Validation on the final source revision included three full-mode hot-path
samples, deterministic package-term/object-module/smoke cohorts, the pinned
Scala import oracle, and the focused baseline gate. Focused classloader and
typer test suites, audit-tool tests, formatting, frontend-boundary checks,
Clippy, docs, and workspace tests are run for this PR; their results are
recorded in its description and CI. No hosted or full-corpus audit was
dispatched.
