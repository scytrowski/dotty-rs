# Classloader Error Taxonomy and Provenance Plan

Status: error and provenance audit from #979, with the additive API chosen for
#980. This document describes the current error model and its compatibility
constraints; it does not change loading or classpath selection.

## Scope and evidence

The audit covers `ClassLoadError`, the private causes inside public
`ClassPathError`, `TastyDecodeError`, error conversion and source chains,
`ClassRepository` caching, and the path from a `ClassResource` to a failed
load. The implementation references below name the owning functions and
variants so they remain useful if line numbers move.

Primary evidence is in:

- `crates/dotty-classloader/src/error.rs` — public `ClassLoadError`,
  `Display`, `Error::source`, and existing error tests;
- `src/loader.rs` — `load_class_with_inheritance_ancestry`, `load_uncached`,
  `load_uncached_class`, `load_uncached_tasty`, `resolve_name`,
  `load_dependency`, and `resolve_member_class`;
- `src/class_path.rs`, `src/jar_class_path.rs`, and `src/jmod_class_path.rs`
  — lookup outcomes, resource formats and origins;
- `src/repository.rs` and `src/session.rs` — failure cache and shared
  successful symbol metadata;
- `src/tasty_symbol.rs` — `TastyDecodeError` and TASTy class projection;
- `src/resolver.rs` — missing-class treatment and transactional rollback;
- `src/zip_archive.rs` and `src/zip_reader.rs` — archive and entry failures.

## Category meanings

The classifications below describe the primary cause represented by a
variant. They are not claims that every instance proves the bytes themselves
are corrupt:

- **Missing** — lookup completed successfully and no resource was present.
- **MalformedArtifact** — the located bytes or their declared class identity
  violate a format or class-loading invariant.
- **UnsupportedFeatureOrVersion** — the bytes may be valid, but exceed a
  supported version, shape, or explicit implementation bound.
- **ResolutionOrDependency** — another class or a name needed for projection
  could not be resolved. Keep the outer dependency wrapper distinct from the
  cause that made resolution fail.
- **IoOrEnvironment** — filesystem or archive access failed. The current
  `Io` wrapper is broader than this label in a few cases, as noted below.
- **InternalInvariant** — trusted compiler-owned state is inconsistent. No
  current `ClassLoadError` variant proves this category on its own; it is
  useful for future typed causes such as invalid resolver state.
- **Conditional** — the retained payload does not distinguish malformed input
  from an unsupported or unresolved case. Do not infer the distinction from
  `Display` text.

## `ClassLoadError` inventory

| Variant | Primary classification | Evidence and retained data |
| --- | --- | --- |
| `NotFound(BinaryName)` | `Missing` | `loader::load_uncached` creates it only after `ClassPathEntry::find_class` returns `Ok(None)`. It retains the requested name. An entry's `Ok(None)` is distinct from an I/O error. `resolver::resolve_target` and `resolve_package_inner` translate this cause to `Ok(None)`. |
| `Io(BinaryName, Rc<ClassPathError>)` | `IoOrEnvironment`, conditionally malformed path/archive | `loader::load_uncached` wraps a failing `find_class`. The name and typed source survive, but the source does not expose the failing classpath entry or attempted format. `ClassPathError::InvalidBinaryName` is not an environmental I/O failure; ZIP parsing can mean a malformed archive or an unsupported ZIP feature. |
| `InvalidClassFile(BinaryName, ClassFileError)` | `Conditional` | `loader::load_uncached_class` maps `ClassFile::decode` failures here. The requested name and typed codec error survive. Read/header/constant-pool/attribute format violations are `MalformedArtifact`, while `ClassFileError::Attribute(AttributeError::TooDeeplyNestedAnnotation)` is an implementation bound (`DEFAULT_MAX_ANNOTATION_DEPTH = 128`) and is `UnsupportedFeatureOrVersion`. Classfile errors do not expose a further `Error::source` chain. |
| `UnsupportedClassVersion(BinaryName, ClassFileVersion)` | `UnsupportedFeatureOrVersion` | `loader::load_uncached_class` checks `ClassFileVersion::is_compatible` before allocating a symbol. The exact major/minor version and requested name survive. |
| `MalformedReference(BinaryName, PoolRefError)` | `MalformedArtifact` | Produced by `loader::resolve_name` and constant-pool reference resolution in class, member, nesting, and annotation lowering. The owning/requested class and typed pool failure survive. |
| `MalformedDescriptor(BinaryName, ResolveDescriptorError)` | `MalformedArtifact` | Produced while lowering field and method descriptors in `loader::lower_field_type` and `loader::lower_method_descriptor`. The owner and typed descriptor cause survive. |
| `MalformedSignature(BinaryName, SignatureError)` | `Conditional` | Produced while parsing/lowering class, field, and method `Signature` attributes. Grammar errors are `MalformedArtifact`; `SignatureError::TooDeeplyNested` is an implementation bound (`DEFAULT_MAX_SIGNATURE_DEPTH = 128`) and is `UnsupportedFeatureOrVersion`. A syntactically valid signature whose variable cannot be resolved is reported separately as `UnresolvedTypeVariable`. |
| `NameMismatch { requested, actual }` | `MalformedArtifact` / resource identity mismatch | `loader::load_uncached_class` resolves `this_class` and checks it against the requested binary name. Both names survive. The class bytes may be valid in isolation, but the resource cannot satisfy this lookup. |
| `InvalidTastyFile(BinaryName, TastyDecodeError)` | `Conditional` | `loader::load_uncached_tasty` maps projection failures here; package-alias resolution can also convert a typed TASTy failure through `TastyAliasResolutionError::into_class_load_error`. `Parse`/`Ast` can be malformed or unsupported; `MissingTypeDef`, `NoTemplateBody`, and `UnresolvedSupertype` do not prove corrupt bytes (see below). |
| `CircularInheritance(BinaryName)` | `MalformedArtifact` | `loader::load_class_with_inheritance_ancestry` emits it when an in-progress name is already on the superclass/interface ancestry. A class's direct or indirect superclass/interface cycle is invalid; member-type cycles are intentionally handled differently. |
| `InheritanceDepthExceeded(BinaryName)` | `UnsupportedFeatureOrVersion` | The same loader checks `MAX_INHERITANCE_DEPTH` (256) before probing the next resource. A sufficiently deep hierarchy can be valid but exceeds this implementation bound. The rejected name may have no located resource yet. |
| `DependencyFailure { owner, dependency, source: Rc<ClassLoadError> }` | `ResolutionOrDependency` as wrapper; classify `source` separately | `loader::load_dependency` wraps superclass/interface failures; `loader::resolve_member_class` wraps member-type failures. Owner, dependency name, and the recursively typed cause survive. `Error::source` returns the same `Rc` allocation, including through nested dependency wrappers. |
| `UnresolvedTypeVariable(BinaryName, String)` | `Conditional` | Emitted while lowering a generic signature when no entered type-parameter symbol matches the name. It can indicate malformed signature input or a valid reference to an enclosing owner's parameter, which this loader does not yet model. Only owner and variable text survive. |

The public enum is currently exhaustive (no `#[non_exhaustive]`), and callers
can match its variants. Adding a required field or a new variant would break
those callers. The #980 design therefore keeps this enum and the existing
`ClassLoader::load_class(&BinaryName) -> Result<SymbolId, ClassLoadError>`
signature unchanged.

## Nested cause inventories

### `ClassPathError`

`ClassPathError` is a public struct with private source variants. It is not
`Clone`; `ClassLoadError::Io` stores it in `Rc`.

| Internal source | Classification | Notes |
| --- | --- | --- |
| `Io(io::Error)` | `IoOrEnvironment`, or `Conditional` for `InvalidData` | Directory reads and JAR/JMOD construction use this for permission failures, truncated reads, a missing configured archive at construction time, and similar operating-system failures. JMOD bad magic is currently synthesized as `io::ErrorKind::InvalidData`, so that `ErrorKind` is classified as `Conditional` rather than assumed to be an environment failure. |
| `Zip(ZipError)` | `Conditional` | Invalid headers, CRC/length mismatches, invalid DEFLATE data, and truncated reads indicate a malformed archive. `UnsupportedCompressionMethod`, ZIP64, and the 256 MiB entry limit can reject an otherwise valid archive as unsupported. `ZipError` does not retain the JAR entry name. |
| `InvalidBinaryName(BinaryName)` | malformed reference / path input | Filesystem and archive entries reject unsafe names before probing. The name may have come from untrusted classfile bytes. `ClassLoadError::Io` currently wraps this as `Io`, which is too broad for a stable category. |

The classpath API distinguishes `Ok(None)` from `Err(ClassPathError)`. In a
`CompositeClassPath`, the first entry returning `Err` stops lookup; the error
does not currently identify that entry. `JarClassPath::new` and
`JmodClassPath::new` can also fail before a class name is being loaded, so
those constructor errors are `ClassPathError`, not `ClassLoadError`.

### `TastyDecodeError`

| Variant | Primary classification | Evidence and ambiguity |
| --- | --- | --- |
| `Parse(TastyFileError)` | `Conditional` | Parsing can fail on malformed bytes. Nested `HeaderError::UnsupportedVersion` or `IncompatibleVersion` means unsupported valid-format/version input instead. `TastyFileError` currently implements `Error` without exposing its nested typed cause via `source()`. |
| `Ast(AstError)` | `Conditional` | Bad tags, references, addresses, and malformed AST structure are artifact errors; unsupported categories/relocations and recursion bounds can be valid input beyond current support. `AstError::source()` does not expose a nested `TermError`. |
| `MissingTypeDef` | `Conditional` (resource/name mismatch) | `tasty_symbol::decode_with_resolvers` cannot find a top-level `TypeDef` with the requested simple name. The `.tasty` bytes may be valid while the classpath resource is mismatched or has an unhandled shape. |
| `NoTemplateBody` | `Conditional` (unsupported shape or malformed projection) | Emitted when the matched definition does not contain the expected template form. The current projection cannot prove whether the input is malformed or a valid form it does not support. |
| `UnresolvedSupertype` | `ResolutionOrDependency` / `Conditional` | The parent tree did not reduce to a class name or the resolver could not choose a unique name. This can be a valid but unsupported TASTy shape or an actually unresolved/malformed reference. |

## `Error::source` and dependency causality

`ClassLoadError::source()` exposes `ClassPathError`, the classfile codec
errors, descriptor/signature/pool errors, `TastyDecodeError`, and the nested
`ClassLoadError` held by `DependencyFailure`. All other variants are leaves.
`TastyDecodeError::Parse` and `Ast` expose `TastyFileError` and `AstError`,
respectively, but those types currently stop the standard error chain even
when they contain a more specific nested cause. This is separate from the
typed enum data that remains available to crate code.

`DependencyFailure` is an outer relationship, not a replacement for the
underlying cause. Its `Rc<ClassLoadError>` retains the inner variant and
source chain. #980 should expose both:

1. the outer code (`classloader.dependency_failure`), describing why the
   requested owner could not complete; and
2. the deepest relevant typed cause/code, reached through nested dependency
   wrappers (for example `classloader.not_found`).

The existing immutable `Rc` chain is acyclic when built by the loader. A
future `root_cause()` accessor can walk `DependencyFailure::source` without
rendering or parsing messages and return a borrow into the retained chain.

## Provenance inventory

`ClassResource` already carries bytes, `ClassFormat`, and `ClassOrigin`.
`ClassOrigin` currently names only the directory root, JAR path, or JMOD path;
it does not identify the exact file/entry or selected multi-release JAR
version. `JarClassPath::find_class` knows the selected entry name while
reading it, but discards that name when it constructs `ClassResource`.

| Failure point / family | Requested name | Format | Current origin knowledge | What the returned/cached error retains |
| --- | --- | --- | --- | --- |
| No resource (`NotFound`) | yes | none | none; every entry returned `Ok(None)` | requested name only; this is correct, no origin should be invented |
| Classpath lookup error (`Io`) | yes | usually unknown | the failing entry may know its root/path, but `ClassPathError` does not retain it and `CompositeClassPath` does not label the failing entry | name plus `Rc<ClassPathError>`; exact failing root/attempted file is lost |
| Classfile decode, version, name, reference, descriptor, signature errors | yes | `Class` known in `load_uncached` | root origin is known in the local `ClassResource`; exact file or selected archive entry is not retained | variant-specific error fields, with no origin or format |
| TASTy parse/projection errors | yes | `Tasty` known in `load_uncached` | same as above | `TastyDecodeError`, with no origin or format |
| Unresolved type variable | owner and variable | `Class` | known at the point signature lowering fails | owner and variable only |
| Inheritance cycle | cycle member name | resource was previously loaded/entered | origin was recorded in the session when its symbol was entered; the error has no direct link to that symbol or origin | class name only, possibly nested in a dependency chain |
| Inheritance depth bound | name rejected before lookup | unknown | no resource lookup has happened for that name | class name only; enclosing resources may appear as dependency wrappers |
| Dependency failure | owner and dependency | source-dependent | source chain identifies the failing dependency name; no origin travels in the chain | owner/dependency names and nested `ClassLoadError`s |

`ClassLoader::origin(SymbolId)` is a success-side lookup into
`LoadingSession::origins`. `load_uncached_class` and `load_uncached_tasty`
record the root origin after decoding and entering a symbol, before dependency
completion. This helps a caller that already holds a recursively exposed
`SymbolId`, but `load_class` returns only `Err(ClassLoadError)` on failure;
pre-entry decode failures have no symbol, and the failed repository entry
does not pair its error with origin or format.

`ClassRepository::Failed(Option<SymbolId>, ClassLoadError)` is loader-local
and caches cloned failures. This scope is intentional: classpaths can differ
between loaders, so `LoadingSession` shares only positive resolved symbols
and symbol-keyed successful metadata/origins. A later call through the same
loader reuses the cached failure without another lookup; a new loader may
probe its own classpath. Resolver operations construct a short-lived loader
and use `transact` to roll back failed store/session mutations.

## Backward-compatible #980 design

1. Keep every existing `ClassLoadError` variant, payload, `Clone`, `Display`,
   `Error::source`, and `load_class` signature intact.
2. Add exhaustive stable `ClassLoadError::code()` and category/root-cause
   accessors. Codes should name the outer variant and use the `classloader.`
   prefix; nested TASTy/classpath causes may expose typed codes of their own.
   Categories for `InvalidClassFile` and `MalformedSignature` must follow the
   retained typed cause: malformed structure/grammar maps to
   `MalformedArtifact`, while `TooDeeplyNestedAnnotation` and
   `TooDeeplyNested` map to `UnsupportedFeatureOrVersion`. The latter limits
   are implementation-defined stack-safety bounds, not JVM format constraints;
   avoid classifying the entire wrapper variant as malformed just from its
   name.
   Suggested outer codes:

   | Variant | Suggested stable code |
   | --- | --- |
   | `NotFound` | `classloader.not_found` |
   | `Io` | `classloader.classpath_error` |
   | `InvalidClassFile` | `classloader.invalid_class_file` |
   | `UnsupportedClassVersion` | `classloader.unsupported_class_version` |
   | `MalformedReference` | `classloader.malformed_reference` |
   | `MalformedDescriptor` | `classloader.malformed_descriptor` |
   | `MalformedSignature` | `classloader.malformed_signature` |
   | `NameMismatch` | `classloader.name_mismatch` |
   | `InvalidTastyFile` | `classloader.invalid_tasty_file` |
   | `CircularInheritance` | `classloader.circular_inheritance` |
   | `InheritanceDepthExceeded` | `classloader.inheritance_depth_exceeded` |
   | `DependencyFailure` | `classloader.dependency_failure` |
   | `UnresolvedTypeVariable` | `classloader.unresolved_type_variable` |

   `classloader.classpath_error` is deliberately neutral: `Io` currently
   wraps operating-system failures, archive failures, and unsafe names.
   `ClassPathError`/`TastyDecodeError` should expose their own structural
   code/category where doing so preserves the public wrappers and avoids
   parsing `Display`.
3. Add a separate failure-context type (for example
   `ClassLoadFailure { error: ClassLoadError, origin: Option<ClassOrigin>,
   format: Option<ClassFormat>, resource: Option<ClassResourceLocation> }`)
   and an opt-in detailed load method. Keep the current method as a
   compatibility entry point that returns the `error` field.
4. Carry the detailed failure in the loader-local `ClassEntry::Failed`, so
   repeated calls preserve the same code, typed cause, and provenance without
   new I/O. Do not put negative results into `LoadingSession`; a failure on
   one classpath is not proof about another classpath.
5. Capture provenance at the existing `ClassResource` read/selection point.
   Retain the exact directory file or archive entry name, including the
   selected multi-release JAR path, without a second `find_class` or disk
   read. Keep the current `ClassOrigin` root field usable and avoid adding a
   required field to `ClassLoadError` variants.
6. When a dependency fails, preserve the current outer
   `DependencyFailure { owner, dependency, source }` and carry the source's
   provenance in the separate context. Report the failing dependency's
   artifact when known, never the requesting owner's artifact. `NotFound`
   correctly has no artifact origin; `InheritanceDepthExceeded` can also have
   no origin because the rejected name is not probed.
7. For classpath errors during lookup, capture the failing entry and attempted
   resource path in typed context where available. For invalid names or
   archive-level errors where a class resource was not selected, do not
   fabricate a `ClassFormat` or class file path.
8. Preserve the current rollback contract: resolver transaction rollback
   must remove context stored for symbols/failed entries created inside the
   rolled-back operation, while ordinary direct-loader failure caching stays
   loader-local. Keep `Error::source` chains useful and keep codes/categories
   independent of semantic-store lookups.

## Existing regression evidence and #980 test plan

Current focused coverage includes:

- `error.rs::not_found_displays_the_missing_class_name` and
  `loader.rs::not_found_when_the_classpath_has_nothing_for_the_name`;
- `loader.rs::a_cached_not_found_does_not_probe_the_same_classpath_twice`;
- `loader.rs::io_error_from_the_classpath_is_wrapped_and_not_cached_as_not_found`;
- `loader.rs::dependency_failure_when_the_super_class_cannot_be_found` and
  `loader.rs::dependency_failure_when_a_tasty_classes_super_class_cannot_be_found`;
- `error.rs::dependency_failure_chains_through_source` plus
  `error.rs::nested_dependency_failures_preserve_rc_identity_and_error_sources`;
- `loader.rs::loading_a_class_records_its_class_path_origin` and
  `loader.rs::a_symbols_metadata_and_origin_survive_a_loading_session_hand_off`;
- JAR/JMOD lookup tests asserting `ClassResource::origin()`;
- resolver tests that turn only `NotFound` into an unresolved `Ok(None)`.

The #980 `ClassLoader::load_class_detailed` API returns a `ClassLoadFailure`
with the error plus optional origin/format. Tests use small fixtures for
malformed `.class`, malformed `.tasty`, unsupported class version, missing
class, nested dependency failure, and a cached repeated failure; assert
code/category, source identity, origin/format presence or absence, and no
extra lookup when cached. Signature and annotation depth-limit unit tests
assert that `SignatureError::TooDeeplyNested` and
`AttributeError::TooDeeplyNestedAnnotation` classify as
`UnsupportedFeatureOrVersion`, while malformed grammar remains
`MalformedArtifact`. Directory and JAR failure provenance, missing
dependency context, and the dependency artifact's origin are covered
directly. JMOD retains its existing success-side origin coverage. Existing
classpath-selection, `Display`, resolver, cache, and rollback behavior stays
unchanged.

## #980 API decision

`ClassLoadError::code()` is an exhaustive stable code for the outer
`classloader.*` variant. `category()` describes that outer failure, so a
`DependencyFailure` remains `ResolutionOrDependency`; `root_cause()` returns
the deepest nested `ClassLoadError`, with separate root-cause code/category
accessors. Classpath and TASTy errors expose their own stable codes and use
retained typed causes for categories where available. No classification
parses `Display` text.

`ClassLoadFailure` is additive and returned by
`ClassLoader::load_class_detailed`. Its `error` keeps the existing typed
chain, while `origin` and `format` identify the selected artifact behind the
deepest failure when known. The loader captures them from the original
`ClassResource` and stores them with loader-local failed cache entries, so a
retry through the same loader returns identical context without another
classpath lookup. Dependency wrappers retain their outer code but carry the
failing dependency's context. `NotFound` and lookup errors before selecting a
resource have no fabricated provenance. Existing `load_class` and every
`ClassLoadError` variant remain unchanged.

No `SourceSpan` is attached: these failures arise from binary classpath
artifacts, which have no source-file identity or source offsets to report.
