# DEFLATE Huffman lookup measurement for #1299

## Same-entry measurement

The benchmark inflates the raw DEFLATE payload for `dotty/tools/dotc/typer/Typer.class` from the pinned Scala 3.9.0 compiler JAR. The entry is 182,458 compressed bytes and 580,610 decoded bytes. Each run performs 50 inflations and checks the decoded CRC; three timings are recorded per implementation. It runs in release mode with Cargo jobs and Rust test threads set to one.

| Implementation | Timings | Median |
| --- | --- | ---: |
| HashMap lookup, before change at `e7a5665cc43e7eb5533923988af93c9de805888e` | 784.673, 782.201, 783.436 ms | 783.436 ms |
| Canonical range lookup, commit `c53e6e800723c20f940e73d059f5f5b37e50f8b4` | 201.021, 197.989, 198.263 ms | 198.263 ms |

The median time fell by 74.7% (3.95x faster) for this same-entry workload. Timings include DEFLATE output allocation and the CRC check, which is identical in both runs. The measurement command was:

```bash
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
DOTTY_DEFLATE_BENCH_JAR=/path/to/scala3-compiler_3-3.9.0.jar \
cargo test -p dotty-classloader --lib --release \
  benchmark_real_deflate_jar_entry -- --ignored --nocapture
```

The benchmark test verifies the expected decoded size and CRC on every iteration. It is ignored during normal test runs and can be repeated against another copy of the same pinned artifact.

## 12-file Typer cohort health check

The post-change run used the same 12 Scala paths recorded during the #1282 profile, Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, JDK 21.0.8, and serial Cargo/test settings. It processed 2,195 method observations in one run.

| Measurement | Result |
| --- | ---: |
| Audit wall time | 125.462 s |
| Cargo wall time, including compile/test startup | 201.902 s |
| Source typing | 71.389 s |
| Post-typing audit accounting | 53.425 s |
| Member resolver requests / inclusive time | 156,446 / 17.723 s |

This was one post-change health check. The earlier #1282 run took 467.8 s with a 99 Hz pprof sampler attached and reported 620 method observations. Those runs have different profiling overhead and observation counts, so their wall times are not an end-to-end before/after comparison. The classloader hotspot's before/after measurement above uses the same entry and benchmark command; no whole-cohort speedup is claimed.

No persistent cache or worker pool was added. The decoder keeps fixed-size per-length metadata for the RFC 1951 maximum of 15 bits and a symbol vector bounded by the input code-length table. Peak RSS was not measured in this run.

The captured profile report, timing TSV, and raw run log are retained in the ignored `target/typer-1299-12file-profile/` directory for local inspection.
