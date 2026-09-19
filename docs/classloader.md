# Compiler Classloader Module

Status: implemented (`crates/dotty-classloader`). `.class` and `.tasty`
classpath entries both load onto `dotty-core`'s canonical semantic model
(`SemanticStore`/`Symbol`/`SymbolId`/`Type`) — this crate no longer keeps a
parallel `Rc<ClassSymbol>` model of its own. §9 lists what each milestone
covers and its current status; §3 and §4.4 list the real, currently-open
gaps (mostly on the `.tasty` side) rather than unimplemented milestones.

Normative sources:
- JVM Specification SE 25, Chapter 4 — [The `class` File Format](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-4.html) (already the basis of `docs/classfile-format-jdk25.md`)
- JVM Specification SE 25, Chapter 5 — [Loading, Linking, and Initializing](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-5.html)
- JLS SE 25, Chapter 12, §12.2 — [Loading of Classes and Interfaces](https://docs.oracle.com/javase/specs/jls/se25/html/jls-12.html)

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
  own ZIP central-directory reader and DEFLATE inflater — no external
  archive dependency.
- Dual-format loading: a classpath entry reports `ClassFormat::Class` or
  `ClassFormat::Tasty` per `ClassResource`; `.tasty` is preferred when
  both exist for the same name.
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
  an unresolved generic type variable, and a dependency's failure
  propagating to its dependent.
- `.tasty`-backed loading converging on the same `Symbol`/`Type::ClassInfo`
  shape as `.class` loading (§4.4), including case-class constructor
  accessor fields.

### Explicitly out of scope
- `module-info.class`, `ACC_MODULE`, and the `Module*` attributes.
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
  rather than ever hard-failing the class load over it.
- **No `.tasty` generics.** A `.tasty`-backed method/class's own type
  parameters are not reconstructed (unlike `.class`'s `Signature`-based
  `Type::Poly`/`SymbolKind::TypeParameter` handling, §4.4).
  `.tasty`-backed `varargs` is always `false` for the same reason
  (`T*` uses a distinct type-tree shape this decoder doesn't model yet).
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
  (`Outer<T>.Inner<U>`) only keeps the last segment's own type
  arguments.** An outer qualifier's type arguments are dropped rather
  than modeled as a fully qualified prefix chain — see
  `ClassLoader::lower_class_type_signature`'s doc comment.

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
shared registry to and from the next adapter.

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
deliberately not reconstructed from `.tasty` yet (precise member types,
generics, annotations).

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

`ClassLoader::load_class(name)` follows that shape:

1. Check the repository: `Loaded` -> return the cached symbol; `Failed`
   -> return the cached error; `Loading` -> a class being its own
   (in)direct supertype is a hard JVMS §5.3.5 error, not a legitimate
   reuse case — reported as `CircularInheritance`, not a silent shell
   reuse. (Reusing an in-progress shell for a *legitimate* mutual
   reference — two classes each having a field/method typed as the
   other, or a nest host and member referencing each other — is a real
   technique this loader does use, via `resolve_member_class`/
   `resolve_semantic_owner`; it's just not what `Loading` on a
   *supertype* edge means.)
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
   it), and resolve+enter its annotations (`.class` only, §3).
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
- `LoadingSession` — bundles a positive-only `BinaryName -> SymbolId`
  map, `PackageRegistry`, and the `ClassfileMetadata`/`ClassOrigin`
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
  `DependencyFailure`, `UnresolvedTypeVariable`; `Clone` (so `Failed`
  entries can be returned repeatedly from the cache; non-`Clone`
  payloads are `Rc`-wrapped to keep the whole enum cheaply cloneable).

`ClassFile`, `TastyFile`, constant-pool indices, and other wire-level
details never appear in this crate's public API surface (`crate::
classloader`, `lib.rs`) — only `BinaryName`, `dotty-core` symbols/types,
and typed errors do.

## 7. Reference material

- [JVMS SE 25, Chapter 4 — The `class` File Format](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-4.html) — already the basis of `docs/classfile-format-jdk25.md`.
- [JVMS SE 25, Chapter 5 — Loading, Linking, and Initializing](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-5.html) — §5.1, §5.3, §5.3.5, §5.4, §5.4.3, §5.4.4 are the relevant sections for this module.
- [JLS SE 25, Chapter 12 §12.2 — Loading of Classes and Interfaces](https://docs.oracle.com/javase/specs/jls/se25/html/jls-12.html) — source-language framing; defers precise semantics to JVMS Chapter 5.
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
- Hand-built minimal class files/synthetic classpaths are sometimes
  unavoidable and acceptable — e.g. a cyclic-inheritance scenario cannot
  be produced by `javac` (rejected at the source level), and a classpath
  combining an in-memory `.tasty` entry with in-memory `.class`
  dependencies needs `CompositeClassPath` over two single-format
  in-memory classpaths. Such fixtures must be clearly marked as
  synthetic, not real compiler output.
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
