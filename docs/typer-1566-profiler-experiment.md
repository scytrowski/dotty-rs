# #1566 experiment: `Profiler.RealProfiler.completionName`

## Question

Does resolving the `root.owner` selection make the focused method
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

The root is still not typed: **0/1**. Parsing and naming succeed. The reported
first blocker is `MemberNotFound` for `owner`.

However, inspecting the captured receiver changes the interpretation of that
error. The receiver symbol has `SymbolFlags(16386)` and two parent types; the
audit also materializes `scala.Symbol`. In pinned Scala source,
`dotty.tools.dotc.core.Symbols.Symbol` is a non-final class extending three
traits (`ParamInfo`, `SrcPos`, and `Showable`). The measured receiver therefore
matches `scala.Symbol`, not the source-level `dotty.tools.dotc.core.Symbols.Symbol`.
The resolver trace also reports that the `Symbol` type member under
`dotty.tools.dotc.core.Symbols` has no available class member scope because the
`Symbols` class info is incomplete.

The source-level Scala behavior is still relevant context: `Symbols.scala`
defines `implicit def toDenot(sym: Symbol)(using Context)`, and
`SymDenotations.scala` defines `SymDenotation.owner`. `RealProfiler` has a
contextual `Context` constructor parameter. This explains how the pinned Scala
compiler can accept the source expression, but the current audit has not
reached a correctly resolved receiver on which to test that conversion path.

## Conclusion

This experiment does **not** establish that implicit-conversion adaptation is
the current typer blocker, nor does it test whether adding that adaptation
would make the method type. The earlier diagnosis attributed the numeric
receiver ID to the nested Dotty `Symbol` type; that attribution was incorrect.
The measured `MemberNotFound` is on a receiver that appears to be `scala.Symbol`,
while resolution of the source-level nested Dotty type is incomplete.

The next bounded step is to determine why the external nested class
`dotty.tools.dotc.core.Symbols.Symbol` is not materialized from the available
TASTy/classpath metadata, and make the audit report the canonical receiver
identity. Rerun this exact root after that is fixed. Only if it then fails on
`owner` for the correctly resolved Dotty `Symbol` should implicit-conversion
search and contextual `Context` insertion be tested as the next change.

No typer behavior is changed by this experiment. The method does **not** start
typing in this PR; the next experiment has to resolve the source receiver type
before it can answer whether the `toDenot` adaptation is sufficient.

## Reproduction

Run `tools/typer-classpath-corpus-audit/run` with a focused manifest containing
`compiler/src/dotty/tools/dotc/profile/Profiler.scala` and a method selector
for `Profiler.scala#tree=1667`, using the pinned Scala 3.9.0 and JDK 21 inputs.
The focused output for this run reports `methods_attempted=1`,
`methods_typed=0`, and `blocker=MemberNotFound`. The generated audit output is
not checked in.

## Follow-up: nested class identity experiment

I added a classloader/resolver experiment for the missing source member
`Symbols.Symbol`. The fixture uses Scala 3.9.0 output for
`object Holder { class InProtected }`, containing `Holder.tasty` and
`Holder$InProtected.class`. It verifies that the module identity comes from
TASTy, the nested JVM class is accepted only when its `InnerClasses` entry
names the requested source member and facade owner, and the resulting class
symbol has the canonical module class as semantic owner. An absent nested name
still resolves to `None` and rolls back the semantic store.

The focused classloader suite passes (440 unit tests, 13 directory classpath
tests, 1 JDK classpath test, 2 visibility tests, and 18 TASTy-loading tests;
4 tests ignored because they require optional JDK tooling).

The same one-method Scala 3.9 audit still reports **0/1 typed**. The first
blocker is now `SymbolResolution`, with the failing request
`Symbols$.Symbol`; class loading fails through the nested hierarchy. A direct
reproduction against the pinned Scala compiler jars shows the chain reaches
nested classes such as `Trees$Tree`, `Types$Type`, and `Texts$Text`, whose
classfile outer names point at top-level object facades (`Trees`, `Types`,
`Texts`) that have module TASTy but no class `TypeDef`. The checked-in fixture
proves the narrow `Holder` case, but the target compiler hierarchy still
exposes another classloader path that does not use the new owner fallback.

Therefore this change does **not** make `RealProfiler.completionName` type.
It isolates the supported nested-member evidence and preserves the negative
result; a follow-up must locate and fix the transitive class dependency path
before claiming progress on #1566. The generated focused audit output remains
local and is not checked in.
