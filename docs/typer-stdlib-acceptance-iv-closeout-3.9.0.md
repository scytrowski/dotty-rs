# Typer Stdlib Acceptance IV closeout (Scala 3.9.0)

## Outcome: SPRINT GOAL UNMET pending fresh hosted validation and independent review

This checked-in closeout separates two measured states and is a historical
snapshot; it has no fresh hosted #2184 result. The immutable #2179 receipt
records the actual first and second ordinary-audit outcomes at
`89f81940fc538a7b24323b82b9415cdb6a7c5404`: 16/25 main and 15/25 analysis
typed, with deterministic identical attempts. The separate #2183 integration
receipt records `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`: 18/25 main and
16/25 analysis typed. The original `Option.map` target changed from blocked in
both measured #2179 attempts to typed in the integration receipt; unprepared
`Tuple.toArray` remained blocked. The prepared Tuple proof is conditional on
explicit owner completion and does not count as ordinary-audit acceptance.

The historical `baseline_status` labels in the previously used sealed-ADT
cohort receipt are not measured #2179 outcomes and are never used to decide
target newness or before state.
[`measured-2179.tsv`](typer-stdlib-acceptance-iv/measured-2179.tsv)
freezes the per-root first/second reports and records the successful #2179
workflow run, PR source revision, baseline revision, artifact ID, and artifact
digest. The separately pinned [#2183 integration
receipt](../typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv) records its first
and second attempts; its `baseline_status` field is ignored for this comparison.

The PASS condition is at least 17/25 typed main roots and at least one exact
original Option.map or Tuple.toArray root typed by the ordinary audit, with
its fresh-session source/typed mapping proof, and it must have been untyped in
both measured #2179 attempts. The separate #2183 integration state is reported
without being confused for the before-state. Protected identity and result
sets, all 25 attempted roots per cohort, and deterministic repeat reports must
remain intact. Analysis retains its separate 25-root denominator and 15
protected successes. Arena-local typed-tree IDs were not retained by the #2183
integration receipt and are shown as unavailable in the static comparison.
The exact per-root comparison is in
[`closeout.tsv`](typer-stdlib-acceptance-iv/closeout.tsv); its provenance is
explicitly the measured #2179 receipt, the separate checked-in integration
receipt, and the checked-in integration result snapshot, not a rerun at this
PR head.

The existing `typer-focused-audit.yml` closeout gate replays the ordinary
cohorts and root-specific fresh-session proofs, checks exact root/result
identity, the 17/25 threshold and target row, and publishes a run-bound report
and TSV artifact. It verifies the immutable #2179 receipt digest and separate
#2183 integration receipt digest before comparing the fresh audit. That hosted
run and independent review must pass on the same PR head before the sprint
outcome can be changed to PASS. The hosted artifact contains the live action
URL and tested revision. A failed or unavailable prerequisite remains
**SPRINT GOAL UNMET**.

## Pinned inputs

- Scala source: `777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0).
- #2179 measured baseline revision: `89f81940fc538a7b24323b82b9415cdb6a7c5404`.
  Successful focused workflow run `42427679560` on PR head
  `92a291efad106555b2be6aa5255ef2a45eac1985`; artifact `13595484615`,
  SHA-256 `ca52bc903a18a45c3e408290a9728fc770fc2ceabb8e4d7c337e970182a43084`.
- Integration revision: `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`.
- #2183 integration receipt: SHA-256 `e783ece1710b7b271c3ca3f9dd12b566b212b655c28d89da35bc900fbccf486e`.
- Merged prerequisite commits, all verified as ancestors of the integration
  revision: #2179 `0c2e6108bfe9dec05b6850bf198390cfdcd6b30d`; #2180
  `dbec2acb7fe9ce8dd43078b55e52f9dba56f05cc`; #2181 `378a01d0b224b9a8c0bb04b80a5e1495a365c8c4`
  and `f8e53f1bba9989422ec0b76d3f34f8b452decb0c`; #2182
  `e863222c5cfec937352ea9f1edcc43d2bdc63d9a`; #2183
  `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`.
- Main/analysis manifests and protected identity files remain the unchanged
  inputs documented in [the #2179 baseline](typer-stdlib-acceptance-iv-baseline-3.9.0.md).
- Artifact hashes and JDK 21 provenance are recorded in that baseline and
  checked by the existing hosted workflow.

The PR head and final merge SHA are intentionally unresolved in this
pre-review snapshot. The hosted artifact records the tested PR head; update
the closeout record with the final merge SHA only after merge.
