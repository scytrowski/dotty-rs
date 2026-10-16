# JDK module metadata contract

This note freezes the Java 21 JPMS `module-info.class` wire contract and the
class-file crate boundary. `dotty-classfile` exposes typed, lossless codecs
for the JVMS `Module`, `ModulePackages`, and `ModuleMainClass` attributes, plus
an explicit validator for `module-info.class`. It does not implement module
resolution or runtime support.

The pinned real-JDK oracle is Eclipse Temurin `21.0.8+9-LTS` (class-file major
version 65). The normative reference is [JVMS Java SE 21, Chapter 4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html),
especially §§4.1, 4.4, 4.7.1, and 4.7.25–4.7.27. Class-file module metadata is
distinct from Scala `object` / `ModuleClass` symbols: an `ACC_MODULE`
`module-info` class is a JVM descriptor, not a Scala declaration.

## Descriptor class shape

A module descriptor has `ACC_MODULE` (`0x8000`) set and names
`module-info` in `this_class`. Its major version is at least 53.0, and its
`super_class`, interfaces, fields, and methods are absent. The only legal
class access flag is `ACC_MODULE`. It has exactly one `Module` attribute.
`ModulePackages` and `ModuleMainClass` may occur at most once. Other standard
class attributes are restricted to the list in JVMS §4.1; non-standard
attributes remain subject to the normal unknown-attribute rules in §4.7.1.

The constant-pool tags `CONSTANT_Module` (19) and `CONSTANT_Package` (20) are
valid only in a module descriptor. Each contains a `name_index` to a
`CONSTANT_Utf8`: module names use the JVMS module-name form, and package names
use slash-separated internal package form. The `Module` attribute refers to
modules, packages, and service/provider classes through these corresponding
constant-pool kinds.

## `Module` attribute wire schema

All `u2` and `u4` values use the class-file big-endian encoding. Rows and their
variable-length indices occur in the order shown. Every count is a `u2`; there
are no alignment bytes or implicit terminators.

| Field | Wire type | Constant-pool kind / interpretation |
| --- | --- | --- |
| `module_name_index` | `u2` | `CONSTANT_Module` |
| `module_flags` | `u2` | `ACC_OPEN`, `ACC_SYNTHETIC`, `ACC_MANDATED` |
| `module_version_index` | `u2` | zero or `CONSTANT_Utf8` |
| `requires_count` | `u2` | number of `requires` rows |
| each `requires_index` | `u2` | `CONSTANT_Module` |
| each `requires_flags` | `u2` | `ACC_TRANSITIVE`, `ACC_STATIC_PHASE`, `ACC_SYNTHETIC`, `ACC_MANDATED` |
| each `requires_version_index` | `u2` | zero or `CONSTANT_Utf8` |
| `exports_count` | `u2` | number of `exports` rows |
| each `exports_index` | `u2` | `CONSTANT_Package` |
| each `exports_flags` | `u2` | `ACC_SYNTHETIC`, `ACC_MANDATED` |
| each `exports_to_count` | `u2` | number of target modules |
| each `exports_to_index` | `u2[]` | each is `CONSTANT_Module`; empty means unqualified export |
| `opens_count` | `u2` | number of `opens` rows |
| each `opens_index` | `u2` | `CONSTANT_Package` |
| each `opens_flags` | `u2` | `ACC_SYNTHETIC`, `ACC_MANDATED` |
| each `opens_to_count` | `u2` | number of target modules |
| each `opens_to_index` | `u2[]` | each is `CONSTANT_Module`; empty means unqualified open |
| `uses_count` | `u2` | number of service class indices |
| each `uses_index` | `u2` | `CONSTANT_Class` |
| `provides_count` | `u2` | number of service/provider rows |
| each `provides_index` | `u2` | service type, `CONSTANT_Class` |
| each `provides_with_count` | `u2` | provider count; must be nonzero |
| each `provides_with_index` | `u2[]` | provider class, each `CONSTANT_Class` |

An optional version uses index zero to mean “no version recorded”; it is not a
constant-pool entry. The remaining indices must resolve to the exact kinds
above, rather than merely any valid constant-pool slot. JVMS requires unique
module names within `requires`, package names within each of `exports` and
`opens`, target modules within one qualified row, service types within
`uses`/`provides`, and provider classes within each `provides` row. The
`provides_with_count` is nonzero. An open module has no `opens` rows.
`ModulePackages` also has unique package names in its own table.

| Flag | Value | Legal field |
| --- | ---: | --- |
| `ACC_OPEN` | `0x0020` | module flags |
| `ACC_TRANSITIVE` | `0x0020` | requires flags |
| `ACC_STATIC_PHASE` | `0x0040` | requires flags |
| `ACC_SYNTHETIC` | `0x1000` | module, requires, exports, opens flags |
| `ACC_MANDATED` | `0x8000` | module, requires, exports, opens flags |

Bits not listed for the relevant field are reserved and must be zero. Every
non-`java.base` descriptor has at least one `requires` row and exactly one row
for `java.base`. Under Java 21, that row must not have `ACC_SYNTHETIC` set;
`ACC_MANDATED` may be set. For class-file version 54.0 and later, neither
`ACC_TRANSITIVE` nor `ACC_STATIC_PHASE` may be set on it. The current Java 26
JVMS permits `ACC_TRANSITIVE` for this row, so that newer rule must not be
silently applied to this Java 21 contract.

## Companion attributes

| Attribute | Payload | Placement and cardinality |
| --- | --- | --- |
| `ModulePackages` | `u2 package_count`, then `package_index: u2[package_count]` (`CONSTANT_Package`) | `module-info` class attribute; optional, at most once. Includes packages exported or opened by `Module`, packages containing listed providers, and may include other packages in the module. |
| `ModuleMainClass` | `main_class_index: u2` (`CONSTANT_Class`) | `module-info` class attribute; optional, at most once; payload length exactly 2. |

`ModulePackages` supplements the directive table; it is not the `jmod describe`
“contains” section alone. In the JDK snapshots below, exported packages plus
the non-exported packages printed by `jmod describe` account for the
`ModulePackages` set. `ModuleTarget` and `ModuleHashes` are JDK-specific
attributes not defined by JVMS §§4.7.25–4.7.27. This decoder currently keeps
them as opaque unknown attributes.

## Decode and validation boundary in this repository

The implementation boundary in `dotty-classfile` is:

* constant-pool tags 19 and 20 are decoded as `ConstantPoolEntry::Module` and
  `ConstantPoolEntry::Package`; encoding validation checks their name indices
  resolve to UTF-8 entries.
* `ACC_MODULE` is represented by the existing access-flags type.
* `Module` decodes to a typed attribute with ordered vectors and original
  constant-pool indices. Its encoder preserves the payload fields and optional
  version zero indices. Constant-pool validation checks Module, Package,
  Class, and Utf8 references, including name indices inside Module/Package
  constant-pool entries.
* `ModulePackages` and `ModuleMainClass` decode into typed variants, preserve
  order and presence, encode through checked lengths, and validate the exact
  package/class constant-pool kinds. `ModuleTarget`, `ModuleHashes`, and other
  unrecognized attributes remain `Attribute::Other` and retain their bytes.
* `ClassFile::validate_module_descriptor` is an explicit semantic check. It
  validates `ACC_MODULE` shape, version and flags, required and unique
  attributes, permitted predefined class attributes, module/package names,
  directive references and uniqueness, `java.base` constraints, and the
  package coverage required by a present `ModulePackages` table. It does not
  resolve dependencies or assert that referenced classes exist.
* `ClassFile::decode` and `validate_references` remain usable independently;
  ordinary class files are not forced through JPMS descriptor rules.
* Known structured attributes are decoded within their declared length and
  reject trailing bytes. Unknown payloads are intentionally opaque, so their
  internal structure cannot be validated. `ClassFile::decode` consumes one
  class file; callers can check `Reader::is_at_end()` when the enclosing input
  must contain no trailing bytes.

The error surface is deliberately split across layers:

| Layer | Errors owned by that layer |
| --- | --- |
| Byte decoder / typed attribute codec | truncated header/table/attribute; impossible declared length; malformed attribute-name reference; reserved `Module` field flags; empty provider row; trailing bytes in the recognized payload. |
| Constant-pool reference validation | wrong constant-pool kind or out-of-range reference in a typed payload; nonzero optional version index not pointing to UTF-8; bad Utf8 target inside a referenced Module/Package/Class entry. |
| Explicit descriptor validator | missing or duplicate `Module`; duplicate optional attributes; wrong placement or class-file version; invalid `ACC_MODULE` shape; malformed names; cross-row uniqueness; open-module/opens interaction; `java.base` row constraints; and `ModulePackages` coverage. |
| Opaque preservation | unknown attributes are retained byte-for-byte; the decoder makes no semantic claims about their payloads. |

Class-level semantic checks belong in an explicit descriptor-validation
operation, not in lossless attribute decoding: structurally readable class
files and their unknown attributes should remain inspectable even when they
are not valid module descriptors.

## Pinned oracle and canonical metadata

The binary fixtures live under
[`crates/dotty-classfile/tests/fixtures/jdk_module_info`](../crates/dotty-classfile/tests/fixtures/jdk_module_info).
Three are extracted from the pinned JDK's `java.base.jmod`,
`java.net.http.jmod`, and `jdk.jshell.jmod`; four tiny descriptors are built
with that JDK's `javac`/`jmod`. Only each `module-info.class` is checked in, not
the JMOD archives. The seven class files total 15,084 bytes. Their SHA-256
hashes are pinned in `SHA256SUMS`. The generator checks the exact JDK runtime
version before replacing any fixture.

This path-independent snapshot records semantic counts and names from
`javap -v` (attribute payload) and `jmod describe` (module directives and
content); it deliberately excludes constant-pool indices, host paths, and
platform-dependent hash bytes:

```json
{
  "oracle": "Eclipse Temurin 21.0.8+9-LTS",
  "java.base": {
    "version": "21.0.8", "flags": [], "requires": 0,
    "exports": 121, "opens": 0, "uses": 36, "provides": 2,
    "module_packages": 196, "main_class": null,
    "module_target": "linux-amd64", "module_hashes_present": true
  },
  "java.net.http": {
    "version": "21.0.8", "flags": [],
    "requires": ["java.base (mandated; version absent)"],
    "exports": ["java.net.http"], "opens": [], "uses": [], "provides": [],
    "module_packages": ["java.net.http", "jdk.internal.net.http",
      "jdk.internal.net.http.common", "jdk.internal.net.http.frame",
      "jdk.internal.net.http.hpack", "jdk.internal.net.http.websocket"],
    "main_class": null, "module_target": "linux-amd64"
  },
  "jdk.jshell": {
    "version": "21.0.8", "flags": [],
    "requires": ["java.base (mandated)", "java.logging", "jdk.compiler",
      "jdk.internal.ed", "jdk.internal.le", "jdk.internal.opt",
      "java.compiler (transitive)", "java.prefs (transitive)",
      "jdk.jdi (transitive)"],
    "exports": 4, "opens": [], "uses": 2, "provides": 3,
    "module_packages": 10, "main_class": null,
    "module_target": "linux-amd64"
  }
}
```

The generated `fixture.directives` descriptor pins a qualified export to two
modules, a qualified open, one service use, one service provider, and the
ordered package list. `fixture.main` pins `ModuleMainClass`. `fixture.minimal`
pins absence of both optional attributes. The final variant changes only
`module_version_index` and its `requires_version_index` to zero, pinning the
optional-index encoding. The integration test checks major version 65,
descriptor class shape, module-name constant-pool kind, modeled references,
the exact opaque companion-attribute set, and byte-for-byte decode/encode
identity. A directed-fixture assertion checks decoded table ordering, flags,
names, optional versions, and provider lists.

## Reproduction and oracle commands

From the repository root, with the pinned JDK installed at the path below:

```sh
export JAVA_HOME="$PWD/target/sdkman-local/candidates/java/21.0.8-tem"
export TMPDIR="$PWD/target/tmp-issue-1197"
bash crates/dotty-classfile/tests/fixtures/jdk_module_info/generate.sh
(cd crates/dotty-classfile/tests/fixtures/jdk_module_info && sha256sum -c SHA256SUMS)
"$JAVA_HOME/bin/jmod" describe "$JAVA_HOME/jmods/java.net.http.jmod"
"$JAVA_HOME/bin/jmod" describe "$JAVA_HOME/jmods/jdk.jshell.jmod"
"$JAVA_HOME/bin/javap" -v -p \
  crates/dotty-classfile/tests/fixtures/jdk_module_info/generated/directives-module-info.class
```

Use `javap -v -p` on the real module-info class extracted from each JMOD to
inspect raw attribute rows and constant-pool references. `jmod describe` is a
human-readable comparison oracle for module directives and package contents,
not a byte-level grammar validator. The fixtures and integration test are the
repeatable repository gate; they do not invoke an installed JDK during Cargo
tests.

## Follow-up issue boundaries

Keep the next work incremental and separate:

1. **Typed `Module` codec**: completed by #1198, including ordered tables,
   checked lengths, field flags, non-empty providers, constant-pool kinds,
   optional version indices, and exact payload re-encoding.
2. **Descriptor validation and metadata extras**: completed by #1199 for
   companion attributes and descriptor validation. JDK-specific attributes
   remain raw unless a later issue requires typed metadata.
3. **Catalog**: expose module descriptors through a classpath/module catalog
   without doing resolution in the class-file decoder.
4. **Benchmark**: measure decoding and metadata projection on a bounded JDK
   fixture set; do not benchmark full JMOD archive loading as codec work.
5. **Compatibility gate**: compare against the pinned Java 21 oracle and add
   malformed-input expectations before changing the source-language typer.

`dotty-classfile` owns the wire codec, `dotty-classloader` owns discovery and
catalog concerns, and `dotty-typer`/`SymbolResolver` are outside this issue's
scope. In particular, JPMS metadata must not be modeled as Scala module
symbols.
