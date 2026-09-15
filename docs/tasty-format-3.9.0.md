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

At file level, `TastyFile::ast_references()` returns the collected edges with
the owning top-level AST address and the typed target `AstRef`.
`TastyFile::ast_address_index()` indexes every visible AST node, including
category-1 through category-4 tree nodes and nested category-5 nodes.
`AstAddressIndex::resolve_node()` and
`TastyFile::resolve_ast_reference()` resolve a target address to its tag and
absolute AST-section offset. `AstAddressIndex::get()` remains the payload
lookup for category-5 nodes.

`RawNodes::address_index()` and `TastyFile::ast_at()` index top-level
category-5 nodes. The file-level global index preserves absolute AST-section
offsets while traversing supported structured payloads. Unknown or
context-dependent category-5 payloads remain indexed as opaque boundaries.
`TastyFile::validate_ast_references()` checks the AST-section range, while
`TastyFile::validate_ast_reference_targets()` additionally requires every
collected reference to resolve to a visible node start.
The eager `TastyFile::validate()` path uses the stricter target validation.

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

## 10. Comments

The `Comments` section contains entries of the form:

```text
Comment = ASTRef Utf8 LongInt
```

The first element is the address of the AST node to which the comment is
attached. The comment text is encoded as UTF-8, and the final element contains
the comment coordinates.

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
   `decode_with_max_depth` for callers that need a different bound;
10. the absence of infinite loops in base-128 numbers and boundary-terminated lists.

Unknown sections can be skipped. Unknown category-5 nodes are preserved as raw
payloads because they carry a length. Unknown category-1–4 nodes are harder to
skip because they have no generic length.

The Rust file model keeps section decoding lazy: `parse_scala_3_9` validates the
container header and section boundaries, while `TastyFile::validate()` eagerly
decodes the ASTs, checks the byte range of their AST references, and decodes
all supported standard sections. Applications that want the eager behavior at
construction time can use `parse_and_validate_scala_3_9`.

## 15. Implementation plan

### Stage 1: container

- `Reader` and `sub_reader`;
- `Nat`, `Int`, and `LongInt`;
- UTF-8;
- header and version compatibility;
- bounded sections.

### Stage 2: name table

- `UTF8` entries;
- composite names;
- `NameRef` and `ParamSig`;
- tests for cyclic or invalid references.

### Stage 3: structural AST decoder

- dispatch by tag category;
- raw nodes;
- `ASTRef` addressing;
- `SHAREDterm` and `SHAREDtype`;
- the complete 3.9.0 tag set.

### Stage 4: additional sections

- `Positions`;
- `Comments`;
- `Attributes`.

### Stage 5: encoder

- deterministic name-table construction;
- payload-length calculation;
- AST-address allocation;
- round-trip tests against files generated by Scala 3.9.0.

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
