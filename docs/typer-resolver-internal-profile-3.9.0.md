# Typer delegated member resolver profile (Scala 3.9.0)

This bounded profile answers issue #1525 for the checked-in four-file manifest;
it is not a corpus-wide performance result and does not apply an optimization.
It selects up to two root methods and one supported-match probe per manifest
file. Six method roots and three match probes were actually selected. Every
match selector failed before its case body was typed.

The committed machine-readable evidence is in
[`typer-resolver-profile-3.9.0/`](typer-resolver-profile-3.9.0/): per-run timings,
per-root requests and stages, cache/completion counts, selected identities and
outcomes, match probes, classloader timings, and the instrumentation comparison.
The complete corrected runs remain under ignored
`target/issue-1525-corrected-{profile,semantic,cheap}/` paths.

## Reproduction inputs

- Repository base: `a3c0131f0faa976dd91db58bbb8b3d9fca28aaec`.
- Measured implementation: `47d6f4b26c1a8d57273bae6d0d005e035781e894`.
- Scala checkout: revision `777528f19a58e794c9954a42f433373472ec57f8` (Scala
  3.9.0).
- JDK: Temurin `21.0.8+9`, Linux x64; bundle SHA-256
  `f2dc5418092c43003db8f9005c4a286e1c0104fea96ccdd49e8ebd037cac9219`.
- Scala Maven artifacts (artifact: SHA-256):
  - `scala3-library_3-3.9.0.jar`: `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`
  - `scala-library-3.9.0.jar`: `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`
  - `scala3-compiler_3-3.9.0.jar`: `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`
  - `tasty-core_3-3.9.0.jar`: `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f`
  - `scala3-interfaces-3.9.0.jar`: `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db`
  - `scala-asm-9.9.0-scala-1.jar`: `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375`
  - `compiler-interface-1.12.0.jar`: `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3`
  - `util-interface-1.11.5.jar`: `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af`
- Rust/Cargo `1.98.1`; host AMD Ryzen 9 7940HS, Linux x86_64; one Cargo
  build job and one test thread. The audit binary was prebuilt outside each
  timed run. JDK/Scala inputs and Cargo target were warm. No full-corpus audit
  was run.

The measured command (set the four roots to the pinned local inputs) was:

```sh
SCALA39_ROOT=target/dotty-scala39-1525 \
JAVA_HOME=target/temurin21-1525 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=target/scala39-maven \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 3 --method-limit-per-file 2 --match-limit-per-file 1 \
  --compare-with target/issue-1525-baseline-profile \
  --compare-with target/issue-1525-semantic-final \
  --output-dir target/issue-1525-committed-full
```

The semantic-only, cheap-counter, and full-profile runs selected the same
method identities, method outcomes/blockers, match-probe identities, and match
outcomes. This is evidence for this bounded cohort only. Three warm Cargo wall
samples were 12,287, 12,286, and 12,285 ms in full mode; comparison means were
12,286 ms full, 12,083.7 ms semantic-only, and 12,084 ms cheap-counters. The
small differences are within run-to-run drift and show no measurable profiler
overhead or speedup. See
[`variant-comparison.tsv`](typer-resolver-profile-3.9.0/variant-comparison.tsv)
and [`run-timings.tsv`](typer-resolver-profile-3.9.0/run-timings.tsv).

## Delegated resolver measurements

The table reports mean time per run over three repetitions. “Exclusive” is the
phase elapsed time after subtracting the explicitly timed children shown by the
instrumentation; it is not a sampled CPU profile. Use the inclusive and
exclusive columns for attribution, not as additive totals.

| Phase | Source calls | Source inclusive / exclusive | Match calls | Match inclusive / exclusive |
| --- | ---: | ---: | ---: | ---: |
| Member lookup | 3,813 | 5,955 ms / 5,955 ms | 2,717 | 5,838 ms / 5,838 ms |
| Module-cache validation | 3,813 | 410 ms / 3.1 ms | 2,717 | 307 ms / 2.3 ms |
| Target resolution | 603 | 5,457 ms / 0.30 ms | 319 | 5,478 ms / 0.29 ms |
| Module completion | 43 | 5,457 ms / 1,609 ms | 41 | 5,478 ms / 1,655 ms |
| Module snapshot validation | 3,253 | 407 ms / 4.6 ms | 2,439 | 305 ms / 3.4 ms |
| Classpath resource lookup | 11,531 | 693 ms / 693 ms | 9,563 | 586 ms / 586 ms |
| TASTy decode | 47 | 3,905 ms / 3,905 ms | 46 | 3,884 ms / 3,884 ms |
| Class-file decode | 845 | 45 ms / 45 ms | 813 | 44 ms / 44 ms |
| Transaction bookkeeping | 4,625 | 5,955 ms / 0.38 ms | 3,298 | 5,837 ms / 0.27 ms |
| Rollback | 4,340 | 0.52 ms / 0.52 ms | 3,042 | 0.47 ms / 0.47 ms |
| Package-term key selection | 3,813 | 0.31 ms / 0.31 ms | 2,717 | 0.19 ms / 0.19 ms |

The large target-resolution interval is almost entirely nested module
completion. Its measured exclusive remainder is about 0.3 ms per run. Module
completion itself includes TASTy decode, resource access, and other completion
work; its 1.61–1.65 s residual is after subtracting the directly timed resource
and decode intervals. Source and match cache profiles recorded 3,211 and
2,398 positive module-cache validations per run, 602 and 343 negative member
cache hits (included in those validation counts), and 42 and 41 module-cache
misses. Module completion returned 42
successes and one error per source run; in the match pass it returned 40
successes and one error.

The snapshot timer now measures only time inside each `find_class` call;
snapshot comparisons, decoded-root checks, and cache invalidation remain in
snapshot validation's exclusive time. Failed module TASTy decoder attempts are
also included in the decode call count and elapsed time.

The three named hot method roots show where that delegated time lands:

| Root method | Member calls/run | Successes | Misses | Errors | Exclusive target/run | Exclusive module-completion/run | TASTy decode/run |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `GenericSignatures$.mayNeedSignature` | 570 | 55 | 515 | 0 | 0.077 ms | 637 ms | 1,214 ms |
| `tpd$.DefDef` | 470 | 13 | 457 | 0 | 0.090 ms | 738 ms | 1,256 ms |
| `Types$.Type.derivesFrom` | 1,687 | 18 | 1,668 | 1 | 0.114 ms | 225 ms | 1,425 ms |

The source-level path explanations below are hypotheses from the pinned Scala
3.9.0 implementations, not proof that any one expression accounts for the
measured duration:

- `GenericSignatures$.mayNeedSignature` (`GenericSignatures.scala:41`) walks
  `ExprType`, `MethodTpe`, and `TypeRef`, then asks the type predicates
  `isAny`, `isAnyRef`, `isRef`, and `isPrimitiveValueType`. That recursive
  inspection is consistent with its 51 repeated stable keys and the unresolved
  package/type requests in its root profile.
- `tpd$.DefDef` (`tpd.scala:237`) maps parameter symbols through `TypeDef` or
  `ValDef`, constructs a result `TypeTree`, and passes the result to
  `ta.assignType`. Those parameter/result type projections are plausible
  sources of its repeated member requests and module completion.
- `Types$.Type.derivesFrom` (`Types.scala:283`) recursively follows a
  `TypeRef` symbol and `superType`, then traverses proxies, applied, match,
  intersection, and union types. Its 1,687 delegated member calls and 1,406 ms
  decode interval fit repeated traversal into incomplete external identities.

All three stop at their first observed typer blocker: `TypeNameNotFound`,
`TypeNameNotFound`, and `MemberLookup`, respectively. The match selector
profiles are keyed by exact method symbol, case tree, and selector tree; all
three selectors failed (`TermNameNotFound`, `TypeNameNotFound`, and
`SymbolResolution`), so their case bodies had no delegated requests. The
committed outcome tables retain every selected root and probe, including the
non-hot roots: [`methods.tsv`](typer-resolver-profile-3.9.0/methods.tsv),
[`method-outcomes.tsv`](typer-resolver-profile-3.9.0/method-outcomes.tsv),
[`match-probes.tsv`](typer-resolver-profile-3.9.0/match-probes.tsv), and
[`match-probe-counts.tsv`](typer-resolver-profile-3.9.0/match-probe-counts.tsv).

Stable-key observations include both package and member requests, while
`resolver_calls` counts the lower-level classpath member resolver. They are
reported separately because package lookups add observations beyond member
calls. Per-root request-key frequencies, first outcomes, and retries after a
failed request are retained in
[`resolver-roots.tsv`](typer-resolver-profile-3.9.0/resolver-roots.tsv); phase
times for each root are in
[`resolver-root-phases.tsv`](typer-resolver-profile-3.9.0/resolver-root-phases.tsv).
No per-call log is required to inspect the repeated-key aggregates.

## Ranked follow-up candidates

1. **Investigate repeated module completion and TASTy projection.** This is the
   largest measured cost: up to about 3.9 s/run in TASTy decode intervals, plus
   completion residuals. Potential benefit is the largest of these candidates,
   but this profile does not show how much work can safely be removed.
   Complexity is high. Any reuse must validate exact resource identity and
   classpath precedence and must preserve rollback/session identity.
2. **Reduce module snapshot resource revalidation.** The selected passes perform
   about 5,700 snapshot validations per run and spend about 0.7 s in measured
   resource lookup across loader and snapshot paths. The safe benefit is below
   that measured lookup total. Complexity is medium; stale positive or negative
   evidence, multi-release selection, and changed classpath contents are the
   correctness risks.
3. **Revisit repeated unresolved member requests only after the first two.**
   The three hot methods have 392 distinct request keys combined and many
   repeated keys, but prefix classification, declaration selection, and the
   exclusive target path are all below 0.4 ms per run. Expected benefit is
   small on this cohort. Complexity is medium, with risks around namespace,
   selector, module identity, and transaction-sensitive negative results.

No optimization was applied. These candidates rank measurement evidence for
future work; they do not claim a speedup or license a semantic cache.

## Limits

- Parent phases are inclusive and nested. The exclusive columns subtract only
  intervals explicitly measured as children; do not add all columns together.
- The TASTy decoder invokes classpath-backed alias and bare-reference helpers.
  Their work remains inside the decode interval and is not separately counted
  as `find_class` calls. Resource counters are logical adapter lookups and
  returned bytes, not physical archive reads, decompressions, or filesystem
  I/O. That uninstrumented callback time is part of the accounted TASTy decode
  interval, not unaccounted wall time.
- Class completion had zero calls in this cohort. The emitted class-completion
  counters and timers are opt-in and are available for workloads that exercise
  that path.
- Method and match attempts stop at their first blocker. These measurements do
  not represent successful compilation of complete methods or match bodies.
- The profiler is inactive by default. No semantic behavior or optimization was
  changed by the profile collection.
