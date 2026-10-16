# TASTy public API

This document describes the supported public surface of `dotty-tasty`, which
is re-exported by the root crate as `dotty::tasty`.

## Compatibility target

The codec targets Scala 3.9.0 and TASTy format `28.9.0`. The default
Scala-specific parsing and validation methods enforce this target. The
`*_compatible_with` methods are explicit escape hatches for format versions
that the caller has verified to be structurally compatible, such as the
28.8.0 compiler corpus used by this repository.

The crate does not implement Scala symbol loading, type checking, semantic
resolution, or JVM class-file loading. It is a binary and structural codec.

## Representation layers

The API keeps ownership and interpretation separate:

| Layer | Main types | Purpose |
| --- | --- | --- |
| Binary | `Reader`, `Writer` | Bounded TASTy primitives and length-prefixed values |
| Raw | `RawName`, `RawTree`, `RawNode`, `Section` | Lossless access when grammar is incomplete or opaque |
| Structured | `StructuredNode`, `StructuredTree` | Decoded AST forms with typed fields |
| Fresh construction | `AstBuilder`, `AstLabel`, `AstNode`, `AstField` | Owned wire nodes and symbolic references, without an original payload |
| File | `TastyFile`, `TastyFileBuilder` | Complete borrowed views and owned construction |

`TastyFile` and the borrowed raw/structured types reference the input byte
slice. `TastyFileBuilder`, `EncodedSection`, and `EncodedTastyFile` own their
encoded data and are suitable for programmatic construction.

## Typical read path

```rust
use dotty::tasty::TastyFile;

let file = TastyFile::parse_and_validate_scala_3_9(bytes)?;
let names = file.names();
let asts = file.asts()?;
let encoded = file.encode()?;
assert_eq!(encoded, bytes);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Use `parse` when validation is intentionally deferred. The
`parse_and_validate_*` variants validate headers, name references, section
references, and AST references before returning the file view.

## Encoding guarantees

- `TastyFile::encode` preserves the parsed file representation and is used for
  raw byte-for-byte round-trip checks.
- `TastyFile::encode_validated` validates the file before encoding.
- `TastyFile::encode_with_ast_addresses` returns the encoded bytes and the
  addresses allocated while encoding AST nodes.
- `TastyFile::structured_asts` exposes structured nodes while retaining raw
  access if a node is opaque or unsupported.
- `TastyFile::encode_structured_relocated` re-encodes structured nodes and
  relocates AST references when node sizes change. Its guarantee is structural
  and semantic compatibility, not byte identity.

Offsets, AST addresses, and compiler bookkeeping are wire-level data. They
are not semantic identities and may change during a relocated structured
encoding.

## Fresh labelled AST fragments

`dotty_tasty::tasty::AstBuilder` (also available through the `construction`
module) addresses the fresh-construction case. Reserve labels before forward
references, bind each label once, and pass roots in emission order to `finish`.
`AstNode` mirrors the wire categories; `AstField::Nat` supports grammar fields
such as method parameter names, while `AstOperand::Reference` is used for
category-four symbol references. Reference leaves and category-four symbol
operands require labels and reject numeric AST addresses in those positions.
Use `SimpleTerm::new` and existing tags for leaves; the builder uses the existing
codec `Writer` for primitives and length prefixes.

Each reserved node must be reachable and emitted exactly once. A reference does
not emit its target. Shared references point backward; direct/symbol references
must target a declaration tag of the corresponding namespace. Child cycles,
unbound/duplicate/unused labels and invalid operands yield `AstBuildError`.
Labels are relative to one builder; same-slot labels from another builder cannot
be authenticated. The builder validates layout and these reference rules; the
caller still owns tag-specific payload grammar, names and full-file validation.

Layout repeatedly sizes nodes using current address estimates, then assigns new
addresses in root/child order until stable. Every pass recomputes Nat and length
widths rather than patching compact bytes in place. `BuiltAst` owns fragment bytes
and the final label-to-address mapping. Addresses are relative to the fragment's
byte zero, so callers compose the complete intended AST before finishing.
`AstBuildLimits` defaults to 1,000,000 labels/fields, depth 256, 16 MiB and 64
layout passes. Checked arithmetic rejects machine and u32 wire overflow.

## Errors and resource limits

Public parsing and encoding methods return typed errors. Malformed input is
reported as `ReadError`, `HeaderError`, `NameTableError`, `SectionError`,
`AstError`, `TermError`, `AstBuildError`, or `TastyFileError` rather than causing
a panic.

Recursive AST and name-table operations have bounded variants. The default
limits are exposed as `DEFAULT_MAX_TREE_DEPTH` and
`DEFAULT_MAX_AST_INDEX_DEPTH`; callers processing untrusted input should use
the explicit `*_with_max_*` methods when a stricter budget is appropriate.

## API boundaries

The root crate exposes the same TASTy items through `dotty::tasty` and the
implementation crate exposes them through `dotty_tasty::tasty`. The module
names (`ast`, `construction`, `file`, `header`, `name_table`, `reader`, `section`,
`term`, and `writer`) remain available for callers that need to distinguish raw, binary,
structured, and file-level operations.
