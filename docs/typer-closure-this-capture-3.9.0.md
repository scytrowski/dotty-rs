# Typer closure capture of lexical `this` (Scala 3.9.0)

An ordinary `Function1` lambda in a source class or trait instance method can
refer to the lexically enclosing instance with `this` or a qualified enclosing
reference such as `Outer.this`. The typed source expression retains its
canonical `Type::ThisType`, and the closure environment records that same typed
`this` tree under the canonical enclosing class or trait `SymbolId`. Repeated
references to the same enclosing instance add one environment entry. A
qualified reference is accepted only when its class is in the lexical owner
chain. Module receivers and local or synthetic methods do not qualify as
instance methods for this capture path.

When the lambda has an expected `Function1` result, the body is checked against
that result before the expected result is retained in the synthetic lambda
method and closure type. This preserves the precise `ThisType` on the body while
keeping the closure's declared function type canonical.

Focused regression coverage lives in
[`enclosing_this_capture.rs`](../crates/dotty-typer/src/typer/tests/enclosing_this_capture.rs).
It checks class, trait, qualified, repeated, and mixed parameter/instance
captures, synthetic lambda ownership, mismatch rollback and retry, module
receiver rejection, and a non-enclosing qualifier. The pinned Scala 3.9.0 source oracle remains the compatibility reference at
revision `777528f19a58e794c9954a42f433373472ec57f8`. Its execution was not
available in this environment: no pinned Scala checkout or cached compiler
artifacts were present (`SCALA39_ROOT` was unset).

Unqualified member calls, field reads through a captured receiver, mutable or
object captures, arbitrary nested closure capture, `super`, JVM lowering, and
TASTy output remain outside this increment.
