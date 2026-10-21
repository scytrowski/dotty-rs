Warning: truncated output (original token count: 13327)
Total output lines: 870

# Compiler Classloader Module

Status: implemented (`crates/dotty-classloader`). `.class` and `.tasty`
classpath entries both load onto `dotty-core`'s canonical semantic model
(`SemanticStore`/`Symbol`/`SymbolId`/`Type`) — this crate no longer keeps a
parallel `Rc<ClassSymbol>` model of its own. §9 lists what each milestone
covers and its current status; §3 and §4.4 list the real, currently-open
gaps (mostly on the `.tasty` side) rather than unimplemented milestones.

Normative sources:
- JVM Specification SE 26, Chapter 4 — [The `class` File Format](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-4.html) (already the basis of `docs/classfile-format-jdk25.md`)
- JVM Specification SE 26, Chapter 5 — [Loading, Linking, and Initializing](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-5.html)
- JLS SE 26, Chapter 12, §12.2 — [Loading of Classes and Interfaces](https://docs.oracle.com/javase/specs/jls/se26/html/jls-12.html)

Where this document is silent, the JVMS is authoritative, per the same
policy as `docs/classfile-format-jdk25.md`.

## 1. Goal and non-goal

Goal: a compiler-frontend class loader — locate classpath entries, decode
`.class` files via `dotty-classfile` and `.tasty` files via `dotty-tasty`,
construct semantic class symbols in `dotty-core`'s `SemanticStore`, and
resolve references between them, for `dotty-rs`'s eventual semantic
analysis / backend.

Non-goal: this is not `java.lang.ClassLoader` and not a JVM runtime.
Explicitly out of scope: bytecode execution, bytecode verification
(`StackMapTable` semantics), class preparation/initialization, `<clinit>`
execution, runtime constant pools, runtime loading constraints, JNI,
reflection, garbage collection. JVMS Chapter 5 is read for its *naming,
loading-dependency and resolution* semantics, not as a checklist to
implement the full JVM lifecycle.

## 2. Relationship to existing crates

```text
classpath
    |
class/tasty lookup by BinaryName
    |
raw bytes
    |
dotty-classfile (.class) / dotty-tasty (.tasty)   <- already implemented, reused as-is
    |
class metadata (already decoded: descriptors, generic signatures, attributes)
    |
symbol creation (dotty-core::SemanticStore)        <- this crate
    |
lazy symbol resolution                             <- this crate
    |
dotty-rs semantic model                            <- future crate(s), consumes SemanticStore directly
```

`dotty-classfile` already covers descriptor parsing (JVMS §4.3.2/4.3.3),
generic `Signature` grammar (JVMS §4.7.9), and the attributes needed for
symbol-level information (`Signature`, `InnerClasses`, `EnclosingMethod`,
`NestHost`/`NestMembers`, `Record`, `PermittedSubclasses`, `Exceptions`,
annotations, `MethodParameters` — see `docs/classfile-format-jdk25.md`
§7). `dotty-classloader` does not re-implement any of that; it consumes
`ClassFile::decode` output directly. `dotty-tasty` covers the `.tasty`
wire/AST layers the same way; `dotty-classloader`'s own `tasty_symbol`
module consumes `TastyFile`/`RawTree`/`StructuredNode` output, not raw
bytes. `dotty-core` owns the canonical semantic model this crate builds
into: `SemanticStore` (symbols/types/scopes/names/annotations arenas),
`Symbol`/`SymbolId`, `Type`/`TypeId`, `Scope`/`ScopeId`, `Definitions`
(the one shared set of builtin primitive/`Object`/`Any`/`Nothing`
identities every lowering step targets). This crate's own job is:
classpath discovery, the enter/complete symbol lifecycle, descriptor/
signature/`.tasty`-tree lowering into `dotty-core` `Type`s, and reference
resolution.

## 3. Scope

### Implemented
- A canonical binary class name representation (`BinaryName`),
  internal-form (`java/lang/Object`), matching `docs/
  classfile-format-jdk25.md` §8. `$` stays a naming convention on the
  name itself — nesting *ownership* (§4.3) is reconstructed from
  `InnerClasses`/`EnclosingMethod`, not string-split, but a class's own
  `BinaryName` still keeps its `$`-joined form.
- An open classpath abstraction (`ClassPathEntry` trait, `Send + Sync`)
  plugged by `DirectoryClassPath`, `JarClassPath` (including
  multi-release JAR version selection, JEP 238), `JdkClassPath`/
  `JmodClassPath` (`$JAVA_HOME/jmods`), and `CompositeClassPath`
  (ordered, first-match-wins). JAR/JMOD reading includes this crate's
  own ZIP central-directory reader and index. Raw DEFLATE streams use
  `flate2`'s `zlib-rs` backend. Decoding uses a fixed-size output buffer and
  enforces the entry output limit incrementally; CRC verification remains in
  the ZIP layer. Malformed DEFLATE streams use a general invalid-data error
  under the existing `classpath.zip.invalid_deflate_data` diagnostic code;
  truncated streams and output-limit violations keep separate errors.
- The crate-private `ZipArchive` supports memory-backed and file-backed
  storage. The file-backed constructor reads only the bounded EOCD window
  and central-directory index while opening; entry reads fetch the local
  header and compressed range on demand. A mutex protects seek/read pairs
  on the shared file handle. ZIP-relative offsets are translated from the
  supplied archive start, including a 4-byte JMOD prefix. `JmodClassPath`
  validates the JMOD magic and uses this backing, so `JdkClassPath::new`
  indexes each JMOD's metadata without retaining its complete payload; JARs
  remain memory-backed. If an opened JMOD is truncated or modified in place,
  later reads use the current file contents and report typed ZIP errors if
  those bytes no longer match the indexed archive. On systems that permit
  unlinking an open file, removing its path does not invalidate the already
  open handle. Central directories and compressed entries are each bounded
  to 256 MiB before allocation.
- `JdkClassPath::module_catalog` explicitly reads, validates, and projects
  each opened JMOD's exact `classes/module-info.class` into an owned
  `JdkModuleCatalog`. Catalog entries are sorted by their descriptor module
  names; duplicate names and JMOD filename/descriptor mismatches are typed
  errors. Requires/exports/opens/uses/provides order and flags, optional
  versions, `ModulePackages`, `ModuleMainClass`, and `ClassOrigin::Jmod` are
  preserved as owned data. This is JVM JPMS metadata, separate from Scala
  module symbols and source resolution.
- Module metadata decoding is lazy: `JdkClassPath::new` still only opens and
  indexes JMOD archives as before. Each explicit `module_catalog()` call
  decodes descriptors from those same open file-backed entries, returns a
  fresh owned snapshot, and retains no module-info bytes or global cache.
  Missing descriptors, malformed class files, failed JPMS validation, bad
  constant-pool references, duplicate module identities, and filename
  mismatches return `JdkModuleCatalogError` with the JMOD origin and underlying
  decoder/archive cause when present.
- Two package-evidence queries with distinct contracts:

  | Check | Meaning |
  | --- | --- |
  | `contains_package` | A usable class/TASTy resource is directly in the package. |
  | `contains_package_prefix` | A usable class/TASTy resource is in the package or any descendant package. |

  Package-prefix evidence is based on logical `.class`/`.tasty` resource
  names; it does not decode resource bytes, create package symbols, or prove
  that the package has a direct class, module, or package-object term.
  Directory entries must be files within the classpath root; archive package
  prefixes are indexed from resource names, not ZIP directory records.
  Multi-release JAR entries contribute to this query only when the manifest
  declares `Multi-Release: true`, and only for versions from 9 through the
  configured effective release. JMOD resources are considered only under
  their `classes/` root. The empty path represents the unnamed/default
  package boundary, while `_root_` is a Scala source lookup marker; these
  classpath checks do not implement Scala default imports or source lookup.
- Dual-format loading: a classpath entry reports `ClassFormat::Class` or
  `ClassFormat::Tasty` per `ClassResource`; `.tasty` is preferred when
  both exist for the same name.
- For the pinned Scala 3.9.0 inputs, the classloader accepts stable TASTy
  formats 28.8.0 (compiler output) and 28.9.0 (library output). This is an
  input policy in `dotty-classloader`; the public strict
  `TastyFile::parse_scala_3_9` API and the pickler's 28.9.0 target remain
  unchanged. Other minor/experimental versions are rejected until explicitly
  validated. Accepting a wire version does not imply that every semantic tree
  can be completed; unsupported references still produce typed errors.
- A `ClassRepository` cache keyed by `BinaryName` with explicit states
  (missing / loading / loaded / failed) so a class already being
  completed is visible to whatever references it next, instead of being
  reloaded from scratch (§5).
- Full class-level reconstruction onto `dotty-core` symbols: name,
  access flags, superclass/interfaces (`Type::ClassInfo`), class-level
  generic type parameters (`SymbolKind::TypeParameter`, §4.4), and a
  real hierarchical package owner chain (§4.2) and enclosing-class owner
  for nested/local/anonymous classes (§4.3).
- Fields, methods, and constructors as real `dotty-core` symbols
  (`SymbolKind::Field`/`Method`/`Constructor`) with real `Type::Method`/
  `Type::Poly` types, lowered from JVM descriptors and — when present
  and consistent with the descriptor — generic `Signature` attributes.
- JVM signature type variables use this class's type parameters, followed
  by enclosing member-class parameters from nearest to farthest. A method's
  own parameters shadow all class parameters. Enclosing parameters are
  visible only across non-static member-class edges recorded by the current
  class's `InnerClasses` self-entry; a static edge stops lookup. The scopes
  come from canonical class symbols, including declaration scopes already
  entered for an enclosing class that is still loading. Missing or out-of-
  scope names remain `UnresolvedTypeVariable` errors.
- Annotations (`RuntimeVisibleAnnotations`/`RuntimeInvisibleAnnotations`)
  resolved best-effort onto real `dotty_core::Annotation`/`AnnotationId`
  entries on their owning symbol, plus a JVM-facing
  `SemanticAnnotation`/`AnnotationValue` sidecar preserving element
  values JVMS §4.7.16.1 that have no home in `dotty_core::Annotation`.
- Remaining class-level attributes as JVM-facing sidecar data (§4.1):
  `NestHost`/`NestMembers`/`PermittedSubclasses`/`InnerClasses`/
  `EnclosingMethod`/`Record` components/`MethodParameters`.
- A structured error model (`ClassLoadError`) distinguishing not-found,
  I/O failure, malformed class file / reference / descriptor /
  signature, invalid `.tasty` file, name mismatch, circular inheritance,
  excessive inheritance depth, an unresolved generic type variable, and a
  dependency's failure propagating to its dependent.
- Explicit classpath aliases for the compiler-only `scala.Any`,
  `scala.AnyRef`, and `scala.Nothing` type names. `Any` and `Nothing` reuse
  their `Definitions` identities; `AnyRef` reuses `Definitions::object_class`.
  These aliases are limited to the `scala` package's type namespace and do
  not probe the classpath. Primitive and `Unit` names keep their real
  classpath classes.
- `.tasty`-backed loading converging on the same `Symbol`/`Type::ClassInfo`
  shape as `.class` loading (§4.4), including case-class constructor
  accessor fields.
- A shared-store `ClasspathSymbolResolver` implementing core's
  `SymbolResolver` port. It reuses canonical `Definitions`, package
  identities, and `LoadingSession`; package lookups require existing
  package state or positive classpath evidence for the full requested
  package path. Descendant evidence can establish an intermediate package;
  the resolver then reuses `dotty_core::Packages` to enter each canonical
  segment. A missing target does not allocate its ancestors, and `_root_` is
  treated as the source root marker rather than a package name. Class prefixes
  support `TypeRef`, `Applied`, and `ThisType` shapes. Resolver calls and caller
  transaction rollbacks undo package-scope links and session cache entries
  together with store allocations.

### Explicitly out of scope
- JPMS module-path resolution, readability/accessibility checks, module-layer
  initialization, service loading, JIMAGE, and `ct.sym`. The catalog reports
  validated descriptor metadata only; it does not alter class lookup or
  determine which modules a Scala source file can read.
- Bytecode verification, `StackMapTable` type-checking,
  `invokedynamic`/`CONSTANT_Dynamic` call-site resolution.
- Explicit field/method/interface-method *overload* resolution (JVMS
  §5.4.3.2-5.4.3.4) as an API distinct from class loading — this crate
  builds the symbols that resolution would consume, but does not
  implement call-site resolution itself.
- Any part of the JVM runtime lifecycle listed in §1's non-goal.

(These mirror `docs/classfile-format-jdk25.md` §9, which already excludes
the JVMS-adjacent ones at the codec layer.)

### Known, deliberately scoped gaps
These are documented, honest simplifications — not silently missing
behavior — each with a doc comment at its own definition site:
- **`.tasty` member types are best-effort, not exact.** JVM descriptors
  are always complete and exact; a `.tasty` value type is reduced to a
  name via `tasty_symbol::resolve_parent_name`, which only recognizes
  the pre-typecheck syntax-tree reference shapes `extends` clauses use
  (`IDENT_TAG`/`SELECT_TAG`/...). Ordinary field/parameter/return types
  are post-typecheck `TYPEREF_TAG`/`TERMREF_TAG` nodes, a different,
  currently-unhandled shape — see `tasty_symbol.rs`'s module doc comment.
  `ClassLoader::lower_tasty_member_type` falls back to `Type::Error`
  whenever a name can't be reduced *or* the reduced name fails to load,
  rather than ever hard-failing the class load over…6327 tokens truncated…lassRepository` (crate-private) — the state machine described in §5,
  keyed by `BinaryName`, storing `SymbolId`s (not shared-ownership
  symbols — a `SymbolId` is already `Copy` and session-stable).
- `ClassLoader<'store, E>` — the loader itself; borrows a
  `&'store mut SemanticStore` for the whole session, owns its own
  loader-local `ClassRepository` (covering `Loading`/`Failed`, not
  shared — see `LoadingSession` below), a `LoadingSession`, and a
  `Definitions` (the shared builtin primitive/`Object`/`Any`/`Nothing`
  identities every descriptor/signature/`.tasty`-tree lowering step
  targets).
- `LoadingSession` — bundles separate positive-only `BinaryName -> SymbolId`
  maps for identified shells and fully resolved classes, `PackageRegistry`,
  and the `ClassfileMetadata`/`ClassOrigin`
  sidecar tables (keyed by the already-resolved `SymbolId`, so sharing
  them carries none of the negative-caching risk below) so several
  `ClassLoader`s loading sequentially against one shared `SemanticStore`
  (`ClassLoader::with_definitions`, one per classpath root or
  incremental recompilation unit) agree on the same ordinary
  class/package `SymbolId`s and provenance data, not just the
  `Definitions` builtins — handed from one loader to the next by value
  via `ClassLoader::into_session`. Deliberately excludes negative
  (`Failed`) and in-progress (`Loading`) results: a name one loader's
  own classpath doesn't have is not evidence a differently configured
  loader sharing the session doesn't have it either.
- `ClassRef` — `Unresolved(BinaryName)` / `Resolved(SymbolId)`, used
  only for JVM-specific sidecar metadata (nest host/members, inner/
  enclosing-class references, an annotation's own type) — never for a
  class's canonical superclass/interfaces, which are `Type::TypeRef`s in
  its `Type::ClassInfo` (`dotty-core`'s semantic model), not this
  crate's.
- `ClassfileMetadata` — the `.class`-only JVM sidecar table entry (§4.1).
- `ClassLoadError` — `NotFound`, `Io`, `InvalidClassFile`,
  `MalformedReference`, `MalformedDescriptor`, `MalformedSignature`,
  `NameMismatch`, `InvalidTastyFile`, `CircularInheritance`,
  `InheritanceDepthExceeded`, `DependencyFailure`,
  `UnresolvedTypeVariable`; `Clone` (so `Failed`
  entries can be returned repeatedly from the cache; non-`Clone`
  payloads are `Rc`-wrapped to keep the whole enum cheaply cloneable).

`ClassFile`, `TastyFile`, constant-pool indices, and other wire-level
details never appear in this crate's public API surface (`crate::
classloader`, `lib.rs`) — only `BinaryName`, `dotty-core` symbols/types,
and typed errors do.

## 7. Reference material

- [JVMS SE 26, Chapter 4 — The `class` File Format](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-4.html) — already the basis of `docs/classfile-format-jdk25.md`.
- [JVMS SE 26, Chapter 5 — Loading, Linking, and Initializing](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-5.html) — §5.1, §5.3, §5.3.5, §5.4, §5.4.3, §5.4.4 are the relevant sections for this module.
- [JLS SE 26, Chapter 12 §12.2 — Loading of Classes and Interfaces](https://docs.oracle.com/javase/specs/jls/se26/html/jls-12.html) — source-language framing; defers precise semantics to JVMS Chapter 5.
- Scala 3 compiler, as an architectural reference to study (not to port
  line-by-line):
  [`SymbolLoaders.scala`](https://github.com/scala/scala3/blob/main/compiler/src/dotty/tools/dotc/core/SymbolLoaders.scala) (the enter/complete split — most directly relevant),
  [`ClassfileParser.scala`](https://github.com/scala/scala3/blob/main/compiler/src/dotty/tools/dotc/core/classfile/ClassfileParser.scala),
  [`Symbols.scala`](https://github.com/scala/scala3/blob/main/compiler/src/dotty/tools/dotc/core/Symbols.scala),
  [`Types.scala`](https://github.com/scala/scala3/blob/main/compiler/src/dotty/tools/dotc/core/Types.scala),
  [`Definitions.scala`](https://github.com/scala/scala3/blob/main/compiler/src/dotty/tools/dotc/core/Definitions.scala).
- `docs/dotty-core-design.md` — the canonical semantic model
  (`SemanticStore`/`Symbol`/`Type`/`Scope`) this crate builds into.

## 8. Testing approach

Follows the same policy as the rest of the workspace (`AGENTS.md`):
focused unit tests per behavior, integration tests against real fixtures,
exact error variants asserted rather than `is_err()`.

- Reuse the real JDK-25-`javac`-compiled fixtures under
  `crates/dotty-classfile/tests/fixtures/` (e.g. `pool_sample/`,
  `nested_sample/`, `sealed_record_sample/`) and the real Scala-3.9.0-
  compiled `.tasty` fixtures under `crates/dotty-tasty/tests/fixtures/`
  wherever they exercise loader-relevant shapes, instead of generating
  new ones for cases already covered there.
- The shared JDK runtime-image corpus under
  `crates/dotty-classfile/tests/fixtures/jdk_corpus/` is materialized for
  JDK 21, 22, 23, 24, 25, and 26. Classfile tests run the same structural and
  representative-shape checks against every version; loader visibility tests
  load each version directory as a separate canonical classpath.
- Hand-built minimal class files/synthetic classpaths are sometimes
  unavoidable and acceptable — e.g. a cyclic-inheritance scenario cannot
  be produced by `javac` (rejected at the source level), and a classpath
  combining an in-memory `.tasty` entry with in-memory `.class`
  dependencies needs `CompositeClassPath` over two single-format
  in-memory classpaths. Such fixtures must be clearly marked as
  synthetic, not real compiler output.
- The small catalog fixtures under
  `crates/dotty-classloader/tests/fixtures/jdk_module_catalog/` retain only
  `module-info.class` files, not whole JMOD archives. Their source and
  `generate.sh` pin the reproducer to Amazon Corretto 25.0.4+7-LTS; checksums
  are listed in `SHA256SUMS`.
- The ignored resolver regression
  `real_jdk_object_and_get_class_materialize_through_the_resolver` uses the
  real `$JAVA_HOME/jmods/java.base.jmod` `java/lang/Object.class` and small
  stubs only for its descriptor dependencies. Run it with
  `JAVA_HOME=/path/to/jdk cargo test -p dotty-classloader real_jdk_object_and_get_class_materialize_through_the_resolver -- --ignored`.
  This pins real JDK symbol/member loading while avoiding unrelated eager
  closure over all of `java.base`.
- The ignored JDK classpath integration test
  `finds_and_decodes_classes_from_real_jdk_modules` checks the `java.base` and
  `java.net.http` modules, classfile decoding, origins, and a missing class
  against a real JDK 21 or newer. Set `DOTTY_JDK_JMODS` to the JDK's `jmods`
  directory, or leave it unset to use `$JAVA_HOME/jmods`, then run:
  `DOTTY_JDK_JMODS=/path/to/jdk/jmods cargo test -p dotty-classloader --test real_jdk_class_path -- --ignored`.
  The test is ignored in the normal suite so CI does not depend on a local JDK;
  when explicitly run, a missing or invalid JDK path is a test failure.
- The ignored JDK metadata integration test
  `reads_validated_module_metadata_from_real_jdk_modules` uses the same
  `DOTTY_JDK_JMODS`/`JAVA_HOME` setting and validates the catalog from real
  `java.base` and `java.net.http` JMODs, including deterministic names,
  dependencies, exports, JMOD origins, and unchanged regular class lookup.
- The opt-in JPMS compatibility gate
  `checks_typed_jdk21_descriptors_against_jmod_describe` also requires the
  matching JDK `jmod` command. It checks raw descriptor fields and CLI-visible
  observations for `java.base`, `java.net.http`, and `jdk.jshell`; see
  `docs/jdk-module-metadata-compatibility.md` for its coverage and limitations.
- Differential testing against `javap -p -v` on real fixtures is used
  throughout to confirm exact attribute/descriptor/signature shapes
  before writing an assertion against them (e.g. confirming
  `NestedSample$Inner`'s `InnerClasses` self-entry carries an
  `outer_class` while `NestedSample$1LocalRunnable`'s does not) —
  mirroring the practice already established for `dotty-classfile`.
- When a `.tasty` decoder question can't be answered from documentation
  alone (no `.scala` source is available for any fixture, only compiled
  bytes), temporary `#[cfg(test)] mod scratch_debugN` blocks dumping
  decoded structures via `eprintln!` are an acceptable, deliberately
  throwaway way to inspect real `.tasty` bytes — removed before the
  change is finalized, never left in the committed diff.

## 9. Milestones

All done unless noted otherwise.

1. `BinaryName`, `ClassPathEntry`, `DirectoryClassPath`,
   `CompositeClassPath`, `ClassRepository`, a `.class`-only `ClassLoader`
   producing symbols with name/flags/superclass/interfaces, including
   circular-inheritance detection. **Done.**
2. JAR-backed classpath entries, including multi-release JAR version
   selection. **Done** (`jar_class_path.rs`, `zip_archive.rs`,
   `inflate.rs`, `manifest.rs`).
3. JDK classpath via `$JAVA_HOME/jmods`. **Done** (`jdk_class_path.rs`,
   `jmod_class_path.rs`).
4. Member symbols (fields/methods/constructors) built from the
   descriptors `dotty-classfile` already parses. **Done.**
5. Generic-signature-based semantic types, built from the `Signature`
   grammar `dotty-classfile` already parses — including class-level and
   method-level type parameters (`Type::Poly` for the latter). **Done**
   for `.class`; not yet built for `.tasty` (§3).
6. Lazy reference resolution for member types: reusing an in-progress
   `Loading` shell for a *legitimate* mutual reference, not just
   detecting a cycle (`resolve_member_class`). **Done.**
7. Nested-class and remaining semantic-attribute reconstruction
   (`InnerClasses`, `EnclosingMethod`, `NestHost`/`NestMembers`,
   `Record`, `PermittedSubclasses`, annotations), including a real
   enclosing-class `Symbol::owner` for nested/local/anonymous classes
   (§4.3). **Done** for `.class`; `.tasty` carries none of these
   attributes (§3).
8. Multi-release JAR version selection. **Done** (folded into
   Milestone 2).
9. `.tasty`-backed loading, converging on the same symbol model as
   `.class` loading. **Done** for name/flags/superclass/interfaces/
   fields/methods (§4.4); member types/generics/annotations remain
   best-effort or unbuilt for `.tasty` specifically (§3's "known gaps").

### Open work (not milestone-numbered — tracked here instead)
- Precise `.tasty` member type resolution (the `TYPEREF_TAG`/
  `TERMREF_TAG` decoder gap, §3).
- `.tasty` generics and annotations.
- Explicit field/method/interface-method overload resolution (JVMS
  §5.4.3.2-5.4.3.4) as an API distinct from class loading.

## 10. Top-level Scala module identities

`ClassLoader::ensure_module_identity` establishes the term `Object` and its
backing type `ModuleClass` as separate `SymbolInfo::Missing` shells. It only
accepts a Scala 3.9 `.tasty` file whose AST proves both exact definitions as
direct members of the requested package. A JVM class name ending in `$` and a
classfile alone do not prove module identity. The API records the pair in the
shared `LoadingSession`, so sequential loaders over the same semantic store
reuse the same `SymbolId`s.

`ClassLoader::complete_module` completes that existing `ModuleClass` in
place. It selects the `OBJECT`-marked `ObjectClass` `TypeDef` from the same
TASTy package, publishes its real hierarchy and declared fields/methods in a
`ClassInfo` whose `class` is the canonical module-class symbol, and gives the
object term a `TypeRef` to that same symbol. Member type references establish
only identity shells; only parents and interfaces are eagerly loaded. A
successful completion is cached in the shared session, so repeating it in
this or a later loader keeps the same declaration scope. A failed completion
rolls back the module pair, scope, symbols, and session updates as one unit.
An absent or unresolved declared superclass is reported as malformed TASTy;
module completion does not synthesize a parent.

An ordinary class or trait companion is linked to the object only when that
definition is present in the same TASTy file. The module class is not put in
`SymbolLinks::companion`; its relationship to the object follows their common
package owner and source name. This operation enters identity shells only and
does not complete object members or semantic value types until
`complete_module` is requested. The completion API currently covers top-level
package members; nested objects and identities proved only by JVM classfiles
remain unsupported.

The pinned Scala 3.9 compiler fixtures (TASTy 28.8.0) exercise this path
without a `$`-name heuristic. `dotty.tools.dotc.core.Types`, `Contexts`,
`Symbols`, and `dotty.tools.dotc.parsing.Tokens` each provide an authoritative
package-owned Object/ModuleClass pair. `Contexts` completes through its real
TASTy hierarchy and exposes `withFreshTyperState` through the Object term
prefix; repeated lookup reuses the same declaration. The other three retain
their exact unsupported-parent errors (`TypeUtils`, `SymUtils`, and
`TokensCommon`, respectively), and failed completion rolls back store and
session changes while preserving the already established identity shells.
If a source-owned Object of the same name exists without a session-validated
ModuleClass pair, package-term resolution reports an identity conflict and
leaves the source package, Object, and ModuleClass entries untouched.

## Post-compiler-module Scala 3.9 audit (#1342)

Current classloader and Object ModuleClass corpus measurements, including the
preserved 29-key class gate and row-level #1180 Object correlation, are recorded
in the [post-compiler-module audit checkpoint](typer-classpath-corpus-audit-post-compiler-modules-3.9.0.md).
Object identity remains grounded in semantic TASTy evidence; rolled-back
completion or member attempts are not reported as successful completion.
The checkpoint's final closeout table maps the module failure, caller rollback,
source-owned identity, and same-session retry cases to their regression tests.
