# Automatic source owner preparation for `Tuple.toArray`

This records the #2234 caller-sequencing change for the original Scala 3.9.0
root `library/src/scala/Tuple.scala#tree=23`. The source fixture is pinned to
`777528f19a58e794c9954a42f433373472ec57f8`; its `Tuple.scala` SHA-256 is
`4d33bdb23f8446d313d531defd9957bb82672bd04211bca54d4bf08309c14d8e`.

## Causal trace

On the exact implementation base `5ea3d34d17730e969654f7abb3ac011d89e6528a`,
the ordinary `type_method_definition` replay first failed at original
application tree 22, argument 0, with
`ApplicationArgumentConformanceUnsupported`. Its actual was
`ThisType(scala.Tuple)` for the canonical source owner; the selected external
member was `.scala.runtime.Tuples$.toArray`, and its loaded formal was an
`ErrorType` after the classpath/source `scala/Tuple` identity collision. The
source `Tuple` class info was still `Missing`. The failure observer ran before
the enclosing transaction rolled back.

The unchanged explicit-preparation replay at that base completed the canonical
source `Tuple`, then typed the original method. Its selected external formal
was `TypeRef` of that same source `Tuple` symbol; the source argument remained
`ThisType(scala.Tuple)`. This reproduced the #2183 differential and confirmed
that no new conformance rule was needed.

After #2234, ordinary method typing prepares the authenticated current-source
owner before resolving a qualified selected call that has exactly one direct
`this` argument. The unmodified root now produces:

```text
TUPLE_TO_ARRAY_ACCEPTED source=Tuple.scala#tree=23 method=.scala.Tuple.toArray owner=.scala.Tuple mappings=17:0,18:1,19:2,20:4,21:3,22:5,23:6 selected=.scala.runtime.Tuples$.toArray argument=scala.Tuple.this result=scala.Array[java.lang.Object]
TUPLE_TO_ARRAY_AUTOMATIC_PREPARATION source_owner=.scala.Tuple external_formal=TypeRef(source_owner) rollback_retry=passed
```

## Boundaries and regression coverage

The preparation uses the existing source index and declaration completion
inside the application transaction. It completes only the owner authenticated
from that direct `this` argument. It preserves the selected TASTy method and
formal identities and leaves member lookup, classloader identity, and
`TypeRelation` behavior unchanged.

The focused tests cover the ordinary complete root, a late injected failure
followed by a same-session retry, a same-spelled source owner in another
package, an unrelated receiver/source class, an unqualified unsupported call
shape, malformed preexisting owner info, and rejection of an inconsistent
formal. Failed root typing restores owner info and source-to-typed mappings.

Both unchanged ordinary manifests were run with determinism enabled. All 18
protected main results and 16 protected analysis results retain their prior
result shapes. The main cohort now has 19/25 typed roots (the original Tuple
root is the one additional success); analysis remains 16/25.
