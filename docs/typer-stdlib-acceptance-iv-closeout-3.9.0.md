# Typer Stdlib Acceptance IV closeout (Scala 3.9.0)

## Outcome: SPRINT GOAL UNMET pending fresh hosted validation and independent review

This checked-in closeout records the integrated evidence available at
`f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`. It is not a current hosted PASS.
The merged Option.map workstream and Tuple.toArray owner-preparation proof are
ancestors of that integration revision. The unchanged 25+25 receipt records
18 typed main roots and 16 typed analysis roots, with the original 16 main and
15 analysis protected successes retained. The exact original `Option.map`
root is typed in the ordinary cohort; unprepared `Tuple.toArray` remains
blocked. The prepared Tuple proof is conditional on explicit owner completion
and does not count as ordinary-audit acceptance.

The PASS condition is at least 17/25 typed main roots and at least one exact
original Option.map or Tuple.toArray root typed by the ordinary audit, with
its fresh-session source/typed mapping proof. Protected identity and result
sets, all 25 attempted roots per cohort, and deterministic repeat reports must
remain intact. Analysis retains its separate 25-root denominator and 15
protected successes. The exact per-root evidence snapshot is in
[`closeout.tsv`](typer-stdlib-acceptance-iv/closeout.tsv); its provenance is
explicitly the checked-in integration receipt, not a rerun at this PR head.

The existing `typer-focused-audit.yml` closeout gate replays the ordinary
cohorts and root-specific fresh-session proofs, checks exact root/result
identity, the 17/25 threshold and target row, and publishes a run-bound report
and TSV artifact. That hosted run and independent review must pass on the same
PR head before the sprint outcome can be changed to PASS. The hosted artifact
contains the live action URL and tested revision. A failed or unavailable
prerequisite remains **SPRINT GOAL UNMET**.

## Pinned inputs

- Scala source: `777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0).
- #2179 baseline revision: `89f81940fc538a7b24323b82b9415cdb6a7c5404`.
- Integration revision: `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`.
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
