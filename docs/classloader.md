# Compiler Classloader Module

Status: working document — scope and architecture for the upcoming
`dotty-classloader` crate. No implementation exists yet.

Normative sources:
- JVM Specification SE 25, Chapter 4 — [The `class` File Format](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-4.html) (already the basis of `docs/classfile-format-jdk25.md`)
- JVM Specification SE 25, Chapter 5 — [Loading, Linking, and Initializing](https://docs.oracle.com/javase/specs/jvms/se25/html/jvms-5.html)
- JLS SE 25, Chapter 12, §12.2 — [Loading of Classes and Interfaces](https://docs.oracle.com/javase/specs/jls/se25/html/jls-12.html)

Where this document is silent, the JVMS is authoritative, per the same
policy as `docs/classfile-format-jdk25.md`.

## 1. Goal and non-goal

Goal: a compiler-frontend class loader — locate classpath entries, decode
`.class` files via the existing `dotty-classfile` codec (and, later,
`.tasty` files via `dotty-tasty`), construct semantic class symbols, and
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
symbol creation                                    <- this crate
    |
lazy symbol resolution                             <- this crate
    |
dotty-rs semantic model                            <- future crate(s)
```

`dotty-classfile` already covers descriptor parsing (JVMS §4.3.2/4.3.3),
generic `Signature` grammar (JVMS §4.7.9), and the attributes needed for
symbol-level information (`Signature`, `InnerClasses`, `EnclosingMethod`,
`NestHost`/`NestMembers`, `Record`, `PermittedSubclasses`, `Exceptions`,
annotations, `MethodParameters` — see `docs/classfile-format-jdk25.md`
§7). `dotty-classloader` does not re-implement any of that; it consumes
`ClassFile::decode` output directly. Its own job starts one layer up:
classpath discovery, the enter/complete symbol lifecycle, and reference
resolution.

## 3. Scope

### In scope for the initial implementation
- A canonical binary class name representation (`BinaryName`),
  internal-form (`java/lang/Object`), matching `docs/
  classfile-format-jdk25.md` §8. `$` stays a naming convention, not a
  parsed structure — nesting is reconstructed from `InnerClasses`/
  `EnclosingMethod`, not string-split.
- An open classpath abstraction (`ClassPathEntry` trait) so filesystem
  directories, archives, and other sources can all plug in without
  changing the trait.
- A directory-backed classpath entry and a composite (ordered,
  first-match-wins) classpath.
- A `.class`-only loading pipeline: locate bytes -> decode via
  `dotty-classfile` -> build a class symbol (name, access flags,
  superclass, interfaces) -> cache it.
- A repository/cache keyed by `BinaryName` with explicit states (missing /
  loading / loaded / failed) so a class already being completed is visible
  to whatever references it next, instead of being reloaded from scratch
  — this is what makes classes that reference each other safe to load
  (see §5).
- A structured error model distinguishing not-found, I/O failure,
  malformed class file, name mismatch, circular inheritance, and a
  dependency's failure propagating to its dependent.

### Planned, but not part of the initial implementation
- JAR-backed and JMOD-backed classpath entries (including multi-release
  JAR version selection), and a JDK classpath built from
  `$JAVA_HOME/jmods`.
- Member symbols (fields, methods, constructors) and JVM descriptor ->
  semantic type reconstruction (the descriptor/signature *parsing*
  already exists in `dotty-classfile`; turning it into semantic types for
  members is new work here).
- A dedicated generic-signature-to-semantic-type reconstruction step
  built on `dotty-classfile::signature`.
- Explicit field/method/interface-method resolution (JVMS
  §5.4.3.2-5.4.3.4), as APIs distinct from class loading.
- A `.tasty`-backed loader converging on the same symbol model as the
  `.class` loader (architecture in §4), reusing `dotty-tasty` as-is.
- Nested-class semantic reconstruction from `InnerClasses`/
  `EnclosingMethod`/`NestHost`/`NestMembers`, beyond just preserving the
  flat binary name.

### Explicitly out of scope
- `module-info.class`, `ACC_MODULE`, and the `Module*` attributes.
- Bytecode verification, `StackMapTable` type-checking,
  `invokedynamic`/`CONSTANT_Dynamic` call-site resolution.
- Any part of the JVM runtime lifecycle listed in §1's non-goal.

(These mirror `docs/classfile-format-jdk25.md` §9, which already excludes
them at the codec layer.)

## 4. Architecture

```text
                 ClassPath
                    |
        +-----------+---------------+
        v           v               v
 DirectoryClassPath JarClassPath  JdkClassPath/JmodClassPath   (Jar/Jmod: planned)
        |           |               |
        +-----------+-------+-------+
                            v
                    CompositeClassPath
                            |
                            v
                     ClassRepository            (BinaryName -> Missing/Loading/Loaded/Failed)
                            |
                            v
                      ClassLoader
                      +-----+-----+
                      v           v
             dotty-classfile   dotty-tasty        (.tasty side: planned)
                      |           |
                      +-----+-----+
                            v
                    Symbols / ClassRef
                            |
                            v
                        Resolver                  (field/method/interface-method resolution: planned)
```

Classpath I/O stays decoupled from symbol construction (a
`ClassPathEntry` only ever hands back bytes + origin metadata); symbol
construction stays decoupled from the wire codec (`ClassSymbol` never
borrows from the decode buffer — it owns its data, per the same
borrowed/owned separation `AGENTS.md` requires between decoder and
encoder representations).

## 5. Loading algorithm (enter before complete)

JVMS §5.4 explicitly allows deferring resolution: *"an implementation may
choose a 'lazy' linkage strategy, where each symbolic reference... is
resolved individually when it is used."* Scala 3's own loader
(`SymbolLoaders.scala`) is built the same way: a symbol is entered into
scope with a completer attached before that completer's `doComplete`
actually runs, which is what lets mutually-referencing classes load
without infinite recursion.

`ClassLoader::load_class(name)` follows that shape:

1. Check the repository: `Loaded` -> return the cached symbol; `Failed`
   -> return the cached error; `Loading` -> the only reason `load_class`
   recurses into itself in the initial implementation is resolving a
   superclass or interface, and a class being its own (in)direct
   supertype is a hard JVMS §5.3.5 error, not a legitimate reuse case —
   so this becomes `CircularInheritance`, not a silent shell reuse.
   (Reusing an in-progress shell for a *legitimate* mutual reference is a
   real technique — Scala 3 uses it for field/method types — but that
   need only appears once member types are resolved eagerly too, which is
   a later increment.)
2. Ask the classpath for the bytes; not found -> `NotFound` (cached).
3. Decode with `dotty_classfile::class_file::ClassFile::decode`; failure
   -> `InvalidClassFile` (cached).
4. Resolve `this_class` and compare it to the requested name (JVMS
   §5.3.5's name check) -> `NameMismatch` on drift.
5. Build the symbol shell and mark it `Loading` in the repository —
   visible to any recursive call from this point on.
6. Resolve `super_class` and each `interfaces` entry, recursing into
   `load_class`. Any failure here is wrapped as `DependencyFailure {
   owner, dependency, source }`.
7. Mark `Loaded`, return the symbol.

## 6. Key types (sketch, not final signatures)

- `BinaryName` — internal-form owned name; conversions from/to qualified
  (dotted) form; no `$` parsing.
- `ClassPathEntry` (trait) — `find_class(&BinaryName) -> Result<Option<ClassResource>, ClassPathError>`.
  `Send + Sync` from the start (classpath sharing across compilation
  threads is expected later; adding the bound after the fact would break
  every implementor).
- `ClassResource` / `ClassOrigin` — bytes plus where they came from
  (directory path today; JAR/JMOD entry later), kept for diagnostics and
  duplicate-class detection.
- `ClassRepository` (crate-private) — the state machine described in §5.
- `ClassSymbol` — name, access flags (reusing
  `dotty_classfile::access_flags::ClassAccessFlags`), superclass and
  interfaces as `ClassRef`.
- `ClassRef` — `Unresolved(BinaryName)` / `Resolved(Rc<ClassSymbol>)`.
  Superclass/interfaces are always `Resolved` once loading succeeds
  today; the `Unresolved` case exists for when member types
  (fields/methods) start being resolved lazily, so this shape doesn't
  need to change later.
- `ClassLoadError` — `NotFound`, `Io`, `InvalidClassFile`, `NameMismatch`,
  `CircularInheritance`, `DependencyFailure`; `Clone` (so `Failed`
  entries can be returned repeatedly from the cache).

`ClassFile`, constant-pool indices, and other wire-level details never
appear in this API surface — only `BinaryName`, symbols, and typed errors
do.

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

## 8. Testing approach

Follows the same policy as the rest of the workspace (`AGENTS.md`):
focused unit tests per behavior, integration tests against real fixtures,
exact error variants asserted rather than `is_err()`.

- Reuse the existing real JDK 25 fixtures under
  `crates/dotty-classfile/tests/fixtures/` where they exercise
  loader-relevant shapes (e.g. `pool_sample/PoolSample.class` implements
  an interface and has a superclass) instead of generating new ones for
  cases already covered there.
- Hand-built minimal class files are sometimes unavoidable and
  acceptable — e.g. a cyclic-inheritance scenario cannot be produced by
  `javac` (it's rejected at the source level) but the loader must still
  handle it without infinite recursion; such fixtures must be clearly
  marked as synthetic, not JDK output.
- Differential testing against `javap -verbose` on the same fixture
  remains valuable once member/generic reconstruction lands, mirroring
  the practice already established for `dotty-classfile`.

## 9. Milestones

1. `BinaryName`, `ClassPathEntry`, `DirectoryClassPath`,
   `CompositeClassPath`, `ClassRepository`, a `.class`-only `ClassLoader`
   producing `ClassSymbol`s with name/flags/superclass/interfaces,
   including circular-inheritance detection.
2. JAR-backed classpath entries.
3. JDK classpath via `$JAVA_HOME/jmods`.
4. Member symbols (fields/methods/constructors) built from the
   descriptors `dotty-classfile` already parses.
5. Generic-signature-based semantic types, built from the `Signature`
   grammar `dotty-classfile` already parses.
6. Lazy reference resolution for member types (the point where reusing
   an in-progress `Loading` shell for a *legitimate* mutual reference,
   not just detecting a cycle, becomes necessary).
7. Nested-class and remaining semantic-attribute reconstruction
   (`InnerClasses`, `EnclosingMethod`, `NestHost`/`NestMembers`,
   `Record`, `PermittedSubclasses`, annotations).
8. Multi-release JAR version selection.
9. `.tasty`-backed loading, converging on the same symbol model as
   `.class` loading.

This ordering keeps every milestone independently testable against real
fixtures before the next one is attempted.
