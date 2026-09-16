# TASTy 3.9.0 Format Specification

Status: working document for the Rust implementation.

Normative source: [`TastyFormat.scala` from Scala 3.9.0](https://github.com/scala/scala3/blob/3.9.0/tasty/src/dotty/tools/tasty/TastyFormat.scala).

This document describes the wire format. It is not yet a complete specification of the semantic Scala symbol model. Full compatibility will also require the implementations of `TastyReader`, the writer/serializer, and the compiler unpickler.

## 1. Format model

A TASTy file consists of:

```text
header
version
tooling string
uuid
name table
sections
```

AST trees are serialized as tagged nodes. Some nodes contain a payload length, some contain a reference to another node, and some contain only their tag.

Important properties:

- every grammar terminal is represented by a single byte;
- names are interned in one name table;
- name references are numbered from `1`;
- section and node lengths are byte lengths;
- terms and types share `Path` elements, but their interpretation depends on context;
- name, AST, and attribute tags use separate numeric namespaces.

## 2. Notation and primitive types

The source grammar uses BNF notation. Names beginning with uppercase letters are terminals represented by a single byte. Mixed-case names are non-terminals.

### 2.1. Base-128 integers

```text
LongInt = Digit* StopDigit
Int     = LongInt
Nat     = LongInt       // non-negative value

Digit     = 0 .. 127
StopDigit = 128 .. 255
```

The format is big-endian. Each byte carries 7 data bits:

- `byte & 0x80 == 0`: another byte belongs to the same number;
- `byte & 0x80 != 0`: this is the final byte of the number;
- the fragment value is `byte & 0x7f`.

For `Nat`, fragments are accumulated as follows:

```text
value = 0
value = (value << 7) | (byte & 0x7f)
```

Examples for `Nat`:

```text
0   = 80
127 = FF
128 = 01 80
```

`Int` and `LongInt` use two's complement over the concatenated 7-bit fragments. The implementation must perform correct sign extension to `i32` or `i64`.

The decoder must reject:

- encodings outside `u32`, `i32`, or `i64`;
- unterminated numbers;
- numbers extending beyond the bounded reader;
- negative values decoded as `Nat`.

### 2.2. UTF-8

```text
Utf8 = Nat UTF8-CodePoint*
```

Text is preceded by a length encoded as `Nat`. The exact length unit should be confirmed against the `TastyReader`/writer implementation; in practice, the Scala reader operates on a bounded UTF-8 byte fragment.

## 3. Header and version

### 3.1. File layout

```text
File = Header
       majorVersion_Nat
       minorVersion_Nat
       experimentalVersion_Nat
       VersionString
       UUID
       nameTable_Length
       Name*
       Section*
```

The header consists of four bytes:

```text
Header = 0x5CA1AB1F
bytes  = 5C A1 AB 1F
```

The UUID has exactly 16 bytes:

```text
UUID = Byte*16
```

The 3.9.0 source defines:

```text
MajorVersion        = 28
MinorVersion        = 9
ExperimentalVersion = 0
```

### 3.2. Version compatibility

For a file `(fileMajor, fileMinor, fileExperimental)` and a compiler `(compilerMajor, compilerMinor, compilerExperimental)`, versions are compatible when either of the following holds:

```text
fileMajor == compilerMajor
fileMinor == compilerMinor
fileExperimental == compilerExperimental
```

or:

```text
fileMajor == compilerMajor
fileMinor < compilerMinor
fileExperimental == 0
```

A library can expose two modes:

1. strict: accept only `28.9.0`;
2. compatible: apply the relation above and preserve the file version in the decoded result.

The Rust header API exposes this second relation through
`Header::is_compatible_with(compiler_major, compiler_minor,
compiler_experimental)`. `TastyFile::parse_scala_3_9` remains the strict
entry point for this repository's current compatibility target. The complete
file model additionally exposes `TastyFile::parse_compatible_with`,
`TastyFile::parse_and_validate_compatible_with`,
`TastyFile::parse_and_validate_scala_3_9_with_max_ast_index_depth`, and
`TastyFile::parse_and_validate_compatible_with_max_ast_index_depth`,
`TastyFile::validate_compatible_with`, and
`TastyFile::validate_compatible_with_max_ast_index_depth`, and corresponding
validated builder and encoder methods.

## 4. Name table

### 4.1. References

```text
NameRef = Nat
Utf8Ref = Nat
```

A reference is the ordinal of an entry in the name table, numbered from `1`. In Rust, a `Vec<Name>` with an empty entry at index `0` is convenient and avoids repeatedly subtracting `1`.

### 4.2. Name table entries

```text
Name = UTF8 Utf8
     | QUALIFIED Length NameRef NameRef
     | EXPANDED Length NameRef NameRef
     | EXPANDPREFIX Length NameRef NameRef
     | UNIQUE Length NameRef Nat NameRef?
     | DEFAULTGETTER Length NameRef Nat
     | SUPERACCESSOR Length NameRef
     | INLINEACCESSOR Length NameRef
     | OBJECTCLASS Length NameRef
     | BODYRETAINER Length NameRef
     | SIGNED Length NameRef NameRef ParamSig*
     | TARGETSIGNED Length NameRef NameRef NameRef ParamSig*
```

Tag meanings:

| Value | Tag | Meaning |
|---:|---|---|
| 1 | `UTF8` | ordinary textual name |
| 2 | `QUALIFIED` | qualified name, for example `A.B` |
| 3 | `EXPANDED` | expanded name, for example `A$$B` |
| 4 | `EXPANDPREFIX` | expanded-name prefix, for example `A$B` |
| 10 | `UNIQUE` | unique name with separator and number |
| 11 | `DEFAULTGETTER` | default-argument getter |
| 20 | `SUPERACCESSOR` | `super$name` accessor |
| 21 | `INLINEACCESSOR` | `inline$name` accessor |
| 22 | `BODYRETAINER` | synthetic method retaining an inline body |
| 23 | `OBJECTCLASS` | module class `A$` |
| 62 | `TARGETSIGNED` | name, target name, and signature |
| 63 | `SIGNED` | name and signature |

`ParamSig` is an `Int`:

```text
ParamSig < 0  =>  -ParamSig is the length of a type-parameter section
ParamSig > 0  =>  ParamSig is a NameRef for a fully qualified term parameter name
```

Zero is not a valid `ParamSig`, and the minimum `i32` value is invalid because
its negation cannot represent the length of a type-parameter section.
The Rust API keeps the raw `ParamSig` value for lossless encoding and exposes
`interpret_param_sig()` as a typed view returning `ParamSigValue`.
`RawName::signed_name()` builds a `NameSignature` view for `SIGNED` and
`TARGETSIGNED` entries, preserving the original/target references and the
wire order of interpreted parameters. It returns no view for non-signature,
unknown, or malformed raw entries.
`NameTable::render_signed_name()` resolves that view to
`RenderedSignedName`, including rendered result and term-parameter names while
keeping type-parameter section lengths explicit. Nested `SIGNED` and
`TARGETSIGNED` references are rendered iteratively in a structural diagnostic
form such as `name[with sig result()]`; this form is intended for inspection,
not as a replacement for the raw signature fields.

Name references are range-checked, and cyclic name-entry dependencies are
rejected by the validated name-table model.

Unknown name tags are preserved as raw length-delimited entries. Programmatic
construction rejects tags already assigned to a known name-entry grammar so
that an unknown entry cannot change meaning after a decode/encode cycle.

The `SIGNED` and `TARGETSIGNED` codes are intentionally unusual: `TARGETSIGNED=62`, `SIGNED=63`. The source contains a TODO about possibly swapping these values at the next major version.

## 5. Sections

```text
Section = SectionNameIndex Length Bytes
SectionNameIndex = Nat
Length  = Nat
```

`Length` is the number of remaining bytes in the section payload. The recommended decoding pattern is:

```text
name_index = read_nat()
len  = read_nat()
end  = current_offset + len
payload_reader = sub_reader(current_offset, end)
parse(payload_reader)
require payload_reader.offset == end
current_offset = end
```

Standard section names are:

```text
ASTs
Positions
Comments
Attributes
```

The section name is an index into the `NameTable`, not inline text. The Scala 3.9.0 compiler emits this section index as zero-based (`ASTs` is `0`, followed by the later standard-section names). This is distinct from the one-based `NameRef` convention used by names referenced from AST nodes and composite name entries. Unknown sections can be skipped using their length, provided that bounds are validated correctly.

## 6. AST grammar

### 6.1. Top-level statements and definitions

```text
TopLevelStat = PACKAGE Length Path TopLevelStat*

Stat = Term
     | TYPEDEF Length NameRef (Type | Template) Modifier*
     | IMPORT Length Term Selector*
     | EXPORT Length Term Selector*

VALDEF  Length NameRef Type Term? Modifier*
DEFDEF  Length NameRef Param* Type Term? Modifier*
TYPEDEF Length NameRef (Type | Template) Modifier*

Selector = IMPORTED NameRef
         | RENAMED NameRef
         | BOUNDED Type

TypeParam = TYPEPARAM Length NameRef Type Modifier*
TermParam = PARAM      Length NameRef Type Modifier*

Param = TypeParam | TermParam

EMPTYCLAUSE
SPLITCLAUSE

Template = TEMPLATE Length
           TypeParam*
           TermParam*
           Type*
           Self?
           SPLITCLAUSE?
           Stat*

Self = SELFDEF NameRef Type
```

Lists marked with `*` usually do not have a separate count. Their end is determined by the surrounding grammar or by the `Length` boundary of the enclosing node.

### 6.2. Terms and expressions

```text
Term = Path
     | IDENT NameRef Type
     | SHAREDterm ASTRef
     | SELECT NameRef Term
     | SELECTin Length NameRef Term Type
     | QUALTHIS TypeIdentTree
     | NEW Type
     | ELIDED Type
     | THROW Term
     | NAMEDARG NameRef Term
     | APPLY Length Term Term*
     | APPLYsigpoly Length Term Type Term*
     | TYPEAPPLY Length Term Type*
     | SUPER Length Term Type?
     | TYPED Length Term Type
     | ASSIGN Length Term Term
     | BLOCK Length Term Stat*
     | INLINED Length Term Term? ValOrDefDef*
     | LAMBDA Length Term Type?
     | IF Length INLINE? Term Term Term
     | MATCH Length (IMPLICIT | INLINE? | SUBMATCH?) Term CaseDef*
     | TRY Length Term CaseDef* Term?
     | RETURN Length ASTRef Term?
     | WHILE Length Term Term
     | REPEATED Length Type Term*
     | SELECTouter Length Nat Term Type
     | QUOTE Length Term Type
     | SPLICE Length Term Type
     | SPLICEPATTERN Length Term Type Type* Term*
```

### 6.3. Patterns

```text
CaseDef = CASEDEF Length Pattern Term Term?

Pattern = BIND Length NameRef Type Pattern
        | ALTERNATIVE Length Term*
        | UNAPPLY Length Term ImplicitArg* Type Pattern*
        | QUOTEPATTERN Length Term Term Type Term*

ImplicitArg = IMPLICITarg Term
```

### 6.4. Type trees

```text
TypeTree = IDENTtpt NameRef Type
         | SELECTtpt NameRef Term
         | SINGLETONtpt Term
         | REFINEDtpt Length Term Stat*
         | APPLIEDtpt Length Term Term*
         | LAMBDAtpt Length TypeParam* Term
         | TYPEBOUNDStpt Length Term Term?
         | ANNOTATEDtpt Length Term Term
         | MATCHtpt Length Term? Term CaseDef*
         | BYNAMEtpt Term
         | EXPLICITtpt Term
```

### 6.5. Path and type references

```text
Path = Constant
     | TERMREFdirect ASTRef
     | TERMREFsymbol ASTRef Type
     | TERMREFpkg NameRef
     | TERMREF NameRef Type
     | TERMREFin Length NameRef Type Type
     | THIS Type
     | RECthis ASTRef
     | SHAREDtype ASTRef

 Type = Path
     | TYPEREFdirect ASTRef
     | TYPEREFsymbol ASTRef Type
     | TYPEREFpkg NameRef
     | TYPEREF NameRef Type
     | TYPEREFin Length NameRef Type Type
     | RECtype Type
     | SUPERtype Length Type Type
     | REFINEDtype Length NameRef Type Type
     | APPLIEDtype Length Type Type*
     | TYPEBOUNDS Length Type Type? Variance*
     | ANNOTATEDtype Length Type Term
     | ANDtype Length Type Type
     | ORtype Length Type Type
     | MATCHtype Length Type Type CaseType*
     | MATCHCASEtype Length Type Type
     | FLEXIBLEtype Length Type
     | BIND Length NameRef Type Modifier*
     | BYNAMEtype Type
     | PARAMtype Length ASTRef Nat
     | POLYtype Length Type TypeName*
     | METHODtype Length Type TypeName* Modifier*
     | TYPELAMBDAtype Length Type TypeName*
```

Annotation payloads use the following length-delimited grammar:

```text
Annotation = ANNOTATION Length Type Term
```

`ASTRef` is a byte position in the AST payload:

```text
ASTRef = Nat
```

`TERMREFdirect` and `TYPEREFdirect` point to a local symbol, normally its definition node. `SHAREDterm` and `SHAREDtype` refer to previously serialized trees.

The Rust decoder keeps the numeric value as `TermValue::AstRef` and exposes
`AstRef { kind, address }` through `SimpleTerm::ast_ref()` and
`RawTree::ast_ref()`. This distinguishes `SHAREDterm`, `SHAREDtype`,
`TERMREFdirect`, `TYPEREFdirect`, and `RECthis` without changing the raw
representation. `RawTree::ast_refs()` and `RawTree::visit_ast_refs()` collect
or visit references through category-3 and category-4 wrappers in source
order. Category-5 payloads remain opaque at this generic term layer because
their child layout is tag-specific; structured AST decoders are the next layer
for traversing those payloads.

`SimpleTerm::name_ref()` and `RawTree::name_refs()` provide the analogous
access to name-table references, including those nested in category-3 and
category-4 wrappers. For category-4 trees, the traversal reports the direct
name field of `IDENT`, `SELECT`, `SELFDEF`, and `NAMEDARG` forms, while AST
reference fields remain AST references. Category-5 payloads remain opaque at
this generic layer.

When a caller already knows the absolute start of a bounded payload,
`RawTree::decode_with_base_offset()` and
`RawTree::decode_with_max_depth_and_base_offset()` preserve that base in every
decoded tree offset. This is the low-level primitive used by the future global
nested-node index. `RawTree::nodes()` and `RawTree::visit_nodes()` expose those
visible `(tag, offset)` pairs in wire order, stopping at category-5 payload
boundaries.

Length-delimited nodes can be routed through `RawNode::decode_structured()`;
it returns a `StructuredNode` variant for supported node grammars and a `Raw`
variant for unknown category-5 tags.

At file level, `TastyFile::structured_asts()` applies this dispatch to every
top-level node while retaining the original borrowed payloads.

`RawNode::ast_refs()` and `RawNode::visit_ast_refs()` walk the child trees of
all currently supported structured category-5 nodes, including nested package
and template statements, as well as `ANNOTATION` payloads. Unknown category-5
nodes remain opaque and produce no references from this convenience API.

`RawTree::decode_structured()` provides the corresponding one-tree dispatcher.
It recognizes typed constants, category-3 AST children, `CLASSconst`, the
category-4 identifier/select/reference/self-definition/named-argument forms,
and known category-5 payloads through `StructuredTree`.

`StructuredTree::encode()` re-encodes each of those semantic tree variants.
Category-1/2 constants use their canonical tags, while leaves, wrappers, and
bounded nodes retain the tags represented by their typed payloads.

Structured definition bodies retain their leading `NameRef`. `DefinitionBody`
and `DefDefBody` expose it through `name()` and can be re-encoded with
`encode_self()`, so structural decoding no longer loses the definition name.
`DefDefBody::header_items` additionally retains the wire order of parameter
nodes and `EMPTYCLAUSE`/`SPLITCLAUSE` markers, which is required for lossless
round-trips when clause markers occur between parameter groups.

At file level, `TastyFile::ast_references()` returns the collected edges with
the owning top-level AST address and the typed target `AstRef`.
`TastyFile::ast_address_index()` indexes every visible AST node, including
category-1 through category-4 tree nodes and nested category-5 nodes.
It uses `DEFAULT_MAX_AST_INDEX_DEPTH` as a safety bound; callers handling
untrusted or unusually deep input can use
`TastyFile::ast_address_index_with_max_depth()` to choose a stricter or more
permissive limit. Exceeding the limit returns `AstError::RecursionLimit`.
`AstAddressIndex::resolve_node()` and
`TastyFile::resolve_ast_reference()` resolve a target address to its tag and
absolute AST-section offset. The index keeps visible nodes sorted by absolute
address, so `get()` and `get_node()` use logarithmic address lookup.
`AstAddressIndex::iter()` visits indexed category-5 payloads, while
`iter_nodes()` visits all visible `(tag, offset)` pairs.
`AstAddressIndex::get()` remains the payload lookup for category-5 nodes.

`RawNodes::address_index()` and `TastyFile::ast_at()` index top-level
category-5 nodes. The file-level global index preserves absolute AST-section
offsets while traversing supported structured payloads. Unknown or
context-dependent category-5 payloads remain indexed as opaque boundaries.
`RawNodes::from_entries()` constructs a validated top-level node list for
programmatic encoding; `encode_with_addresses()` then assigns fresh offsets
from the emitted stream.
`RawNode::new()` is the corresponding validated constructor for an individual
category-5 node.
`SimpleTerm::new()`, `RawTree::leaf()`, `RawTree::ast()`, and
`RawTree::nat_ast()` provide the corresponding validated constructors for
programmatically building category-1 through category-4 trees. Their offsets
start at zero and are derived from the emitted stream only when decoding or
allocating AST addresses.
`TastyFile::validate_ast_references()` checks the AST-section range, while
`TastyFile::validate_ast_reference_targets()` additionally requires every
collected reference to resolve to a visible node start.
The eager `TastyFile::validate()` path uses the stricter target validation and
the default index depth. `validate_with_max_ast_index_depth()` and
`validate_ast_reference_targets_with_max_depth()` expose the same checks with
an explicit nesting limit.
`AstAddressIndex::iter_nodes_with_tag()` filters the global visible-node index
by wire tag while retaining absolute address order. `TastyFile::ast_nodes_with_tag()`
is the file-level convenience method; it includes nested nodes and returns an
empty result for an unknown or absent tag. Its
`ast_nodes_with_tag_with_max_depth()` variant exposes the same recursion limit
as the underlying global index. These queries are structural and do not assign
Scala semantic meaning to a tag.
`AstAddressIndex::iter_nodes_in_address_range()` and
`TastyFile::ast_nodes_in_address_range()` provide the analogous half-open
address-range query `[start, end)`. The file-level
`ast_nodes_in_address_range_with_max_depth()` variant exposes the same
configurable traversal limit as the global index.
The deep index also records structural `AstTreeEdge` values. Use
`AstAddressIndex::parent_of()` for a direct parent lookup,
`AstAddressIndex::children_of()` for direct children in payload order, or
`AstAddressIndex::iter_tree_edges()` to inspect the complete traversal. The
file-level `TastyFile::ast_parent_of()`, `ast_children_of()`, and
`ast_tree_edges()` methods expose the same view. Shallow indexes created with
`RawNodes::address_index()` intentionally contain no edges, because they do
not decode enclosing tree grammars. The file-level navigation helpers also
provide `_with_max_depth()` variants, which apply the same recursion safety
bound as `TastyFile::ast_address_index_with_max_depth()`.
`TastyFile::ast_references_from()` and `TastyFile::ast_references_to()` filter
the collected AST reference graph by owner or target address while preserving
wire order. These APIs expose structural edges only; resolving a reference to
a visible node or Scala symbol remains a separate operation.

`RawNode::name_refs()` and `RawNode::visit_name_refs()` expose the
name-table references contained in supported structured payloads. At file
level, `TastyFile::name_references()` returns the same references together
with the owning top-level AST address, which lets callers resolve them through
the file's `NameTable`.
`TastyFile::validate_name_references()` applies this check eagerly, and the
general `TastyFile::validate()` path includes it.
`TastyFile::name_references_from()` and `TastyFile::name_references_to()`
filter those collected name-table edges by their owning AST address or target
`NameRef`, again without assigning semantic meaning to the referenced name.

`NameTable::get_utf8()` and `RawName::as_utf8()` provide a checked shortcut
for direct UTF-8 entries. `NameTable::find_utf8()` provides the inverse lookup
for the first direct entry with a given string. Composite names deliberately
remain structured `RawName` values rather than being flattened with an
assumed separator.
`RawName::kind()` provides a payload-independent [`RawNameKind`] classification
for all known and unknown entry variants; the original `RawName` remains
available for inspecting payloads and preserving lossless data.
`NameTable::render()` resolves the conventional textual spelling of
non-signature composite names iteratively. Signature-bearing and unknown names
return `NameRenderError::Unsupported`. Signature-bearing names can instead be
inspected through `NameTable::render_signed_name()`, which preserves their
structured fields and uses an iterative structural diagnostic form for nested
signature references.
`TastyFile::render_name()` and `TastyFile::render_signed_name()` provide the
same operations directly against a parsed file's name table, without requiring
callers to extract that table first.
`NameTable::iter()` visits entries in wire order and pairs each borrowed raw
entry with its one-based `NameRef`, which is safer for callers than deriving
references from zero-based slice indexes.
`RawName::visit_references()` exposes their dependency edges in wire order
without requiring a temporary allocation.
`NameTable::dependency_order()` follows those edges transitively and returns
each reachable `NameRef` once in dependency-first order. It is an iterative,
structural traversal: it does not flatten composite names or assume a textual
separator, and returns `None` for an unknown root reference.

`BIND` is context-dependent: in a pattern it carries a pattern tree, while in
a type it carries zero or more modifiers. `BindNode` exposes these alternatives
as `BindBody::Pattern` and `BindBody::Type`; bytes after an undecidable pattern
body are preserved in `BindNode::remainder` for lossless re-encoding.

`MATCHtpt` has an optional bound followed by a required selector. The decoder
normalizes the one-tree form to `bound = None` and the two-tree form to
`bound = Some(...)`, while retaining all case definitions in wire order.

`SPLICEPATTERN` stores its type and term arguments as one ordered tail without
an encoded split point. `SplicePatternNode::split_arguments(type_argument_count)`
lets a typed caller apply that context without changing the lossless raw model;
an out-of-range count returns `None`.

`METHODtype` likewise has an undelimited `TypeName* Modifier*` suffix. The
heuristic decoder remains available for ordinary payloads, while
`decode_method_type_with_type_name_count(count)` lets typed callers decode the
suffix unambiguously when a `TypeName` starts with a modifier-valued byte.

`HOLE` is a valid special category-5 node. Its typed representation contains
the hole index, its type tree, and the remaining ordered argument trees.

### 6.6. Constants

```text
UNITconst
FALSEconst
TRUEconst
NULLconst

BYTEconst   Int
SHORTconst  Int
CHARconst   Nat
INTconst    Int
LONGconst   LongInt
FLOATconst  Int
DOUBLEconst LongInt
STRINGconst NameRef
CLASSconst  Type
```

`FLOATconst` and `DOUBLEconst` carry integer representations of the floating-point bits rather than textual number representations.

The Rust term layer exposes these tags through `SimpleTerm::constant_value()`.
It returns a typed `ConstantValue`, validates the ranges of `BYTEconst` and
`SHORTconst`, and rejects `CHARconst` values that do not fit in Scala's
16-bit `Char` representation.
`FloatBits` and `DoubleBits` retain the exact wire bits so NaNs and signed
zeroes survive a decode/encode cycle. `ConstantValue::encode()` writes the
canonical tag and payload for each supported constant.

`CLASSconst` is represented separately as `ClassConstNode { type_tree }` via
`RawTree::decode_class_constant()`. The older generic
`RawTree::decode_class_const()` API remains available for callers that need
the category-3 wrapper itself.

## 7. AST tag categories

A tag alone is not enough to determine the payload without knowing its category.

| Category | Range | Payload |
|---:|---:|---|
| 1 | `1..59` | tag only |
| 2 | `60..89` | `tag Nat` |
| 3 | `90..109` | `tag AST` |
| 4 | `110..127` | `tag Nat AST` |
| 5 | `128..255` | `tag Length payload` |

In category 1, many tags are modifiers, such as `PRIVATE`, `FINAL`, `INLINE`, `GIVEN`, `OPAQUE`, `MUTABLE`, `COVARIANT`, and `CONTRAVARIANT`. They are interpreted only in grammar productions that allow `Modifier*`.

### 7.1. Complete tag list

#### Category 1: tag only

```text
2  UNITconst          3  FALSEconst        4  TRUEconst
5  NULLconst          6  PRIVATE           8  PROTECTED
9  ABSTRACT          10  FINAL             11 SEALED
12 CASE              13 IMPLICIT           14 LAZY
15 OVERRIDE          16 INLINEPROXY       17 INLINE
18 STATIC            19 OBJECT            20 TRAIT
21 ENUM              22 LOCAL             23 SYNTHETIC
24 ARTIFACT          25 MUTABLE           26 FIELDaccessor
27 CASEaccessor      28 COVARIANT         29 CONTRAVARIANT
31 HASDEFAULT        32 STABLE            33 MACRO
34 ERASED            35 OPAQUE            36 EXTENSION
37 GIVEN             38 PARAMsetter       39 EXPORTED
40 OPEN              41 PARAMalias        42 TRANSPARENT
43 INFIX             44 INVISIBLE         45 EMPTYCLAUSE
46 SPLITCLAUSE       47 TRACKED           48 SUBMATCH
49 INTO
```

Unassigned values in this range include `1`, `7`, and `30`.

#### Category 2: `tag Nat`

```text
60 SHAREDterm        61 SHAREDtype        62 TERMREFdirect
63 TYPEREFdirect     64 TERMREFpkg        65 TYPEREFpkg
66 RECthis           67 BYTEconst         68 SHORTconst
69 CHARconst         70 INTconst          71 LONGconst
72 FLOATconst        73 DOUBLEconst       74 STRINGconst
75 IMPORTED          76 RENAMED
```

#### Category 3: `tag AST`

```text
90 THIS              91 QUALTHIS          92 CLASSconst
93 BYNAMEtype        94 BYNAMEtpt         95 NEW
96 THROW             97 IMPLICITarg       98 PRIVATEqualified
99 PROTECTEDqualified 100 RECtype          101 SINGLETONtpt
102 BOUNDED           103 EXPLICITtpt       104 ELIDED
```

#### Category 4: `tag Nat AST`

```text
110 IDENT             111 IDENTtpt          112 SELECT
113 SELECTtpt         114 TERMREFsymbol     115 TERMREF
116 TYPEREFsymbol     117 TYPEREF           118 SELFDEF
119 NAMEDARG
```

#### Category 5: `tag Length payload`

```text
128 PACKAGE            129 VALDEF             130 DEFDEF
131 TYPEDEF            132 IMPORT             133 TYPEPARAM
134 PARAM              136 APPLY              137 TYPEAPPLY
138 TYPED              139 ASSIGN              140 BLOCK
141 IF                 142 LAMBDA              143 MATCH
144 RETURN             145 WHILE              146 TRY
147 INLINED            148 SELECTouter        149 REPEATED
150 BIND               151 ALTERNATIVE        152 UNAPPLY
153 ANNOTATEDtype      154 ANNOTATEDtpt       155 CASEDEF
156 TEMPLATE           157 SUPER              158 SUPERtype
159 REFINEDtype        160 REFINEDtpt         161 APPLIEDtype
162 APPLIEDtpt         163 TYPEBOUNDS         164 TYPEBOUNDStpt
165 ANDtype            167 ORtype             169 POLYtype
170 TYPELAMBDAtype     171 LAMBDAtpt           172 PARAMtype
173 ANNOTATION         174 TERMREFin          175 TYPEREFin
176 SELECTin           177 EXPORT             178 QUOTE
179 SPLICE             180 METHODtype         181 APPLYsigpoly
182 QUOTEPATTERN       183 SPLICEPATTERN      190 MATCHtype
191 MATCHtpt           192 MATCHCASEtype      193 FLEXIBLEtype
255 HOLE
```

Unassigned values in category 5 must not automatically be treated as valid nodes. `HOLE` is a valid special tag.

## 8. Modifiers

A modifier is usually a single category-1 tag. Modifiers carrying a payload are:

```text
PRIVATEqualified   Type
PROTECTEDqualified Type
```

The remaining modifiers have no payload. `Variance` is:

```text
Variance = STABLE | COVARIANT | CONTRAVARIANT
```

`STABLE` means invariant variance in a `Variance` context.

## 9. Source positions

The `Positions` section has the following shape:

```text
LinesSizes = Nat Nat*
Assoc      = Header Delta? Delta? Delta?
           | SOURCE NameRef
Delta      = Int
```

`LinesSizes` contains the number of lines followed by the size of each line excluding the trailing `\n`.

Each position entry starts with one signed `Int` header. The `SOURCE` entry is
encoded as the reserved header value `SOURCE = 4`, followed by the source
name reference encoded as an `Int`. Association headers encode the following
values:

```text
addrDelta << 3
hasStartDiff << 2
hasEndDiff << 1
hasPoint
```

The flags indicate whether the corresponding deltas are present. Positive and negative deltas are differences relative to the previously recorded position. Nodes with the same position as their parent may be omitted.

All position headers and deltas are serialized as `Int`; line counts and line
sizes remain `Nat` values. The implementation must preserve this distinction
together with the section boundary.

`EncodedSection::attributes()`, `EncodedSection::comments()`, and
`EncodedSection::positions()` own typed section payloads and expose a borrowed
`Section` view for assembling a `SectionTable`. `EncodedSection::raw()` covers
unknown or application-specific sections without requiring a guessed grammar.
`PositionSection::resolved_entries()` provides an ordered derived view that
accumulates association deltas into absolute coordinates while retaining
`SOURCE` events. The raw `PositionEntry` deltas remain authoritative for
lossless re-encoding.
`PositionSection::resolved_associations()` provides a smaller derived view
that omits source-change events and attaches the currently active source
`NameRef`—or `None` before the first source event—to each association. Use
`resolved_entries()` when source-event ordering itself is significant.
`TastyFile::resolved_position_associations()` exposes the same view directly
from a parsed file and returns `None` when the file has no `Positions` section.
`PositionSection::resolved_association_at()` and
`TastyFile::resolved_position_at()` look up the first association for an
absolute AST address while preserving the resolved source and coordinates.
`EncodedSection::asts()` and `EncodedSection::structured_asts()` provide the
same ownership boundary for raw or structured top-level AST nodes.
The corresponding `*_with_addresses()` constructors return an
`EncodedAstSection` containing both the owned payload and the allocated
top-level node addresses.
`TastyFileBuilder` owns these encoded sections and produces either a complete
byte stream or an `EncodedTastyFile` with the allocated AST addresses.
Its `validate()` and `validate_scala_3_9()` methods allow checking the complete
owned file before serialization; corresponding
`*_with_max_ast_index_depth` methods allow an explicit AST nesting limit.
For callers that require an eager safety boundary, `TastyFile::encode_validated()`
and `TastyFile::encode_validated_with_ast_addresses()` validate AST references,
name references, and supported standard sections before writing; the builder
exposes the same two methods. Their
`*_with_max_ast_index_depth` variants apply an explicit AST nesting limit
atomically during validation and encoding.

## 10. Comments

The `Comments` section contains entries of the form:

```text
Comment = Utf8 LongInt
```

The comment text is encoded as UTF-8, and the final element contains the
comment coordinates. Comments do not carry AST addresses in the TASTy wire
format.

## 11. Attributes

```text
Attribute = SCALA2STANDARDLIBRARY
          | EXPLICITNULLS
          | CAPTURECHECKED
          | WITHPUREFUNS
          | JAVA
          | OUTLINE
          | SOURCEFILE Utf8Ref
```

Defined tags:

| Value | Tag |
|---:|---|
| 1 | `SCALA2STANDARDLIBRARYattr` |
| 2 | `EXPLICITNULLSattr` |
| 3 | `CAPTURECHECKEDattr` |
| 4 | `WITHPUREFUNSattr` |
| 5 | `JAVAattr` |
| 6 | `OUTLINEattr` |
| 129 | `SOURCEFILEattr` |

Attributes:

- must not be repeated;
- must be ordered by tag number;
- have no additional payload in range `1..32`;
- have a `Utf8Ref` payload in range `129..160`.

The Scala 3.9.0 compiler emits the `SOURCEFILEattr` `Utf8Ref` as a zero-based
name-table index in the fixtures used by this project, despite the grammar's
one-based `Utf8Ref` description. A decoder should preserve the raw value and
apply the zero-based interpretation when reading compiler-emitted files.

## 12. `numRefs` and tree structure calculation

The format defines a helper function `numRefs(tag)`. It indicates how many references occur at the beginning of an entry, or — for a negative value — how many initial elements are not references.

```text
numRefs = 1 for:
  VALDEF DEFDEF TYPEDEF TYPEPARAM PARAM NAMEDARG RETURN BIND
  SELFDEF REFINEDtype TERMREFin TYPEREFin SELECTin HOLE

numRefs = 2 for:
  RENAMED PARAMtype

numRefs = -1 for:
  POLYtype TYPELAMBDAtype METHODtype

numRefs = 0 for all other tags
```

This information is primarily needed for generic size calculation and node indexing. It does not replace the grammar of an individual node.

## 13. Recommended Rust representation

### 13.1. Binary layer

```rust
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    limit: usize,
}
```

Basic operations:

```text
read_u8()
read_nat()
read_int()
read_long_int()
read_utf8()
read_name_ref()
read_ast_ref()
read_length()
sub_reader(start, end)
```

Every `Length` parser should operate on a reader bounded to the payload and verify that the entire length was consumed.

### 13.2. Tag namespaces

Define separate types:

```text
NameTag
TreeTag
AttributeTag
PositionTag
```

Do not define one global `u8 -> Tag` mapping because values overlap between contexts:

```text
62 = TARGETSIGNED in NameTable
62 = TERMREFdirect in AST
63 = SIGNED in NameTable
63 = TYPEREFdirect in AST
```

### 13.3. Raw-node layer

The first AST model can preserve the wire format without resolving symbols:

```text
RawNode {
    tag: TreeTag,
    start: AstRef,
    end: AstRef,
    payload: RawPayload,
}
```

The payload should preserve `NameRef`, `ASTRef`, child nodes, and — optionally — raw bytes for unknown nodes. A later layer can resolve names and symbols.

## 14. Validation and parser robustness

The decoder should validate:

1. the magic header;
2. version and length overflow;
3. the bounds of every section and `Length` node;
4. UTF-8 validity;
5. `NameRef` ranges;
6. `ASTRef` ranges;
7. tag legality in the current context;
8. that no payload bytes remain after a node parser finishes;
9. maximum recursion depth or an equivalent protection limit. The Rust raw-tree
   decoder uses `DEFAULT_MAX_TREE_DEPTH` by default and exposes
   `decode_with_max_depth` for callers that need a different bound. The global
   AST index uses `DEFAULT_MAX_AST_INDEX_DEPTH` and exposes
   `ast_address_index_with_max_depth` for the same purpose;
10. the absence of infinite loops in base-128 numbers and boundary-terminated lists.

Unknown sections can be skipped. Unknown category-5 nodes are preserved as raw
payloads because they carry a length. Unknown category-1–4 nodes are harder to
skip because they have no generic length.

The Rust file model keeps section decoding lazy: `parse_scala_3_9` validates the
container header and section boundaries, while `TastyFile::validate()` eagerly
decodes the ASTs, checks the byte range of their AST references, and decodes
all supported standard sections. Applications that want the eager behavior at
construction time can use `parse_and_validate_scala_3_9`. The
`*_with_max_ast_index_depth` variants apply an explicit AST nesting limit while
performing that eager validation.

## 15. Implementation plan

The first five stages below are implemented for the Scala 3.9.0 compatibility
target. The checklist records the current boundary of this crate: it provides
lossless wire and structural APIs, while a Scala semantic model remains a
separate future layer.

### Stage 1: container — complete

- `Reader` and `sub_reader`;
- `Nat`, `Int`, and `LongInt`;
- UTF-8;
- header and version compatibility;
- bounded sections.

### Stage 2: name table — complete

- `UTF8` entries;
- composite names;
- `NameRef` and `ParamSig`;
- typed signature views and iterative rendering;
- tests for cyclic or invalid references.

### Stage 3: structural AST decoder — complete

- dispatch by tag category;
- raw nodes;
- `ASTRef` addressing;
- `SHAREDterm` and `SHAREDtype`;
- the complete 3.9.0 tag set.

### Stage 4: additional sections — complete

- `Positions`;
- `Comments`;
- `Attributes`.

### Stage 5: encoder — complete

- deterministic name-table construction;
- payload-length calculation;
- AST-address allocation;
- round-trip tests against files generated by Scala 3.9.0.

### Stage 6: structural tooling — in progress

- file-level validation and reference collection;
- global AST address indexing and tag queries;
- resolved position entries and source changes;
- file-level name rendering and one-based name-table iteration;
- focused unit tests plus fixture-wide integration coverage;
- next: ordering-preserving navigation helpers for nested AST structure and
  source spans.

### Stage 7: semantic model — separate scope

- resolve names, symbols, types, and owners into a Scala-facing semantic API;
- keep this layer separate from the TASTy wire/structural crate;
- define explicit compatibility and fallback behavior for compiler evolution.

## 16. Points to confirm with Scala-generated fixtures

Before declaring the implementation compatible, confirm the following using real `.tasty` files:

- the exact length unit used by `Utf8`;
- the exact interpretation of `nameTable_Length`;
- minimal signed and unsigned integer encodings;
- UUID semantics in generated files;
- recognition rules for optional lists and `Modifier*`;
- behavior for unknown tags and trailing bytes;
- the relation between `ASTRef` and the beginning of the `ASTs` section;
- `SIGNED`/`TARGETSIGNED` behavior for actual overloaded-method signatures.

The fixture suite should also round-trip every top-level AST through the
structured encoder once its corresponding semantic node is supported. Raw
fallback nodes may remain opaque, but their bytes must still be preserved.
