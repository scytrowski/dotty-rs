# Source semantic TASTy pickler boundary

This crate implements **read-only plan validation, semantic type/reference
projection and declaration skeletons** for Scala 3.9.0 TASTy (28.9.0).
Declaration documents are explicitly incomplete: method RHS emission and
completed interoperable artifacts remain downstream work.

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
ordinary monomorphic method clause. Non-package `this` requires a supported local
class symbol; a type parameter cannot denote a class `this` type. Non-plain
clauses, nested method-valued parameters/results, erased/varargs parameters,
structural name targets and other semantic forms return typed errors. Int result
syntax wrappers are separate from semantic types; constructor Unit syntax never
changes the class-instance semantic signature. Sharing keys exact TypeIds.
Repeated calls produce SHAREDtype labels that must follow their original
occurrence in emission order.

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

`ValidatedPicklePlan::declaration_skeleton()` assembles the frozen one-package,
one-class structure using that projection context. The result is a
`DeclarationSkeleton`, with `provisional_bytes()` for structural inspection,
canonical declaration/owner IDs and AST-relative addresses, original source
metadata, the intended package-derived `.tasty` path, and `omitted_bodies()`
containing the exact real typed RHS roots. It does not write a file. The
`declaration_skeleton_with_limits(ProjectionLimits)` variant applies the existing
semantic/name/wire budgets; any failure returns a typed error without publishing
partial output or mutating the compilation session.

Each artifact path component uses the pinned Scala name encoding (for example,
`A-B` becomes `A$minusB`), while source spelling stays unchanged in TASTy names
and source metadata. Non-identifier characters use uppercase `$uNNNN` escapes
per UTF-16 code unit. Java identifier-part classification is frozen to the
oracle's Temurin 21.0.8 / Unicode 15.0 table, so builds need no JDK and do not
silently change encoding when the host Unicode tables change.

The parent application comes from the validated ClassInfo's canonical Object
parent and the frozen oracle's `APPLY/SELECTin/NEW` shape. The default constructor
has EMPTYCLAUSE, direct Unit return syntax, no RHS and its provenance-required
STABLE marker. Its semantic signature still returns the class. The ordinary
parameterless method has an Int IDENTtpt and no fabricated empty clause,
METHODtype payload, abstract flag or substitute RHS. Public visibility and empty
modifier/annotation tails are checked from symbol records; other metadata is
unsupported. Parameterized/empty-parens methods and other class forms remain
outside the validated v0 contract.

The provisional document uses TastyFileBuilder with exact 28.9.0 version checks,
ASTs and SOURCEFILEattr, and structural validation before returning bytes. Its
tooling string says `incomplete declaration skeleton`, with a fixed diagnostic
UUID. Positions, Comments and the compiler SourceFile annotation are omitted per
the reduced-oracle contract. **These bytes are not a completed compiler artifact
and carry no Scala interoperability claim.** #1194 adds real RHS emission; #1195
owns independent compilation. Tests compare canonicalized declarations against
the frozen Scala Answer fixture, normalizing only the real method RHS, the
permitted SourceFile annotation omission, sharing layout and header/section data.

Only `dotty-core` and `dotty-tasty` are normal dependencies. Frontend dependencies
are test-only, and the tests need no Scala/JDK installation:

```sh
cargo test --locked -p dotty-tasty-pickler
```

See the [design contract](../../docs/tasty-pickler-design.md) and independently
verified [oracle](../../docs/tasty-pickler-oracle.md) for the frozen shape, error
and ownership rules, and follow-up encoding/interoperability gates.
