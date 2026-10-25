# #1566 experiment: `Profiler.RealProfiler.completionName`

## Question

Can resolving the missing `Symbol.owner` selection make the focused method
`completionName` type successfully?

## Scope and setup

The experiment targets one root only:

- Scala compiler source: `compiler/src/dotty/tools/dotc/profile/Profiler.scala`
- method: `RealProfiler.completionName`
- focused expression: local `isTopLevel`, at line 357 of the pinned Scala 3.9.0
  source
- Scala revision: `777528f19a58e794c9954a42f433373472ec57f8`
- JDK: 21.0.8
- typer code baseline: `6dec6f90cd9bc581501b6f83087c8311a84304d5`

I reran the focused typer/classpath corpus audit with an external manifest
containing only this source file and a focused-method entry selecting the
`isTopLevel` root. The audit is partial by design; its result is not a corpus
coverage metric.

## Result

The root is still not typed: **0/1**. Parsing and naming succeed. Its first
blocker is `MemberNotFound` for selecting `owner` on the `Symbol` receiver.
The rerun reported the same method tree, source site, and blocker as the prior
focused run. The current `main` changes after this baseline do not touch the
typer member-selection implementation used by this root (the intervening typer
change is Java nullary value adaptation).

The pinned Scala implementation explains the source-level behavior:

- `Symbols.scala` defines `implicit def toDenot(sym: Symbol)(using Context)`.
- `SymDenotations.scala` defines `SymDenotation.owner`.
- `completionName` is inside `RealProfiler`, whose constructor has a contextual
  `Context` parameter.

So `root.owner` is accepted by Scala through an implicit conversion and
contextual argument insertion. It is not an inherited-member lookup on
`Symbol`.

## Conclusion

This experiment does **not** show that a small member-lookup fix would make the
method type. The typer currently lacks the complete adaptation needed here:
find an eligible implicit conversion through the receiver's implicit scope,
check its source and result types, resolve the contextual `Context` argument,
and construct the corresponding typed application before selecting `owner`.
A special case keyed by the spelling `owner`, `Symbol`, or `toDenot` would not
be a sound implementation of those semantics, so this PR does not add one.

The method therefore **does not start typing** in this change. The concrete
next step, if continuing this path, is a separately tested, bounded implicit-
conversion adaptation increment. Its first acceptance test should cover this
expression and verify the exact converted receiver and selected `owner`
symbol; a full `completionName` audit is required to establish whether later
expressions introduce another blocker.

## Reproduction

Run `tools/typer-classpath-corpus-audit/run` with a focused manifest containing
`compiler/src/dotty/tools/dotc/profile/Profiler.scala` and a method selector
for `Profiler.scala#tree=1667`, using the pinned Scala 3.9.0 and JDK 21 inputs.
The focused output for this run is retained as a generated audit artifact, not
checked in; it reports `methods_attempted=1`, `methods_typed=0`, and
`blocker=MemberNotFound`.
