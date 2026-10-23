# Typer audit postprocessing index profile (Scala 3.9.0)

This profile evaluates the post-typing audit changes for issue #1486. It is a bounded, serial probe over the same four manifest files used by the focused #1483 profile, with at most two root methods and one match probe per file. It is not a full-corpus or successful-typing benchmark.

## What changed

| Hot path | Before | After |
| --- | --- | --- |
| Source offset to line | Count newline bytes in each source prefix: O(methods × file bytes) | Build newline offsets once, binary-search each query: O(file bytes + queries × log(newlines)) |
| Enclosing failed attempt | Scan all failures for each local method: O(methods × failures) | Check failures indexed by the method's enclosing-tree chain, plus the small malformed-AST missing-span fallback: O(failures + methods × (nesting depth + missing-span failures)) |
| Resolver trace join | Repeatedly scan all method keys and clone matching observations at each local method | Parse/index trace keys once per file and clone each matching observation once: O(trace keys + observations + local methods × lookup) |
| Baseline function outcomes | Repeated source-prefix line scans and full failure scans | Reuse the source line and enclosing-failure indexes |

The index remains file-local. Resolver observation order and duplicates are retained. The root-failure selection still uses the smallest covering source range and stable original failure order for ties. If method ranges are equal/crossing or a failure's body range disagrees with its owner's recovered span, attribution falls back to the original full range scan for that file; this preserves malformed-AST behavior while keeping the common laminar, consistent case indexed. A failure whose owner has no indexed source span also activates that fallback.

## Measurements

Host: AMD Ryzen 9 7940HS; JDK 21.0.8; Cargo/test concurrency 1; Scala checkout pinned to `777528f19a58e794c9954a42f433373472ec57f8`. Both profile pairs used warm target and input caches. `before` is the pre-index profile at `87c40c65`; `after` is the indexed profile at `dd60ad63` with the complete postprocess timing and counters. Two repetitions are too few for a statistical performance claim.

| Measurement | Before, run 2 | After, run 2 | Interpretation |
| --- | ---: | ---: | --- |
| Cargo wall | 12,178 ms | 12,183 ms | No end-to-end speedup detected (0.04% slower, within noise) |
| Audit wall | 12,098 ms | 12,120 ms | No end-to-end speedup detected (0.18% slower, within noise) |
| Reported postprocess, three expensive files | 13 ms | 4 ms | 9 ms lower in this sample; millisecond granularity and changed timing coverage make this directional only |
| Maximum sampled process-tree RSS | 113.8 MiB | 115.0 MiB | No meaningful conclusion from this single sample |

The new `AUDIT_POSTPROCESS` profile line reports per-file bytes, newline count and line queries; method and failure counts; actual enclosing-failure lookups and candidates examined; resolver trace methods and observation count; and index construction/helper times in microseconds. In the after run, the two repeated samples showed:

| File | Bytes | Methods indexed | Line queries | Failures / lookups / candidates | Trace methods / observations | Index and baseline-outcome helper time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `GenericSignatures.scala` | 27,255 | 40 | 44 | 2 / 22 / 19 | 2 / 1,768 | 0.33 ms |
| `tpd.scala` | 73,174 | 257 | 34 | 2 / 17 / 4 | 2 / 980 | 0.32 ms |
| `Types.scala` | 307,493 | 1,100 | 150 | 2 / 76 / 3 | 1 / 1,845 | 1.14 ms |

The values above are representative of run 2; helper totals combine line index, ownership/failure index, resolver trace index, and source-function outcome timing. They are not the entire postprocess phase: other audit projections and report-row construction remain in that phase. The previous full scans are eliminated from the indexed hot paths, but their original invocation counts were not recorded by the old profiler, so they cannot be reconstructed as observed counts. The current counters expose the new path's actual query and candidate volume.

## Semantic parity

For both profile repetitions, these normalized outcome/cohort artifacts were byte-identical before and after: `method-outcomes.tsv`, `match-probes.tsv`, `match-probe-counts.tsv`, `resolver-counts.tsv`, and `request-roots.tsv`. Timing and detailed instrumentation artifacts naturally differ. The focused `local_definition_audit` integration tests also cover UTF-8 byte offsets, CRLF, nested and sibling methods, smallest enclosing failure, stable tie ordering, and missing-span fallback.

## Conclusion and limits

The implementation removes the repeated scans as designed and preserves the focused semantic sidecars. The probe does **not** demonstrate a material end-to-end runtime improvement: typing and match probing dominate this four-file run, and measured audit wall time is effectively unchanged. The observed postprocess reduction is only 9 ms at millisecond resolution across the selected expensive files and should not be presented as a statistically established user-visible gain. A wider, higher-repetition profile is needed to quantify the impact at corpus scale. No semantic typer or classloader behavior was changed.
