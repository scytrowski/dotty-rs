# Source semantic TASTy pickler boundary

This crate implements **read-only plan validation, semantic type/reference
projection, declaration skeletons and complete literal, formal-reference,
same-class call, conditional and bounded block bodies** for Scala 3.9.0 TASTy
(28.9.0). `ValidatedPicklePlan::pickle()` returns owned `PickledTasty`
bytes with the actual existing typed RHS. Independent Scala compilation of this
producer's output is covered by the pinned opt-in oracle for the `Answer`,
`Parameters`, same-class `References`, `Operations` call and `Blocks` fixtures.

`PicklePlan` borrows the canonical semantic store, Definitions, untyped/typed ASTs,
source semantic index and a `SourceTypedBindings` provider. Source metadata is
owned; declarations carry explicitly ordered canonical IDs and exact body roots.
`validate()` returns an immutable `ValidatedPicklePlan` or a structured
`PickleError`, without allocating into compiler arenas or publishing artifacts.

Validation is split by responsibility inside the private `plan` module:
`source_binding` owns source/typed identity and span checks, while
`expression_validation` owns the supported literal, formal-reference,
same-class application, conditional and block shapes. New expression forms should extend that
validator and its focused tests; they should not add a second binding or span
policy in serialization. Follow-up pickler increments can build on these
boundaries without changing the borrowed input contract or current support
surface.

The accepted shape is the oracle's ordinary public class in one explicit named
package, implicit Object parent, a default constructor or one primary
constructor with one plain Int parameter clause, and one or more public methods
with an Int or Boolean result. Plain constructor parameters retain their
distinct class-owned template and constructor-owned signature identities;
source `val` and `var` accessor parameters remain unsupported. Bodies may be typed Int/Boolean literals,
references to a same-typed ordinary formal, ordinary parameterless references,
or bounded recursive applications of a canonical same-class method with one
plain clause of Int parameters.
An Int method may also contain a typed conditional whose condition is a
Boolean formal reference or Boolean literal and whose branches are supported
Int expressions or nested conditionals. The condition/then/else structure is
preserved in TASTy; the pickler does not fold constant conditions.
An Int block is accepted only when source and typed block roots and every
statement/final-expression edge have exact frontend bindings. Supported Int
statements are emitted in source order; TASTy encodes the final expression
first, then the ordered stat list. Nested source blocks, local declarations,
assignments, throws and unmatched typed-statement expansions remain unsupported.
A statement is never discarded or coerced to Unit.
Same-class method references derive their
target from the typed `TermRef`, require a public declared Int method in the
plan, and serialize as the Scala 3.9 named `SELECT` with a class-typed
receiver. An unqualified reference uses `QUALTHIS`, while an explicit `this`
selection preserves the oracle's `THIS(TYPEREFsymbol(...))` shape; forward
references do not create synthetic AST addresses.
Method signatures may additionally contain one plain formal whose type is a
public, ordinary, non-generic top-level class in the same validated source/JVM
batch and package. The batch binding must provide that canonical class
identity; the pickler checks its source declaration, owner, declaration scope,
`ClassInfo`, and the formal's exact `TypeRef` and source syntax. The formal is
emitted as a method-owned `PARAM` with the pinned external-to-sidecar
`TYPEREF`/`TERMREFpkg` class type. A method body may make a zero-argument
call to one public, concrete, non-generic method with an explicit empty value
clause on that exact formal, when both classes and the target method are
present in the same validated source/JVM/TASTy batch. The selected target must
be the canonical `TermRef`; its owner type shares the formal's class type node.
Arbitrary receivers, external or unplanned targets, overloaded/private/abstract
methods, arguments, type applications, reference-typed method results,
constructors, and multiple class formals remain unsupported.
A method may have no value clause, one explicit
empty clause, or one plain clause of ordinary Int or Boolean
parameters. Parameter IDs, ownership, source scope, types and order must agree
across the source AST, semantic index and completed method signature.
Source Int literals, including the parser's raw decimal, hexadecimal and binary
whole-number nodes, must agree with the typed value. A formal reference is
resolved only from the typed `TermRef` target and must name a canonical parameter
of that exact method; its emitted `TERMREFdirect` points to the bound `PARAM`
declaration. Raw literal spellings are checked against the name-byte limit
before decoding; malformed whole numbers and unsupported noninteger numbers
yield typed errors.
Same-class call applications require the exact selected method's completed
plain MethodType, ordered Int parameters, Int result, and corresponding
canonical Int arguments; Boolean arguments and Boolean-returning calls remain
unsupported. The bounded cross-class call above requires an explicit empty
argument clause and a zero-parameter Int-returning method.
Conditional branch joins accept canonical Int, an Int singleton constant, or a
validated Int formal reference. When the typed conditional itself has an Int
singleton type, both branches must have that same singleton value.
Traversal is bounded by expression-depth and item limits and rejects repeated
typed expression IDs. Type, context, default, varargs and multi-clause
applications remain unsupported. The wire
constructor's Unit return syntax is distinct from the semantic
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
STABLE marker. Its semantic signature still returns the class. Ordinary methods
are emitted in exact source order. A parameterless method has an Int IDENTtpt
and no fabricated empty clause or METHODtype payload; an explicit empty value
clause has EMPTYCLAUSE; one nonempty plain Int clause emits ordered canonical
PARAM declarations with Int type trees. Complete output attaches each method's
actual typed Int literal or direct formal-reference root after its declared
return syntax. Public visibility and empty modifier/annotation tails are checked from symbol records; other
metadata, parameter kinds and clause forms remain unsupported.

The provisional document uses TastyFileBuilder with exact 28.9.0 version checks,
ASTs and SOURCEFILEattr, and structural validation before returning bytes. Its
tooling string says `incomplete declaration skeleton`, with a fixed diagnostic
UUID. Positions, Comments and the compiler SourceFile annotation are omitted per
the reduced-oracle contract. **These bytes are not a completed compiler artifact
and carry no Scala interoperability claim.** The separate complete-file API
adds real RHS emission. The opt-in oracle compiles/runs the `Answer` and
`Parameters` outputs; other shapes remain unverified. Tests compare canonicalized
declarations against the frozen Scala fixture, normalizing the real method RHS, the
permitted SourceFile annotation omission, sharing layout and header/section data.

Only `dotty-core` and `dotty-tasty` are normal dependencies. Frontend dependencies
are test-only, and the tests need no Scala/JDK installation:

```sh
cargo test --locked -p dotty-tasty-pickler
```

See the [design contract](../../docs/tasty-pickler-design.md) and independently
verified [oracle](../../docs/tasty-pickler-oracle.md) for the frozen shape, error
and ownership rules, and follow-up encoding/interoperability gates.

`ValidatedPicklePlan::pickle_with_limits(PickleLimits)` shares the validated
declaration builder and emits the exact method-plan root. `Tree::ty` must agree
with its supported expression and the declared method result. Literal RHSs use
one `INTconst` with the safe codec's signed integer encoding. A block's typed
result type must match its final expression; no inference or Unit conversion is
performed. Its `BLOCK` fields follow the pinned Scala layout: final expression,
then ordered statements. Expression recursion is bounded by depth/item limits
and rejects repeated typed IDs and cycles; unsupported kinds fail with typed
errors.

The owned result exposes `bytes()`, `source()`, `intended_tasty_path()`, canonical
`declarations()` with AST-relative addresses, and `body(method_symbol)` with the
exact serialized typed root. It writes no files. Existing source-span validation
remains in force; Positions, Comments and the compiler SourceFile annotation
remain omitted under the reduced-oracle contract.

Complete output uses tooling string `dotty-rs minimal semantic pickler` and the
pinned Scala PJW64 UUID construction: names payload XOR ASTs payload in the first
64 bits, Attributes payload in the second, both big-endian. This is deterministic
content identity, not a cryptographic digest. A regression verifies that algorithm
against the unchanged Scala-generated Answer fixture.

`PickleLimits` adds a default **16 MiB whole-file budget**, including the header,
name table, section framing and payloads. The final output size is measured with
the existing codec before full-file encoding; excess returns
`LimitExceeded { kind: FileBytes, .. }`. `ProjectionLimits` and `AstBuildLimits`
also apply, including wire depth and AST bytes. Assembly failures return no
partial artifact and leave all borrowed semantic stores and arenas unchanged.
Rust codec round trips and frozen structural/body comparisons are covered here;
the opt-in Scala oracle checks only the explicitly supported fixture cases.
