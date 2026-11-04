# Mixed-origin lookup probe profile

Issue #2100 considered combining the positive Scala lookups in
`lookup_probes()` in `tests/jvm_tasty_mixed_origin_constructor_scala_consumer.rs`.
The function currently has six members in its probe inventory. Each member is
compiled once with the original TASTy sidecars and once as an independent
classfile-only control, for six positive and six negative `scalac` invocations.
It makes no `javap` or runtime calls. Per-member positive success and the
classfile-only first diagnostic are asserted independently.

## Decision

No batching change is made. A reliable benefit was not established, so merging
positive sources would add compatibility risk without measured evidence. All
six positive lookups and all six independent negative controls remain intact.

## Measurement limits

The source-level inventory was inspected at base commit
`8a8032782375d2433d26708b68c35e66cf7bba3a` on 2026-11-04. Comparable local
timings could not be captured: `SCALA39_ROOT` and
`COURSIER_MAVEN_ROOT` are not provisioned in this environment, and the cited
GitHub Actions run log was unavailable to the local `gh` client. `/tmp` was
full during this work; profiling used no cleanup of existing temporary data.
Consequently, no three-run timing or post-change comparison is claimed. This
is a measurement-limited no-go, not evidence that batching is slower.
