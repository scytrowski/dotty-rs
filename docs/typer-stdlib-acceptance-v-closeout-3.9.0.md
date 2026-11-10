# Typer Stdlib Acceptance V closeout (Scala 3.9.0)

**Outcome: SPRINT GOAL UNMET pending fresh hosted validation and independent review.**

The release gate requires at least 19/25 complete ordinary main methods on one
tested revision, including the original unprepared `Tuple.toArray` DefDef, and
preservation of all 18 main and 16 analysis exact successful identities and
result shapes from the frozen #2233 baseline. Both unchanged 25-root cohorts
must be attempted in two deterministic sessions. The analysis denominator stays
25. `Either.swap` is optional stretch evidence and does not affect the minimum
pass.

Before the required Tuple completion, its original first blocker was
`ApplicationArgumentConformanceUnsupported` at source tree 22. The optional
`Either.swap` stretch was blocked at the first `Left(a)` extractor with
`ExtractorUnapplyNotFound`; the second `Right(b)` pattern was not reached. The
full source identities, spans, and baseline observations are in
[`roots.tsv`](typer-stdlib-acceptance-v/roots.tsv).

The existing focused workflow now applies this threshold to the first and
second complete reports, validates the exact frozen manifests and protected
success receipt, and writes a run-bound TSV and report artifact. A prepared-only
Tuple receipt is not accepted as whole-method cohort success. The checked-in
per-root TSV records the protected baseline and leaves the fresh result pending
until that workflow succeeds on the reviewed PR head.

Pinned Scala source: `777528f19a58e794c9954a42f433373472ec57f8`. Protected
identity/result receipt SHA-256:
`2a31b0b10cac7fe86d3c30f2d4e632ac93f2037e9a392efaf6e6b548e5284c4d`.
Required Tuple whole-method proof: [#2237 receipt](typer-stdlib-acceptance-v/tuple-to-array.md),
based on merged #2234/#2237. Integration base: `fb74fb4459dad48f33e778159ce0ca89dc1721aa`.
Tested PR head, hosted workflow URL, and final merge SHA remain pending; no
fresh hosted Acceptance V result is claimed here.
