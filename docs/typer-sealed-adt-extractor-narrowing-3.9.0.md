# Pattern-only extractor narrowing (Scala 3.9.0)

Extractor applicability retains the existing `selector <: unapply input`
check. If it is false, the pattern typer may also use the bounded nominal
relation `unapply input <: selector`, but only when both types are direct
class or trait references and the completed canonical class metadata proves
the parent path. This second check is local to extractor patterns; global
`conforms` and `is_subtype` behavior is unchanged. Applied, union, intersection,
and other unsupported shapes remain deferred with their relation error.

The accepted branch uses the authenticated `unapply` input as the `UnApply`
prototype. It leaves the source selector and `unapply` input/result types
unchanged. Components still come from the authentic unapply result and its
canonical Product selectors, so `Created._1` and `Deleted._1` remain separate
variant-owned declarations. Sealedness is not part of the nominal relation;
the pinned nonsealed control is accepted as well.

## Evidence

`source_sealed_adt_match_baseline.rs` checks that both pinned `Event` method
roots move past their original `ExtractorPatternConstraintDeferred` first
errors. [`rust-before-after.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-before-after.tsv)
records those original locations and the post-change outcome. The already
frozen Product1 source roots have the same first-error movement, recorded in
[`rust-product1-first-error-movement.tsv`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/rust-product1-first-error-movement.tsv).
The focused
pattern tests verify both distinct variant inputs and unapply symbols, the
unchanged `Event` selector parameter type, each variant-owned `_1: Int`, an
unrelated input remaining deferred, an unchanged forward-supertype prototype,
and rollback followed by a successful valid retry in the same typer session.

The Scala 3.9.0 oracle is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`; all compiler jars were verified
against the checked-in SHA-256 manifests. The `Event` and nonsealed examples
are accepted. The new `InputSupertype.scala` forward-direction control is
accepted with the Scala exhaustivity warning. Existing `Box` and `Flagged`
controls, the same-spelled unrelated extractor warning, and rejection cases
remain in the case-class/Product2 oracle matrices. The matrices preserve
warning-only cases as warnings rather than treating them as rejected source.

The oracle scripts pin Temurin JDK 21.0.8+9. This environment provided
Temurin 21.0.13+9, so the three Scala 3.9.0 runners were executed from temporary
copies that changed only their JDK-version guard; artifact hashes, source
hashes, compiler revision, snapshots, and classifications still matched.
Rerun the checked-in scripts unchanged with the required 21.0.8 installation
for the exact environment gate.

The receipt records first-error movement for these two bounded roots. It does
not add exhaustivity analysis, general pattern constraint inference, or a
general source `Match` contract.
