# Source semantic TASTy pickler boundary

This crate implements the **read-only plan validation** increment for Scala 3.9.0
TASTy (28.9.0). It does not encode TASTy yet.

`PicklePlan` borrows the canonical semantic store, Definitions, untyped/typed ASTs,
source semantic index and a `SourceTypedBindings` provider. Source metadata is
owned; declarations carry explicitly ordered canonical IDs and exact body roots.
`validate()` returns an immutable `ValidatedPicklePlan` or a structured
`PickleError`, without allocating into compiler arenas or publishing artifacts.

The first accepted shape is the oracle's ordinary public class in one explicit
named package, implicit Object parent, default constructor and one parameterless
method with a declared Int result and a typed Int literal. Clause boundaries are
preserved in the input even though nonempty/empty-parens methods remain deferred.
Source Int literals, including the parser's raw decimal, hexadecimal and binary
whole-number nodes, must agree with the typed value. Raw spellings are checked
against the name-byte limit before decoding; malformed whole numbers and
unsupported noninteger numbers yield typed errors.
The wire constructor's Unit return syntax is distinct from the semantic
constructor's class-instance result.

All IDs and binding providers must belong to the same compilation session.
Arena-relative IDs cannot authenticate a foreign same-slot identity. The provider
must expose the actual frontend index; validation cannot authenticate arbitrary
caller code. The filename and source-text length remain producer-owned facts;
validation checks normalized relative filenames and available source-span IDs.

Only `dotty-core` and `dotty-tasty` are normal dependencies. Frontend dependencies
are test-only, and the tests need no Scala/JDK installation:

```sh
cargo test --locked -p dotty-tasty-pickler
```

See the [design contract](../../docs/tasty-pickler-design.md) and independently
verified [oracle](../../docs/tasty-pickler-oracle.md) for the frozen shape, error
and ownership rules, and follow-up encoding/interoperability gates.
