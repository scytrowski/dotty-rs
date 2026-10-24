# Typer delegated member resolver profile (Scala 3.9.0)

This is a bounded measurement for issue #1525, not a corpus-wide performance
claim or an optimization result. It profiles the four-file manifest in
[`typer-hot-path-profile-manifest-3.9.0.tsv`](typer-hot-path-profile-manifest-3.9.0.tsv)
with two root methods per file and one match probe per file. The selected
attempts stop at the first typer blocker. The complete raw logs remain in the
ignored `target/issue-1525-*` directories; the tables below retain the stable
aggregate evidence needed to review this measurement.

## Reproduction inputs

- Repository base: `a3c0131f0faa976dd91db58bbb8b3d9fca28aaec`.
- Profile instrumentation: `56b7f8915b` (measured working tree; report changes are
  committed separately).
- Scala source checkout: revision
  `777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0).
- JDK: Temurin `21.0.8+9`, Linux x64; downloaded bundle SHA-256
  `f2dc5418092c43003db8f9005c4a286e1c0104fea96ccdd49e8ebd037cac9219`.
- Scala Maven inputs (artifact: SHA-256):
  - `scala3-library_3-3.9.0.jar`: `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba`
  - `scala-library-3.9.0.jar`: `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd`
  - `scala3-compiler_3-3.9.0.jar`: `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a`
  - `tasty-core_3-3.9.0.jar`: `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f`
  - `scala3-interfaces-3.9.0.jar`: `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db`
  - `scala-asm-9.9.0-scala-1.jar`: `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375`
  - `compiler-interface-1.12.0.jar`: `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3`
  - `util-interface-1.11.5.jar`: `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af`
- Rust `1.98.1`, Cargo `1.98.1`, test profile; AMD Ryzen 9 7940HS,
  Linux x86_64; Cargo and test concurrency set to 1.
- The exact `local_definition_audit` test binary was prebuilt outside each
  sample. All three samples used a warm Cargo target and warmed selected input
  files/JARs/JDK modules. No full-corpus audit was run.

Run the full profile from the repository root with the pinned source checkout,
JDK, and Maven files supplied through the corresponding environment variables:

```sh
SCALA39_ROOT=target/dotty-scala39-1525 \
JAVA_HOME=target/temurin21-1525 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=target/scala39-maven \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 3 --method-limit-per-file 2 --match-limit-per-file 1 \
  --output-dir target/issue-1525-instrumented-final
```

The audit wall times were 12,113, 12,170, and 12,229 ms (median 12,170 ms).
The selected root method typing totaled 18,031 ms across the three repetitions;
match-probe typing is reported separately. The original baseline and instrumented
profile had identical method identities, method outcomes, match-probe identities,
and match outcomes. The instrumented full run and a separate three-run
`semantic_only` run also had identical selections and outcomes. These checks
establish that opt-in timing did not change this cohort's observed results;
they do not establish semantic equivalence outside it.

## Resolver results

Counts and stage totals below are for source root-method requests, aggregated
over the three serial repetitions. Per-run counts and timings were stable; the
raw sidecars are retained in the ignored output directory.

| Delegated resolver phase | Calls | Mean elapsed per run |
| --- | ---: | ---: |
| Member lookup, inclusive | 11,439 | 5,956 ms |
| Prefix classification | 11,439 | 0.094 ms |
| Module cache validation | 11,439 | 415 ms |
| Target resolution | 1,809 | 5,452 ms |
| Declaration selection | 1,806 | 0.027 ms |
| Module identity resolution | 1,359 | 85.6 ms |
| Class loading, inclusive | 36 | 0.030 ms |

Each run observed 3,813 source member requests: 161 successes, 3,651 misses,
and one error. The 3,652 misses/errors rolled back their member transaction.
The separate match-probe pass observed 2,717 requests per run: 86 successes,
2,630 misses, and one error.

| Source request shape (per run) | Requests | Distinct keys | Requests per key |
| --- | ---: | ---: | ---: |
| Module-class prefix, term | 1,908 | 224 | 8.518 |
| Module-class prefix, type | 1,345 | 215 | 6.256 |
| Package prefix, term | 548 | 32 | 17.125 |
| Package prefix, type | 12 | 4 | 3.000 |

The main follow-up candidate is the target-resolution path: it accounts for
about 5.46 seconds per run, while prefix classification and declaration
selection are below 0.1 ms combined. Repeated requests and the high miss rate
make cache behavior worth investigating by request shape. This profile does
not identify a safe cache key or justify a semantic cache change; the next
measurement should split target resolution by target kind and success/miss
before an optimization is proposed.

## Interpretation limits

- Phase timings are inclusive and nested. In particular, target resolution
  occurs inside member lookup. Do not sum phase totals as disjoint CPU time.
- The 12 class-loading phase calls per run spent about 30 microseconds in
  total. The selected dependency observations recorded about 1.3 microseconds
  of resource lookup and no ClassFile or TASTy decode time. This is a property
  of this bounded cohort, not evidence that decoding is free in other workloads.
- Classpath resource observations count logical results and returned bytes,
  not physical archive reads or decompressions. The adapter does not expose
  those events.
- Resolver outcomes are mostly unresolved attempts, and every selected method
  attempt stops at its first typer blocker. These timings describe current
  bounded audit work, not successful compilation of complete methods.
- No implementation optimization was applied and no speedup is claimed.

The full generated tables, including per-run details and audit-wrapper
overhead, can be regenerated into `target/issue-1525-instrumented-final` using
the command above. The parser and report unit tests cover the new profile
records and sidecars.
