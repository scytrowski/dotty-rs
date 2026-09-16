# JVM Class File Format Specification (JDK 25)

Status: working document for the Rust implementation.

Normative source: [Chapter 4, "The `class` File Format", Java Virtual Machine
Specification, SE 25](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-4.html).

This document describes the wire format of `.class` files, scoped to what a
**class loader for semantic analysis** needs: enough structure to resolve
classes, supertypes, fields, methods, their descriptors/generic signatures,
and the handful of attributes that carry symbol-level information consumed by
TASTy semantic analysis (nesting, sealed hierarchies, records, generics,
source-level names). It intentionally does not cover:

- bytecode execution or verification (Chapters 4.9–4.11 of the JVMS);
- the module system (`module-info.class`, `ACC_MODULE`, `Module*`
  attributes) — out of scope until the compiler needs module resolution;
- annotation *processing* beyond being able to skip or losslessly retain
  annotation attributes;
- `invokedynamic`/`Dynamic` constant resolution semantics (`BootstrapMethods`
  is parsed structurally so the class file can be traversed, not to resolve
  call sites).

Where this document is silent on a validation rule, the JVMS is authoritative;
this file is a working reference for the Rust decoder, not a replacement for
the spec.

## 1. Format model

A class file consists of:

```text
magic
minor_version
major_version
constant_pool
access_flags
this_class
super_class
interfaces
fields
methods
attributes
```

All multi-byte numeric values are big-endian, unsigned, and fixed-width —
unlike TASTy's base-128 varints, there is no variable-length integer encoding
anywhere in the class file format. The primitive widths used throughout the
spec (and this document) are:

```text
u1 = 1 byte
u2 = 2 bytes
u4 = 4 bytes
```

Every structure is a fixed sequence of `u1`/`u2`/`u4` fields, `count`-prefixed
arrays of a repeated sub-structure, or (for `Utf8` and attribute payloads) a
length-prefixed byte blob. There are no optional fields encoded by a
discriminant tag the way TASTy nodes are; instead, **attributes** are a
generic, named, length-delimited extension mechanism (§7), and unrecognized
attributes are always safely skippable via their declared length.

## 2. Header and version

```text
ClassFile {
    u4 magic;              // 0xCAFEBABE
    u2 minor_version;
    u2 major_version;
    ...
}
```

`magic` must equal `0xCAFEBABE`.

### 2.1. Version numbers

`major_version` follows the scheme `44 + JDK release number` for releases
that introduced one major version per JDK (JDK 1.2 = 46, ..., JDK 8 = 52,
..., **JDK 25 = 69**). A JVM that implements version `M.m` supports class
file formats in the inclusive range `45.0` up to `M.0` or `M.65535`.

Minor version rules:

- for `major_version >= 56` (JDK 12+): `minor_version` must be `0` or
  `65535`; `65535` marks the class as compiled against **preview features**
  of exactly that major version and must be rejected unless preview features
  are enabled for a matching major version;
- for `45 <= major_version <= 55`: `minor_version` may be any value
  (historically used by JDK 1.0.x/1.1.x).

For this project's compatibility target (JDK 25 / Scala 3.9.0 output), the
expected pair is `major_version = 69`, `minor_version` in `{0, 65535}`. As
with the TASTy header, a loader should expose both a strict check (accept
only the target version) and a compatible-range check (accept the
historically valid range and preserve the observed version), since
dependency class files may legitimately be compiled for older JDKs.

## 3. Constant pool

```text
u2      constant_pool_count;
cp_info constant_pool[constant_pool_count - 1];
```

The constant pool is the only variable-shape, tag-dispatched table in the
format — structurally the closest analogue to TASTy's name table. Key rules:

- entries are numbered from **1**; index `0` is never valid;
- valid indices are `1 .. constant_pool_count - 1`;
- `CONSTANT_Long_info` and `CONSTANT_Double_info` each occupy **two**
  consecutive constant pool slots: an entry at index `n` makes `n + 1`
  unusable, and the next real entry starts at `n + 2`. A validating reader
  must special-case this while building an index → entry map.

Every entry begins with a one-byte tag:

```text
cp_info {
    u1 tag;
    u1 info[];   // tag-dependent
}
```

| Tag | Name | Since | Layout (after `tag`) |
|---:|---|---|---|
| 1 | `CONSTANT_Utf8` | 45.3 | `u2 length; u1 bytes[length]` — modified UTF-8 |
| 3 | `CONSTANT_Integer` | 45.3 | `u4 bytes` (big-endian `i32`) |
| 4 | `CONSTANT_Float` | 45.3 | `u4 bytes` (IEEE 754 binary32) |
| 5 | `CONSTANT_Long` | 45.3 | `u4 high_bytes; u4 low_bytes` (occupies 2 slots) |
| 6 | `CONSTANT_Double` | 45.3 | `u4 high_bytes; u4 low_bytes` (occupies 2 slots) |
| 7 | `CONSTANT_Class` | 45.3 | `u2 name_index` → `Utf8` (binary class/interface name, or array descriptor) |
| 8 | `CONSTANT_String` | 45.3 | `u2 string_index` → `Utf8` |
| 9 | `CONSTANT_Fieldref` | 45.3 | `u2 class_index` → `Class`; `u2 name_and_type_index` → `NameAndType` (field descriptor) |
| 10 | `CONSTANT_Methodref` | 45.3 | `u2 class_index` → `Class`; `u2 name_and_type_index` → `NameAndType` (method descriptor) |
| 11 | `CONSTANT_InterfaceMethodref` | 45.3 | same layout as `Methodref`, class must be an interface |
| 12 | `CONSTANT_NameAndType` | 45.3 | `u2 name_index` → `Utf8`; `u2 descriptor_index` → `Utf8` |
| 15 | `CONSTANT_MethodHandle` | 51.0 (Java 7) | `u1 reference_kind` (1–9); `u2 reference_index` |
| 16 | `CONSTANT_MethodType` | 51.0 (Java 7) | `u2 descriptor_index` → `Utf8` (method descriptor) |
| 17 | `CONSTANT_Dynamic` | 55.0 (Java 11) | `u2 bootstrap_method_attr_index`; `u2 name_and_type_index` (field descriptor) |
| 18 | `CONSTANT_InvokeDynamic` | 51.0 (Java 7) | `u2 bootstrap_method_attr_index`; `u2 name_and_type_index` (method descriptor) |
| 19 | `CONSTANT_Module` | 53.0 (Java 9) | `u2 name_index` → `Utf8`; module-only |
| 20 | `CONSTANT_Package` | 53.0 (Java 9) | `u2 name_index` → `Utf8`; module-only |

`CONSTANT_Utf8` uses *modified* UTF-8: the NUL byte and bytes in
`0xf0..=0xff` never appear; code points above `U+FFFF` are encoded as a pair
of 3-byte surrogate sequences rather than the standard 4-byte UTF-8 form. A
decoder must not treat this as plain UTF-8 without translation.

`CONSTANT_MethodHandle.reference_kind`:

| Value | Name | Target constant pool entry |
|---:|---|---|
| 1 | `REF_getField` | `Fieldref` |
| 2 | `REF_getStatic` | `Fieldref` |
| 3 | `REF_putField` | `Fieldref` |
| 4 | `REF_putStatic` | `Fieldref` |
| 5 | `REF_invokeVirtual` | `Methodref` |
| 6 | `REF_invokeStatic` | `Methodref` or `InterfaceMethodref` (class file ≥ 52.0) |
| 7 | `REF_invokeSpecial` | `Methodref` or `InterfaceMethodref` (class file ≥ 52.0) |
| 8 | `REF_newInvokeSpecial` | `Methodref`, name must be `<init>` |
| 9 | `REF_invokeInterface` | `InterfaceMethodref` |

For this crate's purposes, `MethodHandle`/`MethodType`/`Dynamic`/
`InvokeDynamic`/`Module`/`Package` entries mainly need to be **decoded
structurally and preserved** so the constant pool round-trips and other
entries can be resolved by index; they are not expected to be interpreted
semantically in the initial loader.

## 4. Class structure

```text
u2              access_flags;
u2              this_class;      // constant_pool index -> CONSTANT_Class_info
u2              super_class;     // constant_pool index -> CONSTANT_Class_info, or 0 for java.lang.Object
u2              interfaces_count;
u2              interfaces[interfaces_count];  // constant_pool indices -> CONSTANT_Class_info
u2              fields_count;
field_info      fields[fields_count];
u2              methods_count;
method_info     methods[methods_count];
u2              attributes_count;
attribute_info  attributes[attributes_count];
```

- `this_class` is always a valid, non-zero index.
- `super_class` is `0` only for `java.lang.Object`; the referenced class must
  not have `ACC_FINAL` set.
- `interfaces` lists direct superinterfaces in declaration order.

### 4.1. Class access flags

| Flag | Value | Meaning |
|---|---:|---|
| `ACC_PUBLIC` | `0x0001` | public visibility |
| `ACC_FINAL` | `0x0010` | no subclasses permitted |
| `ACC_SUPER` | `0x0020` | historical; JVMs treat this as always set since Java SE 8 |
| `ACC_INTERFACE` | `0x0200` | is an interface |
| `ACC_ABSTRACT` | `0x0400` | must not be instantiated |
| `ACC_SYNTHETIC` | `0x1000` | not present in source |
| `ACC_ANNOTATION` | `0x2000` | is an annotation interface |
| `ACC_ENUM` | `0x4000` | is (or is a member of) an `enum` class |
| `ACC_MODULE` | `0x8000` | is `module-info`, not a class/interface |

Rules: `ACC_INTERFACE` implies `ACC_ABSTRACT` and excludes `ACC_FINAL`,
`ACC_SUPER`, `ACC_ENUM`, `ACC_MODULE`. A non-interface class must not set
`ACC_ANNOTATION` or `ACC_MODULE`, and must not set both `ACC_FINAL` and
`ACC_ABSTRACT`. If `ACC_MODULE` is set, no other flag may be set and the
class body must take the constrained `module-info` shape (§9) — out of scope
for this loader for now.

`ACC_ENUM` and `ACC_ANNOTATION`/`ACC_INTERFACE` are the structural signals a
semantic loader needs to reconstruct a class's Java-level *kind* (class,
interface, annotation interface, enum, record — records are additionally
signaled by the `Record` attribute, §7.9); Scala 3 maps its own `sealed`/
`enum`/`case class` concepts onto combinations of these flags plus
`PermittedSubclasses` and synthesized methods, not onto a single flag.

## 5. Fields

```text
field_info {
    u2             access_flags;
    u2             name_index;         // Utf8, unqualified field name
    u2             descriptor_index;   // Utf8, field descriptor (§6.1)
    u2             attributes_count;
    attribute_info attributes[attributes_count];
}
```

Field access flags:

| Flag | Value | Meaning |
|---|---:|---|
| `ACC_PUBLIC` | `0x0001` | |
| `ACC_PRIVATE` | `0x0002` | |
| `ACC_PROTECTED` | `0x0004` | |
| `ACC_STATIC` | `0x0008` | |
| `ACC_FINAL` | `0x0010` | |
| `ACC_VOLATILE` | `0x0040` | |
| `ACC_TRANSIENT` | `0x0080` | |
| `ACC_SYNTHETIC` | `0x1000` | |
| `ACC_ENUM` | `0x4000` | enum constant |

At most one of `ACC_PUBLIC`/`ACC_PRIVATE`/`ACC_PROTECTED` may be set;
`ACC_FINAL` and `ACC_VOLATILE` are mutually exclusive. Interface fields must
have exactly `ACC_PUBLIC | ACC_STATIC | ACC_FINAL` (plus optionally
`ACC_SYNTHETIC`) and nothing else.

## 6. Methods

```text
method_info {
    u2             access_flags;
    u2             name_index;         // Utf8: name, or "<init>"/"<clinit>"
    u2             descriptor_index;   // Utf8, method descriptor (§6.2)
    u2             attributes_count;
    attribute_info attributes[attributes_count];
}
```

Method access flags:

| Flag | Value | Meaning |
|---|---:|---|
| `ACC_PUBLIC` | `0x0001` | |
| `ACC_PRIVATE` | `0x0002` | |
| `ACC_PROTECTED` | `0x0004` | |
| `ACC_STATIC` | `0x0008` | |
| `ACC_FINAL` | `0x0010` | |
| `ACC_SYNCHRONIZED` | `0x0020` | |
| `ACC_BRIDGE` | `0x0040` | compiler-generated bridge method |
| `ACC_VARARGS` | `0x0080` | declared with `...` |
| `ACC_NATIVE` | `0x0100` | |
| `ACC_ABSTRACT` | `0x0400` | |
| `ACC_STRICT` | `0x0800` | `strictfp`; only meaningful for class file versions 46–60 |
| `ACC_SYNTHETIC` | `0x1000` | |

Relevant rules for a loader: interface methods with class file version
`>= 52.0` require exactly one of `ACC_PUBLIC`/`ACC_PRIVATE` (default methods
are `ACC_PUBLIC` without `ACC_ABSTRACT`); `<clinit>` is only meaningful with
`ACC_STATIC` and takes no arguments; `<init>` is only ever invoked via
`invokespecial` and must have a `void` descriptor. Native and abstract
methods carry no `Code` attribute; every other method has exactly one.
`ACC_BRIDGE` is the signal to treat a method as compiler-synthesized for
generic-erasure/covariant-return purposes rather than as a source-level
overload.

### 6.1. Field descriptors

```text
FieldDescriptor  = FieldType
FieldType        = BaseType | ObjectType | ArrayType
BaseType         = 'B' | 'C' | 'D' | 'F' | 'I' | 'J' | 'S' | 'Z'
ObjectType       = 'L' ClassName ';'
ArrayType        = '[' ComponentType
ComponentType    = FieldType
```

| Symbol | Type |
|---|---|
| `B` | `byte` |
| `C` | `char` |
| `D` | `double` |
| `F` | `float` |
| `I` | `int` |
| `J` | `long` |
| `S` | `short` |
| `Z` | `boolean` |
| `L ClassName ;` | reference type, `ClassName` in internal form (§8) |
| `[ ComponentType` | array, one `[` per dimension (at most 255) |

### 6.2. Method descriptors

```text
MethodDescriptor    = '(' ParameterDescriptor* ')' ReturnDescriptor
ParameterDescriptor = FieldType
ReturnDescriptor    = FieldType | VoidDescriptor
VoidDescriptor      = 'V'
```

Example: `Object m(int i, double d, Thread t)` →
`(IDLjava/lang/Thread;)Ljava/lang/Object;`.

Constraint: the sum of parameter "units" (each category-2 type — `long`,
`double` — counts as 2, everything else as 1, plus an implicit receiver slot
for instance methods) must not exceed 255.

Descriptors carry only *erased* types — no type-parameter names or bounds.
Generic type information, when present, lives exclusively in the `Signature`
attribute (§6.3) as a parallel, richer grammar over the same descriptor
shape; a semantic loader that wants generics must parse both and treat
`Signature` as authoritative for type parameters/arguments while falling
back to the descriptor when no `Signature` attribute is present.

### 6.3. Signatures (generics)

Carried by the `Signature` attribute (§7.9) on classes, fields, methods, and
record components (class file ≥ 49.0, Java 5). Grammar (JVMS §4.7.9.1):

```text
ClassSignature       = TypeParameters? SuperclassSignature SuperinterfaceSignature*
TypeParameters       = '<' TypeParameter+ '>'
TypeParameter        = Identifier ClassBound InterfaceBound*
ClassBound           = ':' ReferenceTypeSignature?
InterfaceBound       = ':' ReferenceTypeSignature
SuperclassSignature      = ClassTypeSignature
SuperinterfaceSignature  = ClassTypeSignature

JavaTypeSignature     = ReferenceTypeSignature | BaseType
ReferenceTypeSignature = ClassTypeSignature | TypeVariableSignature | ArrayTypeSignature

ClassTypeSignature       = 'L' PackageSpecifier? SimpleClassTypeSignature ClassTypeSignatureSuffix* ';'
PackageSpecifier         = (Identifier '/')+
SimpleClassTypeSignature = Identifier TypeArguments?
ClassTypeSignatureSuffix = '.' SimpleClassTypeSignature

TypeVariableSignature = 'T' Identifier ';'

TypeArguments  = '<' TypeArgument+ '>'
TypeArgument   = WildcardIndicator? ReferenceTypeSignature | '*'
WildcardIndicator = '+' | '-'

ArrayTypeSignature = '[' JavaTypeSignature

MethodSignature = TypeParameters? '(' JavaTypeSignature* ')' Result ThrowsSignature*
Result          = JavaTypeSignature | 'V'
ThrowsSignature = '^' ClassTypeSignature | '^' TypeVariableSignature

FieldSignature  = ReferenceTypeSignature
```

Notes for the loader:

- `ClassTypeSignatureSuffix` (the `.` form) encodes a generic *inner* class
  qualified by an enclosing generic instantiation, e.g.
  `Outer<T>.Inner<U>` → `LOuter<TT;>.Inner<TU;>;`.
- `TypeArgument = '*'` is the unbounded wildcard `?`; `+`/`-` are the
  upper/lower bound wildcard forms (`? extends`/`? super`).
- A missing `ClassBound` type (`:` immediately followed by another `:` or
  the parameter's end) means the bound is implicitly `Object`.
- `Result` reuses `V` for `void`, matching `VoidDescriptor` in plain
  descriptors.

## 7. Attributes

### 7.1. General shape

```text
attribute_info {
    u2 attribute_name_index;   // Utf8, attribute name
    u4 attribute_length;       // byte length of info[]
    u1 info[attribute_length];
}
```

`attribute_name_index` is resolved through the constant pool like any other
name; there is no fixed numeric tag namespace for attributes (unlike TASTy's
byte-tagged AST/section/name kinds) — attribute *identity* is the UTF-8
string, and unrecognized names are always structurally skippable because
`attribute_length` bounds `info[]` exactly. Mirroring the TASTy convention of
preserving unknown structures losslessly (see `docs/tasty-format-3.9.0.md`
§5), an unrecognized attribute should be retained as a raw
`(name, bytes)` pair rather than dropped, so re-encoding is possible.

The subsections below cover the attributes relevant to symbol-level semantic
analysis. Every other standard attribute (`StackMapTable`, `LineNumberTable`,
`LocalVariableTable`, `LocalVariableTypeTable`, `SourceDebugExtension`,
`Module*`, `AnnotationDefault`, etc.) is expected to be treated as opaque and
preserved raw in the initial loader.

### 7.2. `ConstantValue`

```text
ConstantValue_attribute {
    u2 attribute_name_index;   // "ConstantValue"
    u4 attribute_length;       // 2
    u2 constantvalue_index;    // -> CONSTANT_{Integer,Float,Long,Double,String}_info
}
```

On a `static final` field only; gives the field's compile-time constant
value. The referenced constant pool entry's type must match the field's
descriptor (`String` fields point at `CONSTANT_String`, numeric fields at the
matching numeric tag).

### 7.3. `Exceptions`

```text
Exceptions_attribute {
    u2 attribute_name_index;         // "Exceptions"
    u4 attribute_length;
    u2 number_of_exceptions;
    u2 exception_index_table[number_of_exceptions];  // -> CONSTANT_Class_info
}
```

On a method; the checked exception types declared with `throws`.

### 7.4. `InnerClasses`

```text
InnerClasses_attribute {
    u2 attribute_name_index;     // "InnerClasses"
    u4 attribute_length;
    u2 number_of_classes;
    inner_class_info classes[number_of_classes];
}

inner_class_info {
    u2 inner_class_info_index;     // -> CONSTANT_Class_info
    u2 outer_class_info_index;     // -> CONSTANT_Class_info, or 0
    u2 inner_name_index;           // -> Utf8 simple name, or 0 if anonymous
    u2 inner_class_access_flags;   // source-level flags of the inner class
}
```

On a class; every class or interface that is a member of this class file's
constant pool *and* is itself a nested/inner/local/anonymous class gets an
entry here, including the class's own entry when it is itself nested.
`outer_class_info_index` is `0` for local and anonymous classes (their
enclosing context instead comes from `EnclosingMethod`, §7.5).
`inner_class_access_flags` carries the *source-level* modifiers (visibility,
`static`, etc. as originally declared), which can differ from the class's own
`access_flags` (compiled inner classes are always given package-private
top-level flags plus adjusted bits).

### 7.5. `EnclosingMethod`

```text
EnclosingMethod_attribute {
    u2 attribute_name_index;   // "EnclosingMethod"
    u4 attribute_length;       // 4
    u2 class_index;            // -> CONSTANT_Class_info, enclosing class
    u2 method_index;           // -> CONSTANT_NameAndType_info, or 0
}
```

On a local or anonymous class; identifies the immediately enclosing class
and, if the class was declared inside a method/constructor body, that
member's name and descriptor.

### 7.6. `Signature`

```text
Signature_attribute {
    u2 attribute_name_index;   // "Signature"
    u4 attribute_length;       // 2
    u2 signature_index;        // -> Utf8, grammar in §6.3
}
```

On a class, field, method, or record component (class file ≥ 49.0).

### 7.7. `SourceFile`

```text
SourceFile_attribute {
    u2 attribute_name_index;   // "SourceFile"
    u4 attribute_length;       // 2
    u2 sourcefile_index;       // -> Utf8, simple file name (no path)
}
```

On a class; analogous in intent to TASTy's `SOURCEFILE` position/attribute
entry, but here it is a bare file name (e.g. `Foo.scala`), not a full path.

### 7.8. `Deprecated`

```text
Deprecated_attribute {
    u2 attribute_name_index;   // "Deprecated"
    u4 attribute_length;       // 0
}
```

Marker attribute (no payload) on a class, field, or method.

### 7.9. `Record` and `record_component_info`

```text
Record_attribute {
    u2 attribute_name_index;   // "Record"
    u4 attribute_length;
    u2 components_count;
    record_component_info components[components_count];
}

record_component_info {
    u2             name_index;         // Utf8
    u2             descriptor_index;   // Utf8, field descriptor
    u2             attributes_count;
    attribute_info attributes[attributes_count];  // e.g. Signature, RuntimeVisibleAnnotations
}
```

On a `record` class (class file ≥ 60.0, JDK 16). Each component is *not* the
same structure as `field_info`/`method_info`; it is a separate declaration
independent of, though normally paired with, the record's synthesized
private final field and public accessor method of the same name. A loader
reconstructing "this is a Scala/Java record with components `(name, type)*`"
should read this attribute rather than pattern-matching field/accessor
naming conventions.

### 7.10. `PermittedSubclasses`

```text
PermittedSubclasses_attribute {
    u2 attribute_name_index;   // "PermittedSubclasses"
    u4 attribute_length;
    u2 number_of_classes;
    u2 classes[number_of_classes];   // -> CONSTANT_Class_info
}
```

On a `sealed` class or interface (class file ≥ 61.0, JDK 17). Lists the
classes/interfaces directly permitted to extend/implement it. This is the
structural counterpart of Scala's `sealed` closed hierarchies and is expected
to matter for exhaustiveness-style semantic analysis carried over from
TASTy.

### 7.11. `NestHost` and `NestMembers`

```text
NestHost_attribute {
    u2 attribute_name_index;   // "NestHost"
    u4 attribute_length;       // 2
    u2 host_class_index;       // -> CONSTANT_Class_info
}

NestMembers_attribute {
    u2 attribute_name_index;   // "NestMembers"
    u4 attribute_length;
    u2 number_of_classes;
    u2 classes[number_of_classes];   // -> CONSTANT_Class_info
}
```

Class file ≥ 55.0 (JDK 11). A *nest* is the set of classes compiled from one
top-level source declaration (a top-level class and all its nested classes);
members other than the host carry `NestHost` pointing at the top-level
class, and the host optionally carries `NestMembers` listing them back.
Nest membership governs private-member accessibility between classes that
share a nest — relevant if semantic analysis needs to model Scala-visible
access to Java private members synthesized across nested classes.

### 7.12. `Code` (structural only)

```text
Code_attribute {
    u2 attribute_name_index;   // "Code"
    u4 attribute_length;
    u2 max_stack;
    u2 max_locals;
    u4 code_length;
    u1 code[code_length];               // raw bytecode, opaque to this loader
    u2 exception_table_length;
    exception_table_entry exception_table[exception_table_length];
    u2 attributes_count;
    attribute_info attributes[attributes_count];  // e.g. LineNumberTable, StackMapTable
}

exception_table_entry {
    u2 start_pc;
    u2 end_pc;
    u2 handler_pc;
    u2 catch_type;   // -> CONSTANT_Class_info, or 0 for "any" (finally)
}
```

On every method except native/abstract ones. Not needed for semantic type
analysis; the loader must still be able to walk past it (using
`attribute_length`) and, if the writer side ever needs to round-trip class
files unchanged, retain `code[]` and its nested attributes as opaque bytes
rather than parse them.

### 7.13. `BootstrapMethods` (structural only)

```text
BootstrapMethods_attribute {
    u2 attribute_name_index;   // "BootstrapMethods"
    u4 attribute_length;
    u2 num_bootstrap_methods;
    bootstrap_method bootstrap_methods[num_bootstrap_methods];
}

bootstrap_method {
    u2 bootstrap_method_ref;         // -> CONSTANT_MethodHandle_info
    u2 num_bootstrap_arguments;
    u2 bootstrap_arguments[num_bootstrap_arguments];  // constant pool indices
}
```

On a class, required if any `CONSTANT_Dynamic`/`CONSTANT_InvokeDynamic`
entry exists in the constant pool. Parsed structurally so the class file
round-trips; not interpreted for `invokedynamic` call-site resolution.

### 7.14. Annotation attributes (lower priority)

`RuntimeVisibleAnnotations` / `RuntimeInvisibleAnnotations` (on classes,
fields, methods, record components) and `RuntimeVisibleParameterAnnotations`
/ `RuntimeInvisibleParameterAnnotations` (on methods) share one `annotation`
structure:

```text
annotation {
    u2 type_index;                 // -> Utf8, a field descriptor naming the annotation type
    u2 num_element_value_pairs;
    element_value_pair element_value_pairs[num_element_value_pairs];
}

element_value_pair {
    u2            element_name_index;  // -> Utf8
    element_value value;
}

element_value {
    u1 tag;   // 'B','C','D','F','I','J','S','Z','s' (String), 'e' (enum), 'c' (class), '@' (nested annotation), '[' (array)
    // payload shape depends on tag:
    //   const_value_index: u2                          for primitive/String/'s' tags
    //   enum_const_value { u2 type_name_index; u2 const_name_index; }  for 'e'
    //   class_info_index: u2                            for 'c'
    //   annotation_value: annotation                    for '@'
    //   array_value { u2 num_values; element_value values[num_values]; }  for '['
}
```

Needed only if/when semantic analysis has to see Java annotations that
influence Scala semantics (e.g. `@FunctionalInterface`, `@Deprecated` as an
annotation distinct from the `Deprecated` attribute, or SAM-related
metadata). Treated as skippable/opaque until then.

### 7.15. `MethodParameters` (lower priority)

```text
MethodParameters_attribute {
    u2 attribute_name_index;   // "MethodParameters"
    u4 attribute_length;
    u1 parameters_count;
    method_parameter parameters[parameters_count];
}

method_parameter {
    u2 name_index;       // -> Utf8, or 0 if unnamed
    u2 access_flags;     // ACC_FINAL=0x0010, ACC_SYNTHETIC=0x1000, ACC_MANDATED=0x8000
}
```

Optional, compiler-dependent source of formal parameter names (`javac`
requires `-parameters`; `scalac`'s Java-facing output may or may not emit
it). Useful as a fallback for parameter names when no richer source (like a
sibling TASTy file) is available; absent by default.

## 8. Internal form of names

Binary class/interface names use *internal form*: `.` is replaced by `/`
(`java.lang.Thread` → `java/lang/Thread`), and array class names use the
field-descriptor array syntax (`[Ljava/lang/Thread;`, `[[I`). Unqualified
names (field names, method names, local variable names) must contain at
least one character and must not contain `.`, `;`, `[`, or `/`; method names
additionally exclude `<` and `>` except for the two special names `<init>`
and `<clinit>`.

There is no attribute that directly encodes "this binary name is a nested
class of that one" beyond the naming convention (`Outer$Inner`) plus the
`InnerClasses`/`EnclosingMethod` attributes (§7.4–7.5), which are the
authoritative source for reconstructing nesting — the `$`-separated name
should be treated as a convention to fall back on, not as a guaranteed
parseable structure (member names themselves may legally contain `$`).

Module and package names (`CONSTANT_Module_info`, `CONSTANT_Package_info`,
§3) use a distinct, non-internal-form encoding with a backslash escape
scheme; out of scope while the module system is unsupported.

## 9. Out of scope for the initial loader

- `module-info` class files (`ACC_MODULE`) and the `Module`, `ModulePackages`,
  `ModuleMainClass` attributes.
- Bytecode verification and the `StackMapTable` attribute's type-checking
  semantics (the attribute itself is just skipped/preserved raw).
- Resolving `invokedynamic`/`CONSTANT_Dynamic` call sites (structural parsing
  of `BootstrapMethods` and the constant pool entries is enough to traverse
  the file).
- Class file versions below what JDK 25 / Scala 3.9.0 actually emits;
  historical version-gated flag semantics (e.g. `ACC_STRICT`, `ACC_SUPER`)
  are documented above only to the extent needed to parse a wider
  compatibility range without misinterpreting a flag bit.
