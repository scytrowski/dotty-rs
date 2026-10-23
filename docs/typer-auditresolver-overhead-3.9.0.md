# AuditResolver observer cost (Scala 3.9.0)

Issue: [#1483](https://github.com/scytrowski/dotty-rs/issues/1483). This bounded A/B/C experiment compares the same four-file manifest, six root-method observations, and three match probes in two serial repetitions per mode. It does not estimate a full-corpus run.

## Result

| Mode | Cargo wall mean | Repetition range | Child CPU user/system mean | Peak sampled process-tree RSS | Log bytes/sample |
| --- | ---: | ---: | ---: | ---: | ---: |
| A: semantic only | 11,981.5 ms | 11,981–11,982 ms | 10.53 / 1.385 s | 104,071,168 B (99.2 MiB) | 8,880 |
| B: cheap counters | 12,030 ms | 11,979–12,081 ms | 10.695 / 1.25 s | 104,083,456 B (99.2 MiB) | 13,864–13,865 |
| C: full audit | 12,180.5 ms | 12,176–12,185 ms | 10.765 / 1.35 s | 114,307,072 B (109.0 MiB) | 3,766,458 |

Relative to A, the observed mean wall deltas were **+48.5 ms (+0.40%)** for B and **+199 ms (+1.66%)** for C. C was +150.5 ms (+1.25%) over B. With two repetitions, these are small observed deltas, not precise or statistically stable overhead estimates: B's 102 ms range is larger than its 48.5 ms mean difference from A. CPU means are similarly close and do not distinguish the modes reliably.

Peak sampled RSS was effectively unchanged from A to B (+12,288 bytes); C peaked 10,235,904 bytes (+9.76 MiB) above A. This is a sampled process-tree peak, not an allocation counter or exact live-heap measure. RSS can double-count shared mappings and 100 ms sampling can miss short peaks. An accurate allocated-byte profiler was not available for this run, so the report uses sampled RSS and emitted log size as memory/collection indicators rather than claiming allocation totals.

## Attribution

In C, the source-pass `AuditResolver` wrappers spent about 90.5 ms exclusive across both repetitions (45.3 ms per measured run). The three selected match probes added about 49.5 ms exclusive across both repetitions (24.8 ms/run). Together that is about **70 ms per ~12.1 s run, or 0.58%**. This exclusive time covers wrapper work outside delegated resolver intervals; it is not additive to the inclusive wall time.

Delegated `member_lookup` consumed about 5.93 s in each source-method pass and 5.79 s in each match pass. Combined, this is approximately 11.72 s per sample, while full phase wall averages about 12.1 s. The measured cost is therefore dominated by real resolver work, not the `AuditResolver` wrapper. The classpath report records logical responses and returned bytes; it does not reveal physical JAR reads or decompression.

The corrected B-mode aggregation retains all three match files on each repetition: 2,717 `member_lookup` calls, 5,033 `module_identity` calls, and 185 rollbacks per run. These counts match the corresponding full-mode cohort. The earlier generated B table had retained only the last match file; [`cheap-counters-summary.md`](typer-auditresolver-overhead-3.9.0/cheap-counters-summary.md) and [`cheap-resolver.tsv`](typer-auditresolver-overhead-3.9.0/cheap-resolver.tsv) were regenerated from the preserved logs after fixing the parser.

The source pass recorded 386 rollbacks per repetition. The exclusive rollback wrapper plus rollback annotation was under 0.2 ms per sample; measured postprocessing was about 16 ms per sample. Neither currently supports a rollback or postprocessing optimization on performance grounds. Most observer collection time was in member request preparation and observation collection; the subphase table reports these separately. Subphase windows can overlap resolver call windows and must not be added to their totals.

## Semantic equivalence and interpretation

The runner compares selected method identities and emitted first-error labels, plus match-probe identities and outcomes, before writing an A/B/C comparison. The comparison succeeded with six method observations and three match probes in every mode. This establishes equality of the audit's observable labels for this bounded cohort; it is not a full typed-tree equivalence proof. The authoritative detailed C mode remains available and keeps its identity, transaction, failure, rollback, and sidecar records.

Recommendation: keep full mode for the authoritative deterministic report. Use cheap counters for ad-hoc profiles that need operation counts without detailed request identities. Do not prioritize rollback or postprocessing optimization from these numbers. If memory/log volume becomes a concern, narrow experiments to optional member-request identity and observation collection: full mode emitted about 3.77 MB per sample and added roughly 9.76 MiB of sampled peak RSS, although its observed mean wall increase was only 1.66%. Follow-up performance work should start with delegated member resolution, which accounts for nearly all measured sample time.

## Reproduction

Run the same pinned manifest serially once per mode; each invocation itself contains two repetitions. Compare each later mode against the previous output directory:

```sh
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --mode semantic_only --runs 2 \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --method-limit-per-file 2 --match-limit-per-file 1 \
  --output-dir target/issue-1483-semantic-only

python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --mode cheap_counters --runs 2 \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --method-limit-per-file 2 --match-limit-per-file 1 \
  --compare-with target/issue-1483-semantic-only \
  --output-dir target/issue-1483-cheap-counters

python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --mode full --runs 2 \
  --manifest docs/typer-hot-path-profile-manifest-3.9.0.tsv \
  --method-limit-per-file 2 --match-limit-per-file 1 \
  --compare-with target/issue-1483-semantic-only \
  --compare-with target/issue-1483-cheap-counters \
  --output-dir target/issue-1483-full
```

The checked-in comparison and selected diagnostic tables are in [`typer-auditresolver-overhead-3.9.0/`](typer-auditresolver-overhead-3.9.0/). Captured raw full-mode logs (about 7.5 MB total) remain under ignored `target/` and are not committed.

Environment: dotty-rs measured commit `d7dc059ceecc38885db05a4ce3a3b93caff8b119`; Scala checkout revision `777528f19a58e794c9954a42f433373472ec57f8`; JDK 21.0.8; cargo/test concurrency 1; warm compiled test binary prebuilt outside the measured intervals; 100 ms RSS sampling. The run used a 4-file/6-method/3-match bounded cohort, not a full corpus. Timing variation, sampled RSS, diagnostic output volume, and lack of allocation-byte instrumentation limit precision.
