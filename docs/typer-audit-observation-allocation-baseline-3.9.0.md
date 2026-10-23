# Typer audit observation allocation baseline (Scala 3.9.0)

Issue: [#1484](https://github.com/scytrowski/dotty-rs/issues/1484). This baseline reuses the exact bounded four-file full-mode cohort from [#1483](typer-auditresolver-overhead-3.9.0.md): six root methods, three match probes, two serial repetitions, pinned Scala 3.9.0 and JDK 21.0.8. The source-side `AuditResolver` implementation is unchanged between the measured code commit `d7dc059ceecc38885db05a4ce3a3b93caff8b119` and the #1491 merge base `4025adf3092cfe4a0e51597acdfc91fcf5359746`; the intervening changes were profiler/docs fixes.

## Baseline profile

Full mode averaged **12,180.5 ms Cargo wall time** (12,176–12,185 ms), **10.765 s child user CPU**, **1.35 s system CPU**, and a maximum sampled process-tree RSS of **114,307,072 bytes (109.0 MiB)**. It emitted 3,766,458 bytes per sample. These are the exact C-mode samples recorded in `docs/typer-auditresolver-overhead-3.9.0/`.

The full collector retained the following event rows in each sample:

| Event vector cloned for report sorting | Rows | Owned string payload duplicated | Vector element buffer duplicated | Total requested clone bytes |
| --- | ---: | ---: | ---: | ---: |
| `module_package_term_observations` | 548 | 145,254 B | 83,296 B | 228,550 B |
| `module_operation_observations` | 6,664 | 1,115,680 B | 1,386,112 B | 2,501,792 B |
| `module_member_observations` | 3,253 | 923,113 B | 598,552 B | 1,521,665 B |
| `package_term_request_rows` | 388 | 123,925 B | 96,224 B | 220,149 B |
| **Total per sample** | **10,853** | **2,307,972 B** | **2,164,184 B** | **4,472,156 B** |

This is a component-level allocation calculation, not a claim about total process allocations. The element buffers use `size_of` for the four Rust observation structs multiplied by row count. String payload bytes are the UTF-8 lengths of their owned fields in the saved rendered rows; cloning each `String` duplicates those bytes. Allocator metadata and size-class rounding are excluded. The cloned `Vec`s are created before sorting and their owned rows remain live through report emission. `sort_unstable` over borrowed references needs one pointer per row (86,824 requested bytes total) and no owned string copies, so the code change targets approximately **4,385,332 fewer transient requested bytes per sample** after accounting for those reference arrays.

### Repetition and key counts

Each sample contained 3,253 module-member observations across 439 distinct `request_key` values: 2,814 rows repeat a key; median frequency 5, p95 frequency 20, maximum 95. The separate package-term request vector contained 388 rows across 8 distinct request keys; module-package-term observations contained 548 rows across 32 request keys. Repeated observations and their transaction states remain part of the authoritative report and must not be deduplicated.

The #1483 subphase profile also identifies the per-request work still present after this report-only baseline: across two C samples, source member-request preparation took 29,286 µs and observation collection 31,230 µs; the three match files added 29,898 µs and 14,022 µs respectively. Symbol identity rendering accounted for 9,858 µs in the source pass and 2,020 µs in the match pass across those two samples. These subtimers overlap resolver-call windows and are not additive to total wall time.

## Measured boundary

No accurate whole-process allocated-byte profiler was available for #1483. This baseline instead calculates the known clone allocations from exact emitted row counts and owned string payloads, and records whole-process wall/CPU/sampled RSS separately. The after profile will reuse the same manifest, limits, mode, machine, build profile, and serial runner. Equality will be checked for all four report sections; their stable rows must remain byte-for-byte identical. Timing metadata is the only expected output variation.
