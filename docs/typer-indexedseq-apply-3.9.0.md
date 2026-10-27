# External `IndexedSeq.apply` typing in Scala 3.9.0

This report records the bounded generic TASTy projection work for issue #1746.
It separates the isolated nested application from the frozen `IterableOnce`
local-definition roots and the unrelated `Predef.charAt` control.

## Inputs

- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`
- Scala artifacts: 3.9.0
- dotty-rs baseline: `1d4d4cee70e66fdc7b673c16c7fb48126c03bd8`
- Audit runtime: JDK 25.0.4 (JDK 21 is not installed in this environment)
- Frozen root selection: `IterableOnce.scala#tree=2043` and `#tree=2097`

## Implemented behavior

The classloader now retains the declaration-ordered class type parameters,
member references to those parameters, and applied parent views represented by
the bounded TASTy decoder. It enters canonical class-owned type-parameter
symbols and gives the typer their ordered identities through the resolver.
Unsupported parent argument trees remain explicit error slots. The typer
substitutes only by canonical symbols and selects the unique nearest inherited
`apply` when the inherited `SeqOps` and `Function1` method signatures are
mutually equivalent.

An isolated generic method with a parameter of exactly
`scala.collection.IndexedSeq[X]` now types `seq(at)` as a real nested
application. Its callee is the actual external `apply` member, its index is
checked as `Int`, and its result retains the source method's exact `X` symbol.
Wrong index type and wrong arity remain errors; their failed source mappings
are rolled back. A valid nested application succeeds in the same typer after
those failures.

## Measured audit results

The focused audit of the two frozen local definitions was run twice with
normalized deterministic output. It remains **0/2 fully typed**:

| Root | Result | First blocker |
| --- | --- | --- |
| `IterableOnce.scala#tree=2043` (`foldl` local `loop`) | Not fully typed | `ApplicationCalleeNotMethod` at the initializer call `loop(start, seq.length, z)` (line 711) |
| `IterableOnce.scala#tree=2097` (`foldr` local `loop`) | Not fully typed | `ApplicationCalleeNotMethod` at `seq.length - 1` (line 718) |

These roots stop before the loop bodies, so this audit does not measure their
`seq(at)` / `seq(at - 1)` calls. The isolated regression proves that one such
nested call is now supported, but does not establish complete typing of either
local `DefDef`.

The separate strict ordinary-method audit of `Predef.scala#tree=614`
(`SeqCharSequence.charAt`) was also run twice with deterministic output. It
remains **0/1 typed**, blocked by `TermNameNotFound` for inner tree 611 at
`sequenceOfChars(index)`. This has not been connected to the external generic
`IndexedSeq.apply` path and is not counted as uplift.

The repository's three-file focused smoke audit was also run twice, and both
normalized reports matched the CI artifact and each other. It remains **0/31
fully typed**. Fourteen `BCodeBodyBuilder.scala` roots now reach `MemberLookup`
where the prior checked-in report stopped at `TypeNameNotFound`; one more
`BCodeBodyBuilder` root moved from `TermNameNotFound` to `MemberLookup`. The
smoke report therefore records that blocker movement, but it does not show a
fully typed method or establish broader corpus uplift. The checked-in focused
baseline was updated to this exact report so CI will compare against the
reviewed post-change observations. On this same selection, package requests
changed from 4,177 to 128, member requests from 3,341 to 102, and recorded
class-symbol successes from 827 to 31. These are focused resolver counters,
not full-corpus metrics. The lower counts accompany the first-blocker shift;
they do not establish typing uplift or by themselves say whether the reduced
resolver activity is beneficial.

Focused reports and retained per-run logs are in the task-local ignored
`target/issue-1746/reports/` and `target/issue-1746/*audit-tmp-final-expanded/`
directories. They are evidence for these selected roots only, not corpus-wide
coverage claims.
