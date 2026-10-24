# Typer resolver optimization evidence (Scala 3.9.0)

This report tracks issue #1526. Its measurements use the same four-file,
bounded cohort as [the #1525 profile](typer-resolver-internal-profile-3.9.0.md),
not the full Scala corpus. The selected method and match identities, outcomes,
and blockers must remain fixed for before/after comparisons.

## Selected candidate and pre-change reasoning

The only selected candidate is repeated extraction of the same classpath
resource from one `JarClassPath` instance. The #1525 profile measured 11,531
source and 9,563 match classpath resource lookups per run, costing about 693 ms
and 586 ms respectively. It recorded 47 and 46 TASTy decodes costing about
3.9 seconds each; this change does not cache decoding or alter resolver/module
completion behavior.

Call path: typer member resolution requests a binary name from
`SharedClassPath`; the `CompositeClassPath` probes entries in established
order; a `JarClassPath` checks the versioned and unversioned `.tasty` entries,
then the corresponding `.class` entries, through `read_versioned_entry` and
`ZipArchive::read_entry`. Construction has already read the entire JAR into
memory. The repeated resource operation therefore does not perform additional
filesystem reads, but it repeats ZIP directory lookup, decompression, and
allocation for successful lookups (and repeats missing-entry probes).

For an ordinary JAR, an unsuccessful TASTy probe and a class probe perform two
entry reads per class lookup. A successful TASTy lookup stops after its TASTy
entry; a successful class lookup performs both probes. A multi-release JAR may
probe version directories from the configured release down to 9 before the
base entry. Before the optimization, the lookup cost is proportional to the
number of candidate ZIP entries checked plus the uncompressed resource size.
The expected cached cost is an average constant-time map lookup and a clone of
the retained resource handle; cache misses keep the existing lookup cost.

The cache is scoped to one immutable `JarClassPath` archive snapshot. Its key is
the `BinaryName`; release version, multi-release state, archive bytes, and
origin are fixed by the owning instance. It does not span classpath entries,
classpath instances, stores, or sessions. The composite continues to probe
earlier entries first. Errors and unsafe names are not cached. Resource
precedence, exact origin, and fresh classpath snapshots remain unchanged.

## Baseline instrumentation

The pre-cache measurement adds counters for JAR `find_class` calls and actual
ZIP entry reads. Cache counters remain zero in the baseline. This separates
JAR-internal archive work from classloader-level resource requests. The saved
`archive-resources.tsv` sidecar records each run's counts.

Baseline command, using the pinned Scala 3.9.0 checkout and Temurin 21 inputs:

```sh
SCALA39_ROOT=target/dotty-scala39-1525 \
JAVA_HOME=target/temurin21-1525 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=target/scala39-maven \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --runs 3 --method-limit-per-file 2 --match-limit-per-file 1 \
  --compare-with target/issue-1525-corrected-profile \
  --output-dir target/issue-1526-baseline
```

The baseline and optimized profiles use sequential warm runs, identical
selected probes, and the same Cargo target. Comparison must include total wall
and child CPU time, peak RSS, member lookup inclusive/exclusive time, classpath
requests, ZIP reads, cache hits/misses/evictions, and exact selected identities,
outcomes, and blockers. Measurements and a per-variant comparison are recorded
below. The baseline profile used repository base
`8c72ec83222aeb75ef597b2d396ef4db20bab123` with the profiling counters present
and the cache disabled. The cached profile changed only the per-JAR resource
cache behavior. The branch later rebased onto `91904f2f7f`; the commits
between these bases touched only `dotty-jvm`, so the profiled typer and
classloader paths remained identical. Each profile selected six methods and
three match probes; all method identities/outcomes and all match-probe
identities/outcomes are equal.

## Measurements

The selected candidate is the only optimization in this issue, so its
individual and combined result are the same run. Values are means of three
sequential warm runs. The cache lowered wall time in all three samples; the
sample ranges do not overlap.

| Measurement | Before | Per-JAR cache | Change |
| --- | ---: | ---: | ---: |
| Cargo wall time | 12,621 ms | 10,976 ms | -13.0% |
| Child CPU user + system | 12.53 s | 10.88 s | -13.2% |
| Maximum sampled process-tree RSS | 119.5 MB | 123.1 MB | +3.0% |
| Member lookup, source delegated inclusive | 6,078 ms/run | 5,286 ms/run | -13.0% |
| Member lookup, source wrapper exclusive | 29.3 ms/run | 25.4 ms/run | -13.4% |
| Member lookup, match delegated inclusive | 6,018 ms/run | 5,188 ms/run | -13.8% |
| Member lookup, match wrapper exclusive | 28.9 ms/run | 15.1 ms/run | -47.7% |
| Classpath-entry lookup wall time | 1,346 ms/run | 171 ms/run | -87.3% |
| ZIP entry reads | 49,522/run | 2,727/run | -94.5% |

Each run made the same 23,110 classpath requests, with 15,561 inner calls,
15,480 positive results, 81 negative results, and 543,524,646 returned logical
resource bytes. The cache recorded 27,447 hits and 1,388 misses per run, no
evictions, and 4,142,233 retained resource bytes across the profiled JAR
instances. TASTy/classfile decode counts and all selected typer outcomes stayed
fixed. Peak RSS increased by about 3.4 MB, consistent with the retained
resource payloads.

The detailed per-run records and exact selected roots/probes are checked in at
[`typer-resolver-optimization-profile-3.9.0/`](typer-resolver-optimization-profile-3.9.0/).
The source and match wrapper reports separate wall, delegated, and exclusive
member lookup time; classpath and archive sidecars report logical requests and
actual ZIP entry reads separately. Reproduce each run with the command above,
using a distinct output directory and the cached profile's
`--compare-with target/issue-1526-baseline` argument for the after run.

## Cache bounds and validation

The cache is bounded per JAR by entry count, total retained resource
bytes, and maximum individual resource size. Positive and successful negative
lookups are eligible; failed ZIP reads are retried on the next lookup. Eviction
only drops retained cache entries and does not alter archive contents.

Focused regressions cover repeated positive and negative lookups, cache bounds,
errors and retries, `.tasty` precedence, multi-release selection, and
classloader resolver rollback/revalidation. The classloader and typer tests
must pass before the performance result is accepted. If the measured cohort
shows no repeat hits or no improvement beyond run variance, remove the cache
and record the limiting evidence instead of claiming an optimization.
