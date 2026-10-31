# Inherited immutable source-field reads (Scala 3.9.0)

Issue #1905 audits the complete source method:

```scala
class Parent:
  val bias: Int = 2

class Child extends Parent:
  def withBias(x: Int): Int = x + bias
```

The Scala 3.9.0 oracle evidence is inherited from the pinned #1903 fixture at
compiler revision `777528f19a58e794c9954a42f433373472ec57f8`. Its typed-tree
snapshot confirms that `Parent.bias` is a source field owned by `Parent`,
`Child` has the direct `Parent` edge, and both `bias` and `this.bias` are valid
references in a method body returning `Int`. The corresponding Rust baseline
already completed `Child.withBias` before this issue; #1904's inherited-method
change did not alter that result. Since the assigned positive was already
green, this increment adds no production resolver change.

The focused regression now inspects the mapped `DefDef`, its source body and
the original field reference. It asserts that unqualified `bias` and the
explicit `this.bias` comparator both point to the exact source `Parent.bias`
symbol, retain the `Child.this` receiver view, have the original source span
and `SourceTypedIndex` mapping, and keep the exact `Int` value/result type.
It also verifies that the inherited declaration remains owned by `Parent` and
does not appear in `Child`'s declaration scope. The four-root baseline runs in
two fresh sessions, so the canonical identity assertions run against distinct
semantic stores.

Negative and precedence controls cover missing, private, unrelated, and
wrong-typed fields, invalid Parent `ClassInfo`, local and Child-owned
shadowing, a local import that shadows the inherited field, a mutable `var`
whose `MUTABLE` flag remains intact, and failure rollback followed by a valid
same-session retry. The mutable control tests a read only; inherited writes
remain outside this issue. These Rust results and
the frozen target classifications plus Rust-only controls are summarized in
[`inherited-immutable-field.tsv`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/inherited-immutable-field.tsv).

Focused validation:

```sh
TMPDIR="$PWD/tmp" CARGO_TARGET_DIR="$PWD/target" \
  cargo test -p dotty-typer --test inherited_members_baseline --locked
```
