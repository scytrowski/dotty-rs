# Source semantic TASTy pickler boundary

This crate implements **read-only plan validation and semantic type/reference
projection** for Scala 3.9.0 TASTy (28.9.0). It does not assemble class/definition
trees or complete TASTy files yet.

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

`EncodingContext::for_plan` seeds canonical declaration reservations from a
validated plan. The standalone `new` entry point checks supplied symbols/types
against the borrowed store and source index. Ordinary names share wire spelling
between namespaces, as in upstream NameBuffer; reference tags preserve namespace
and symbol identity. Package paths follow actual canonical owners. Int/Object
references use the audited Definitions mapping, and Unit has explicit
constructor-return APIs. The Object constructor's signed name is a bounded
oracle operation, without a fabricated source constructor symbol.

`reserve_declaration` returns a fresh wire label; `local_reference` can target it
before `bind_declaration` supplies a node of the correct declaration kind.
`register_external` requires a public dependency class and an exact unambiguous
owner scope. External methods, private/shadowed references and arbitrary
signatures remain unsupported. Package scope provenance remains an integration
precondition because core packages do not store their scope on the symbol.

`lower_type` supports canonical type/term references, local/package `this` and one
ordinary monomorphic method clause. Non-plain clauses, nested method-valued
parameters/results, erased/varargs parameters, structural name targets and other
semantic forms return typed errors. Int result syntax wrappers are separate from
semantic types; constructor Unit syntax never changes the class-instance semantic
signature. Sharing keys exact TypeIds. Repeated calls produce SHAREDtype labels
that must follow their original occurrence in emission order.

`finish` returns an owned `ProjectedFragment` (names and AST fragment bytes).
Every label must be bound and emitted exactly once as an ordered root or child;
all local declarations must be bound. The wire codec resolves labels with bounded
iterative layout, recomputing variable-width addresses and lengths. Addresses are
relative to byte zero of that fragment: compose all nodes in one context before
finishing; appending a finished fragment into a larger AST does not relocate it.
The downstream definition producer owns definition payload grammar and full-file
validation. A mutating-call failure poisons the context; discard it. Failed
`finish` calls publish no fragment and can be retried after binding missing labels.

`ProjectionLimits` bounds semantic fuel, names, bytes/name and owner/type depth.
Type recursion has a hard ceiling of 256, including when a larger limit is
supplied. `AstBuildLimits` independently bounds flat wire items, iterative
traversal depth, output bytes and layout passes. Default limits are 1,000,000
semantic items, 100,000 names, 4,096 bytes/name, 256 owner/type depth; wire defaults
are 1,000,000 items, 256 depth, 16 MiB and 64 layout passes.

Only `dotty-core` and `dotty-tasty` are normal dependencies. Frontend dependencies
are test-only, and the tests need no Scala/JDK installation:

```sh
cargo test --locked -p dotty-tasty-pickler
```

See the [design contract](../../docs/tasty-pickler-design.md) and independently
verified [oracle](../../docs/tasty-pickler-oracle.md) for the frozen shape, error
and ownership rules, and follow-up encoding/interoperability gates.
