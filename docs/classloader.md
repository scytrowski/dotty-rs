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
  After a module has completed successfully once, its immutable decoded
  declarations can also prove that other requested names are absent without
  repeating semantic completion after rollback. Decoding alone does not
  establish this proof: hierarchy loading must have succeeded. The proof
  contains binary names only and is scoped to the resolver's fixed classpath;
  completed modules still use their live declaration scope, including normal
  namespace and ambiguity checks.

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
- **`.tasty` member types remain a bounded subset.** The classloader
  lowers named and applied method parameter/result types, including exact
  address-based references to method-local type parameters. Other shapes
  (tuples, function types, refinements, dependent/path types, aliases, and
  higher-kinded bounds) remain explicit `Type::Error` values rather than
  guesses. A `scala.Any` method bound keeps the canonical Scala top type;
  hierarchy-only reduction to `java.lang.Object` does not apply to member
  types. Unresolvable named dependencies also remain `Type::Error`; they do
  not fail the class load.
- **Class-level `.tasty` type parameters are not reconstructed.** Method-local
  parameters use `Type::Poly`; their supported named/applied bounds are kept,
  and unsupported bounds become explicit error bounds. `.tasty`-backed
  `varargs` is always `false` because `T*` uses a distinct type-tree shape
  this decoder doesn't model yet.
- **No `.tasty` annotations.** `.tasty`-backed symbols always get an
  empty `Symbol::annotations`; annotation decoding exists only for
  `.class` (`annotation.rs`, `ClassLoader::resolve_annotations`/
  `enter_annotations`).
- **A local/anonymous class's owner is its enclosing class, not the
  specific enclosing method** real `dotc` would use. Matching a JVM
  `EnclosingMethod`'s raw `(name, descriptor)` pair back to one of that
  method's own overloads' `SymbolId` would need descriptor-vs-`Type`
  structural matching this loader does nowhere else — see
  `ClassLoader::resolve_semantic_owner`'s doc comment. The raw pair is
  still available via `ClassfileMetadata::enclosing_method`.
- **A `.`-qualified inner-class generic signature
  (`Outer<T>.Inner<U>`) only keeps the last segment's own applied type
  arguments.** The outer qualifier's applied arguments are still dropped
  rather than modeled as a fully qualified prefix chain — see
  `ClassLoader::lower_class_type_signature`'s doc comment. Independently,
  type-variable names in the nested class signature can resolve through the
  enclosing class scopes described above.

## 4. Architecture

```text
                 ClassPath
                    |
        +-----------+---------------+
        v           v               v
 DirectoryClassPath JarClassPath  JdkClassPath/JmodClassPath
        |           |               |
        +-----------+-------+-------+
                            v
                    CompositeClassPath
                            |
                            v
                     ClassRepository            (BinaryName -> Loading/Loaded/Failed)
                            |
                            v
                      ClassLoader
                      +-----+-----+
                      v           v
             dotty-classfile   dotty-tasty
                      |           |
                      +-----+-----+
                            v
             dotty-core::SemanticStore    (Symbol/SymbolId/Type/TypeId/Scope, +
                            |              ClassLoader's own SymbolId-keyed
                            v              ClassfileMetadata sidecar table)
                        Resolver           (field/method/interface-method overload
                                            resolution: not yet built here)
```

Classpath I/O stays decoupled from symbol construction (a
`ClassPathEntry` only ever hands back bytes + origin metadata); symbol
construction stays decoupled from the wire codec (nothing in
`dotty-core`'s `Symbol`/`Type` borrows from the decode buffer — it owns
its data, per the same borrowed/owned separation `AGENTS.md` requires
between decoder and encoder representations).

### 4.1 JVM metadata sidecar

Not every JVM-classfile fact belongs in `dotty-core`'s canonical
semantic model — raw access flags, a class's own raw generic
`Signature`, `NestHost`/`NestMembers`/`PermittedSubclasses`/
`InnerClasses`/`EnclosingMethod`/`Record` components, and annotation
element values (JVMS §4.7.16.1) have no `dotty-core` `Type`/`Symbol`
shape to fit into, and are diagnostic/bytecode-facing rather than
something semantic analysis needs. `ClassLoader` keeps this as a
`SymbolId`-keyed `ClassfileMetadata` table (`ClassLoader::metadata`),
separate from `SemanticStore` — produced only for `.class`-backed
symbols; a `.tasty`-backed symbol has no entry (§4.4).

### 4.2 Packages and ownership

Package identities are session identities, not the classloader's own:
`PackageRegistry` is a thin view over `dotty_core::Packages`, the registry
every adapter shares (see `docs/dotty-core-design.md` §9.1). One
term-named `SymbolKind::Package` symbol exists per package *segment*
(`java/util` is `<root> -> java -> util`, three symbols), reused across
repeated loads and across adapters: a path the TASTy unpickler entered is the
same `SymbolId` here. Each package symbol's `owner` is its immediately
enclosing package, its own `name` is just its segment, and it is declared in
its owner's scope; the full path is only ever reconstructed by walking
`owner`. The unnamed package is the explicit root (empty name,
`owner: None`), which owns every top-level package and every top-level class
with no package. `LoadingSession::with_packages` / `into_packages` hand the
shared registry to and from the next adapter. Each loaded binary class is
also entered under its type name in the scope of the package from its binary
name. This package-scope index is separate from `Symbol::owner`: a nested
class can be semantically owned by its enclosing class while remaining
discoverable by its binary name through the package scope.

Classpath evidence and package identity are separate contracts:
`contains_package_prefix` says only that a usable class/TASTy resource exists
at or below a logical path. When that evidence exists for the full requested
path, the resolver may enter missing intermediate package segments through the
shared registry. A direct classpath resource is not required at each ancestor.
These package symbols remain logical namespace entries; they do not imply a
class, object, or package-object term. Existing source/TASTy package identities
and their members are reused. New segments are transactional: missing evidence
creates no partial chain, and resolver or caller rollback removes newly entered
segments together with their scope links.

### 4.3 Nesting

A class's `Symbol::owner` is the package it's declared in for a
top-level class, or the real enclosing class for a nested/local/
anonymous one — reconstructed from its own class file's `InnerClasses`
self-entry (JVMS §4.7.6 requires one) when it names an `outer_class`, or
its `EnclosingMethod` attribute's `class` otherwise (see the known gap
in §3 for why this stops at the enclosing class, not the enclosing
method). `Visibility::Package` is computed once, from the real package,
and is never repointed at the enclosing class — a package-private
nested class is visible package-wide, not only from within its
enclosing class. `.tasty` files carry neither attribute, so a
`.tasty`-backed nested class's owner is still just its package (§4.4).

### 4.5 Shared-store symbol resolver

`ClasspathSymbolResolver<E>` is the classloader implementation of
`dotty_core::SymbolResolver`. The frontend bootstraps `Definitions` once,
then passes the same `SemanticStore`, `Definitions`, and `LoadingSession`
through resolver instances. A resolver can hand its session to the next
instance with `into_session`; the returned `SymbolId`s and package registry
therefore remain canonical across classpath roots and frontend phases.

Package paths are reused when already present in the shared `Packages`
registry. A new path is entered only when its classpath reports a direct
`.class` or `.tasty` resource in that package. Member lookup understands
package and class `TypeRef`s, stable `TermRef`s to validated external Scala
objects, `Applied` references, and class/package `ThisType`s. `MemberSpace::Explicit`
chooses its declaring type as the lookup scope; names retain their type or
term namespace, and a `Unique` request with multiple declarations returns
`ResolutionError::Ambiguous`. The resolver returns declaration symbols only;
inherited lookup, receiver adaptation, and overload ranking stay with the
caller. Unsupported prefix forms return `None`.

A unique type-name request whose prefix is a package `TypeRef` derives the
binary class name from the canonical package owner chain and asks the loader
to materialize that class. Success returns the class's canonical symbol only
after it has `Complete(Type::ClassInfo)` and a valid declaration scope. The
same `LoadingSession` shares its canonical package registry and positive
class cache across package lookup, class dependencies, and later member
lookup, so repeated requests return the same `SymbolId`. Package term
requests materialize a canonical Object/ModuleClass pair only when the exact
type-side TASTy proves a top-level object. A `.class` file for `Foo` or a
resource named `Foo$` alone does not establish module identity. An object
prefix completes its ModuleClass on demand and looks up declarations in that
class's `ClassInfo`; a missing or ambiguous member rolls back completion
performed by that request. Package term and member observations are available
by opting in with `with_module_resolution_profile_collection()` and draining
`take_module_resolution_profile()`. Collection is disabled by default to avoid
adding audit work to ordinary member resolution. Sorted target keys use binary
names and namespaces rather than allocated symbol IDs. The counters
distinguish unresolved requests, malformed evidence, newly materialized and
reused objects, module completion, and non-class member successes.
`resolved_object_targets` and `resolved_member_targets` contain only keys that
resolved to canonical Object and declared member symbols, respectively;
`package_term_targets` and `member_targets` also include misses. Corpus reports
can therefore report distinct successful objects and members without counting
attempts as successes.

The profile preserves a separate channel from the frozen #1046 source audit:
the baseline remains 2,749 package-term observations across 80 request keys,
including 2,196 observations across 46 keys with companion resource evidence.
The resolver profile reports actual outcomes for later focused or corpus runs;
it does not rewrite those frozen observations.

The checked-in resolver regressions exercise a real JDK 26
`java.time.DayOfWeek` classfile and a real Scala 3.9 `scala.io.Source` TASTy
file through package type requests. They check completed class identity,
scope and repeated-request reuse, then reach declared members (`getValue`,
`getLines`). The source-typing integration also resolves inherited
`Source.isEmpty` through the existing parent lookup path. Synthetic JDK
dependency stubs keep the JDK fixture independent of a local JDK install;
the materialized `DayOfWeek` bytes themselves are the checked-in JDK class.

Every resolver call is atomic. A miss or loading error removes newly loaded
classes, session cache entries, package entries, and package-scope links.
Successful materializations are journaled so the frontend can call
`rollback_to` before its paired `SemanticStore::rollback_to`.

Corpus audits can opt in to `with_load_profile_collection()` on the resolver.
Each observed top-level class load reports its root binary name, maximum
dependency depth, and sorted distinct class names reached through superclass,
interface, member-type, signature, and annotation loading. This profile
includes internal dependency loads but does not increment the top-level
resolver request success counters. Collection is disabled by default; callers
drain observations with `take_load_profiles()`. Profiles describe attempted
work, including a load later rolled back by an enclosing typer transaction,
and are kept separate from semantic session state.

In the frozen #976 Scala 3.9/JDK 21 source audit, 958 top-level class loads had a
median dependency depth of 0, p95 of 140, and maximum of 141. One request
touched 1,048 distinct classes; the largest roots were repeated
`dotty/tools/io/AbstractFile` requests from classpath and TASTy code. The
distribution is strongly skewed: most roots did no nested loading, while a
small tail eagerly traversed large dependency graphs. #1019–#1023 reduced
p95 completion depth to 3, maximum depth to 4, and maximum completed fanout
to 8, while retaining 358 external class successes on 29 canonical keys.
The [hardening report](typer-deferred-classpath-hardening-3.9.0.md) retains
separate shell and completion measurements. Package term/object
materialization remains a separate identity boundary; a discovered `Foo$`
resource is not itself sufficient to construct a canonical module symbol.
The full measurements and the single next-sprint recommendation are recorded
in [`typer-classpath-corpus-audit-3.9.0.md`](typer-classpath-corpus-audit-3.9.0.md).

### 4.4 `.tasty` convergence

`tasty_symbol::decode` reconstructs a `DecodedTastyClass` (flags,
superclass, interfaces, fields, methods) from a `.tasty` file's
`TypeDef`/`Template`, without full type-checking. `ClassLoader::
load_uncached_tasty` enters it the same enter-before-complete way as
`.class` loading, converging on the identical `Symbol`/`Type::ClassInfo`
shape — reusing the very same format-agnostic `enter_field`/
`enter_method` (built for `.class`, unmodified for `.tasty`) once
`.tasty`'s own modifiers are translated into the same JVM-shaped
`FieldAccessFlags`/`MethodAccessFlags` `.class` decoding produces.
Case-class constructor `val`/`var` parameters are extracted from the
primary constructor's own `term_params` (tagged `CASEACCESSOR_TAG`/
`FIELDACCESSOR_TAG`) rather than `Template.stats`, since Scala's pickler
never duplicates them there. See §3's "known gaps" for what's
deliberately not reconstructed from `.tasty` yet (class type parameters,
unsupported member type shapes, annotations).

For `DefDef` methods, TASTy clause presence is retained independently of the
flattened JVM parameter list. A no-term-clause definition lowers as
`Type::ByName` (TASTy's `ExprType`); an explicit empty `()` clause remains
`Type::Method` with no parameters. A `.class` descriptor cannot distinguish
those source forms and continues to lower a zero-argument method as an empty
`Type::Method`.

Parent-name recovery follows the compiled TASTy reference tree. Qualified,
root-qualified, applied, shared, and direct symbol references retain the
package or owner encoded by that tree; an applied parent's binary identity is
read from its type constructor. For a TASTy type selection whose qualifier is
an AST reference to an owner, the selected nested type keeps that owner as
part of its binary identity. Constructor `SelectIn` wrappers then resolve to
the selected type itself, rather than adding a constructor name. Signed
selection names use their source-level original spelling; signature data is
not treated as a class name. This resolves the pinned compiler
`Trees.Instance[Type]` parent used by both `tpd$` and `untpd$` as
`Trees$Instance`, while keeping type arguments outside this hierarchy
projection. AST-child and identifier wrappers around semantic references are
traversed with the same depth bound, and a repeated `SelectIn` name is not
appended when its AST-child qualifier already proves that exact final
segment. Shared references are followed through their TASTy address with the
same depth bound as other reference chains. Known
Scala aliases such as `scala.AnyRef` still normalize to their canonical JVM
identity. A semantic reference with an unreadable prefix remains
`UnresolvedSupertype`, except for the separately validated Scala package-object
alias path. For a parent tree with no semantic reference, the existing bounded
classpath check considers the current package, `scala`, and `java.lang`, and
accepts a result only when exactly one classpath resource matches. The pinned
Scala 3.9.0 audit records each residual parent's reference flags, visible names,
and compact raw-tree tag shape.

The applied nested constructor change is grounded in the frozen #1412 roots:

| Requested root | Frozen unresolved-parent observations | Parent index | Pinned TASTy result |
| --- | ---: | ---: | --- |
| `dotty/tools/dotc/ast/tpd$` | 1,715 | 0 | `dotty/tools/dotc/ast/Trees$Instance` |
| `dotty/tools/dotc/ast/untpd$` | 141 | 0 | `dotty/tools/dotc/ast/Trees$Instance` |

The focused tests verify those names from the real compiler `tpd.tasty` and
`untpd.tasty` trees and independently reconstruct the nested `Instance`
owner chain from `Trees.tasty`. These counts identify the affected frozen
roots; they do not claim that all weighted observations now complete
successfully through class loading.

## 5. Loading algorithm (enter before complete)

JVMS §5.4 explicitly allows deferring resolution: *"an implementation may
choose a 'lazy' linkage strategy, where each symbolic reference... is
resolved individually when it is used."* Scala 3's own loader
(`SymbolLoaders.scala`) is built the same way: a symbol is entered into
scope with a completer attached before that completer's `doComplete`
actually runs, which is what lets mutually-referencing classes load
without infinite recursion. `dotty-core`'s own model follows the same
shape: `SymbolInfo::Missing` until a class's supertypes/members are
known, then `SymbolInfo::Complete(TypeId)` pointing at its
`Type::ClassInfo`.

Callers that only need a stable external class identity can use
`ClassLoader::ensure_class_identity(name)`. It reads the selected resource
and decodes only the class kind and visibility (including nested-class
access from the `InnerClasses` self-entry), enters the symbol in its
canonical package scope, and leaves `SymbolInfo::Missing`. It does not
resolve supertypes, member types, or class members. JVM nested ownership is
followed only when the class file's `InnerClasses` or `EnclosingMethod`
metadata names an enclosing class; `$` in the binary name is not enough.
The session tracks these identified shells separately from fully resolved
classes. A later `load_class` reuses and completes the same `SymbolId`.
Failed identity establishment rolls back newly entered symbols, packages,
and session metadata.

The [#1023 hardening report](typer-deferred-classpath-hardening-3.9.0.md)
freezes lifecycle states, permanent regression coverage, metric definitions,
and the measured before/after dependency gate.

The dependency policy is explicit:

| Reference role | Policy |
| --- | --- |
| Direct superclass/interface or TASTy trait parent | Eager completion |
| Field or record component value type | Identity only |
| Method parameter/result, including object arrays | Identity only |
| Generic constructor/argument, class/method type parameter bound, wildcard bound | Identity only |
| Annotation type | Identity only; unresolved identity retains the existing annotation fallback |
| Best-effort TASTy member type | Identity only; missing/unreducible types retain focused `Type::Error` |
| Static member/local/anonymous enclosing class | Identity only |
| Non-static member-class enclosing class | Eager completion for its declaration/type-parameter scope |

A completed `ClassInfo` can therefore contain member `TypeRef`s whose target
class still has `SymbolInfo::Missing`. SourceTyper requests `SymbolResolver::complete_class` when member lookup
needs an external shell's `ClassInfo` (#1021). Direct resolver member requests also complete owned Missing class prefixes
before inspecting declarations (#1022). This bounded
hook does not introduce the general `CompletionId`/completer engine. Non-static enclosing owners remain eager because the
current #973 type-variable environment consumes their declaration scopes;
identity alone cannot resolve an outer generic parameter. Static edges still
stop that lookup.

Classfile members naming a missing/malformed class still fail with
`DependencyFailure`; class existence must be established by the classpath.
The special canonical `Definitions` identities remain shared. Public full-load
failures roll back new symbols, package-scope entries, types, and positive
session state, including member shells created earlier in that load. Previous
shells and their provenance survive, and prior symbol state is restored before
store allocations are freed. Loader-local failures retain their typed artifact
context with no freed symbol IDs; a new loader sharing the session can retry.

Opt-in `ClassLoadProfile` records distinct touched identities and eager load
depth, plus `full_completions`, `shells_created`, `shells_reused`,
`member_type_references`, and `hierarchy_loads`. `completed_classes` records
successful full loads, `identity_shells` records incomplete identities observed,
`completed_cache_hits` counts completed-session reuse, and `root_was_shell`
identifies later on-demand root completion. Completions exclude cache hits;
shell reuse counts incomplete external identities, not completed classes or
builtins. Member references count successful descriptor/signature/annotation
identity requests; hierarchy loads count superclass/interface requests,
including cache hits or errors. The opt-in `dependency_edges` groups directed
owner-to-dependency requests by `Hierarchy`, `MemberTypeIdentity`,
`EnclosingOwnerIdentity`, or `EnclosingOwnerCompletion`; edge request counts
include repeated visits and failed requests. Counts report work done even if
the root later rolls it back. The corpus audit emits totals and per-request
counts for #1023.

The classpath completion hook accepts only identities registered in the same
`LoadingSession` and backed by classfile/TASTy origins. It recovers the canonical
binary name, validates package/name/owner identity, and calls `load_class` for
`Missing` info. Already-complete classes are validated and reused without byte
reads or duplicate declarations. Error/Deferred or malformed info produces a
focused `ResolutionError`; unowned source/synthetic classes and unbacked
builtins are declined atomically. Successful completion retains the same
`SymbolId` and joins the resolver journal. A later caller rollback restores a
pre-existing shell's entire previous symbol and removes new members/scopes,
metadata, and completed-cache entries. A shell introduced inside that caller
transaction is removed by the matching store rollback instead. Session handoff
between a loader and resolver preserves both forms of identity.

Direct class-prefix requests accept `TypeRef`, `Applied` over a class reference,
and `ThisType`. `MemberSpace::Explicit(owner)` completes and searches only the
explicit owner, regardless of the displayed prefix, and never falls back.
Returned symbols are declarations; receiver/generic adaptation stays with the
caller. The declaration scope contains decoded fields/methods/constructors and
supported type parameters. `InnerClasses` metadata does not manufacture nested
class declarations; nested classes absent from an owner's semantic scope can
be loaded via `ClassLoader::load_class` with their exact JVM binary name.
The existing package/type policy is preserved, including its refusal to
replace a pre-existing incomplete package entry.

Prefix completion runs inside the member request's existing transaction,
without creating a nested completion transaction. An absent member returns
`Ok(None)` and undoes completion started solely by that call. A Unique request
with several declarations returns `Ambiguous` and likewise restores the shell,
new scopes, members, dependency identities, and positive caches. A missing or
malformed artifact returns a focused resolver error, never member absence.
Pre-existing complete prefixes remain complete on absent/ambiguous requests;
repeated successful calls reuse member IDs without rereading class bytes.
Package type requests also complete an existing owned external shell before
returning its canonical class symbol (#1023). Unowned/source Missing classes,
builtin aliases, and unresolved package terms without positive TASTy module
evidence retain their contracts.

`ClassLoader::load_class(name)` follows that shape:

1. Resolve `scala.Any`, `scala.AnyRef`, and `scala.Nothing` to their
   canonical `Definitions` identities before classpath lookup. Otherwise,
   check the repository: `Loaded` -> return the cached symbol; `Failed`
   -> return the cached error. On `Loading`, consult the explicit current
   superclass/interface ancestry: a name already on that ancestry is a
   genuine cycle and reports `CircularInheritance`; an in-progress symbol
   outside it may be reused as a dependency reached through a member type
   (for example Object -> member Class -> superclass Object). Mutual
   member references reuse their canonical identity through
   `ensure_dependency_identity`; owner resolution follows the policy above.
   The inheritance ancestry is capped at 256 classes; exceeding it reports
   `InheritanceDepthExceeded` rather than recursing without bound.
2. Ask the classpath for the bytes; not found -> `NotFound` (cached).
   The classpath also decides `.class` vs `.tasty` (`ClassFormat`) —
   preferring `.tasty` when both exist.
3. Decode with `ClassFile::decode` or `tasty_symbol::decode`; failure ->
   `InvalidClassFile`/`InvalidTastyFile` (cached).
4. For `.class`, resolve `this_class` and compare it to the requested
   name (JVMS §5.3.5's name check) -> `NameMismatch` on drift.
5. Build the symbol shell (`Symbol`, `SymbolInfo::Missing`) and its
   declarations `Scope`, and mark it `Loading` in the repository —
   visible to any recursive call from this point on. For `.class`, also
   patch `Symbol::owner` to the real enclosing class here, if
   `InnerClasses`/`EnclosingMethod` names one (§4.3) — early enough that
   a mutual reference back to this class while resolving *that* owner
   still sees the in-progress shell.
6. For `.class`, enter this class's own generic type parameters
   (`SymbolKind::TypeParameter`) before resolving anything that might
   reference them — enter-before-complete one level deeper, so a
   F-bounded/mutually-referential bound (`<T extends Comparable<T>>`)
   resolves regardless of declaration order.
7. Resolve `super_class` and each `interfaces` entry, recursing into
   `load_class`. Any failure here is wrapped as `DependencyFailure {
   owner, dependency, source }`.
8. Decode and enter fields/methods/constructors as real symbols, lowering
   each one's descriptor (preferring a `Signature` attribute's generic
   type over the erased descriptor type when present and consistent with
   it), using identity shells for referenced classes, and resolve+enter
   its annotations (`.class` only, §3).
9. Complete the class: build its `Type::ClassInfo` (parents + the
   declarations `Scope`) and set `SymbolInfo::Complete` on it.
10. Mark `Loaded`, return the symbol.

## 6. Key types

- `BinaryName` — internal-form owned name; conversions from/to qualified
  (dotted) form; no `$` parsing (§4.3 covers nesting *ownership*
  separately from the name itself).
- `ClassPathEntry` (trait) — `find_class(&BinaryName) ->
  Result<Option<ClassResource>, ClassPathError>`; `Send + Sync`.
  Implementors: `DirectoryClassPath`, `JarClassPath`, `JdkClassPath`,
  `JmodClassPath`, `CompositeClassPath` (+ an in-memory test-only
  implementation used throughout this crate's own unit tests).
- `ClassResource` / `ClassOrigin` / `ClassFormat` — bytes, where they
  came from (directory/JAR/JMOD path, for diagnostics and
  duplicate-class detection), and which decoder (`.class`/`.tasty`)
  they need.
- `JdkModuleCatalog` / `JdkModuleDescriptor` — explicit, owned JPMS metadata
  from validated JMOD module-info class files; directive names use dotted
  Java spelling and retain class-file order. `JdkModuleCatalog::get` looks up
  the descriptor's authoritative module name. These types are not used by
  `ClassLoader`, `ModuleIdentity`, or the Scala semantic model.
- `ClassRepository` (crate-private) — the state machine described in §5,
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
   for `.class`; `.tasty` supports a bounded method-generic subset (§3).
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
   fields/methods (§4.4); method generics and named/applied method types
   are supported for a bounded subset, while class type parameters,
   unsupported member shapes, and annotations remain unbuilt (§3).

### Open work (not milestone-numbered — tracked here instead)
- Broader `.tasty` member type resolution beyond the named/applied method
  subset, including class type parameters and annotations.
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
prefix; repeated lookup reuses the same declaration. The `Types`, `Symbols`,
and `Tokens` module classes also complete through their exact package-owned
parents (`TypeUtils`, `SymUtils`, and `TokensCommon`). Failed completion still
rolls back store and session changes while preserving already established
identity shells.
If a source-owned Object of the same name exists without a session-validated
ModuleClass pair, package-term resolution reports an identity conflict and
leaves the source package, Object, and ModuleClass entries untouched.

The focused #1415 gate uses the frozen #1412 roots to distinguish a completed
module from a decoded parent or an entered identity shell:

| Real compiler root | Frozen unresolved-parent observations | Current completion/member result |
| --- | ---: | --- |
| `dotty/tools/dotc/core/Symbols$` | 3,652 | `Symbols$` completes in place with the TASTy parent `SymUtils`; `NoSymbol` resolves twice through the Object `TermRef` to the same TASTy-owned field. |
| `dotty/tools/dotc/core/Types$` | 2,545 | `Types$` completes in place with its pinned `TypeUtils` parent. |
| `dotty/tools/dotc/ast/tpd$` | 1,715 | The selected parent resolves to `Trees$Instance`, but completion rolls back because this fixture set has no independent `Trees$Instance` resource. The typed missing-class failure is stable on retry. |

The `Symbols$` gate also checks that the canonical Object and ModuleClass
remain distinct symbols in the package's Term and Type namespaces, that the
completed ClassInfo retains the ModuleClass ID and ordered TASTy parent, and
that repeated member lookup adds no new declarations. `tpd$` remains an
explicit residual; the test does not synthesize a binary identity for the
nested `Trees.Instance` type. The complete frozen-root profile records other
downstream dependency failures separately from this direct module gate.

## Post-compiler-module Scala 3.9 audit (#1342)

Current classloader and Object ModuleClass corpus measurements, including the
preserved 29-key class gate and row-level #1180 Object correlation, are recorded
in the [post-compiler-module audit checkpoint](typer-classpath-corpus-audit-post-compiler-modules-3.9.0.md).
Object identity remains grounded in semantic TASTy evidence; rolled-back
completion or member attempts are not reported as successful completion.
The checkpoint's final closeout table maps the module failure, caller rollback,
source-owned identity, and same-session retry cases to their regression tests.

## Final TASTy-parent corpus gate (#1417)

The [checkpoint](typer-classpath-corpus-audit-post-tasty-parents-3.9.0.md)
keeps the #1342 compiler/library roots frozen and adds a current parent and
hierarchy profile. Its deterministic join retains the original failed parent
index, current binary identity, complete hierarchy-load cause, source-file
observations, and newly surfaced roots. A resolved parent name is not counted
as a completed `ClassInfo`; module completion and selected-member outcomes
remain separate evidence.

The manual hosted workflow runs two serial full audits and compares the new
root/file provenance sidecar with the normalized reports and historical cohort
sidecars. The current hard limits are completed dependency p95 <= 6 and maximum
completed-class fanout <= 16. The historic #1342 root weights total 8,949,
while their separate source-attribution sidecar totals 8,948; this mismatch is
explicitly retained pending the final reconciliation. No current full-corpus
measurement or recommendation is claimed until that workflow succeeds.
