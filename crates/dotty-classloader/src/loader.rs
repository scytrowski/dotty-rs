use crate::annotation::{AnnotationValue, SemanticAnnotation};
use crate::binary_name::BinaryName;
use crate::class_path::{ClassFormat, ClassOrigin, ClassPathEntry};
use crate::error::ClassLoadError;
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::nesting::{EnclosingMethodRef, InnerClassEntry};
use crate::record_component::RecordComponentSymbol;
use crate::repository::{ClassEntry, ClassRepository};
use crate::session::LoadingSession;
use crate::symbol::{ClassRef, ClassfileMetadata};
use crate::tasty_symbol;
use dotty_classfile::access_flags::{ClassAccessFlags, FieldAccessFlags, MethodAccessFlags};
use dotty_classfile::attribute::{Annotation, Attribute, ElementValue};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, EntryKind, PoolRefError,
};
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{
    ClassSignature, ClassTypeSignature, FieldSignature, MethodSignature, ReferenceTypeSignature,
    SignatureError, TypeArgument, TypeSignature,
};
use dotty_core::{
    Annotation as CoreAnnotation, ClassInfo, Definitions, ErrorType, MethodKind, MethodParam,
    MethodType, Name, Namespace, PolyType, Scope, ScopeId, SemanticStore, Symbol, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, TermName, Type, TypeId, TypeName,
    TypeParam, Visibility,
};
use std::collections::HashMap;
use std::rc::Rc;

/// Loads `.class`/`.tasty`-backed classes from a [`ClassPathEntry`] into a
/// [`SemanticStore`], caching results in a [`ClassRepository`].
///
/// Implements `dotty-core`'s "enter before complete" algorithm
/// (`docs/classloader.md` §5): a class's `SymbolId` is allocated before its
/// superclass/interfaces are resolved, so a class that (in)directly
/// references itself as a supertype is reported as
/// [`ClassLoadError::CircularInheritance`] instead of recursing forever.
///
/// JVM-specific data that doesn't belong in `dotty-core`'s canonical
/// semantic model (fields/methods, raw signatures, nesting metadata,
/// annotations — see [`ClassfileMetadata`]) lives in this loader's own
/// `SymbolId`-keyed metadata table instead.
pub struct ClassLoader<'store, E> {
    class_path: E,
    store: &'store mut SemanticStore,
    /// The positive-only, `SymbolId`-allocating state (successfully
    /// resolved classes/packages) this loader shares with every other
    /// `ClassLoader` sequentially loading against the same `store` — see
    /// [`LoadingSession`]'s own doc comment for why this must be threaded
    /// through by value rather than each loader owning its own, and for
    /// why it deliberately does not also share negative (`Failed`) or
    /// in-progress (`Loading`) state.
    session: LoadingSession,
    /// This loader's own `Loading`/`Loaded`/`Failed` cache, covering
    /// circular-inheritance detection and negative-result caching — kept
    /// loader-local (unlike `session`) because a name this loader's own
    /// `class_path` doesn't have is not evidence that a differently
    /// configured loader sharing `session` (the documented
    /// one-per-classpath-root pattern) doesn't have it either; see
    /// [`LoadingSession`]'s doc comment.
    repository: ClassRepository,
    /// The canonical `TypeId`/`SymbolId` every JVM primitive and
    /// `Object`/`Any`/`Nothing` reference lowers against — see
    /// [`Definitions::bootstrap`]. `definitions.no_prefix` is this
    /// loader's `Type::NoPrefix` too (see [`Self::with_definitions`]):
    /// there is no separate loader-owned copy, so every `ClassLoader`
    /// sharing one `Definitions` also agrees on prefix identity.
    definitions: Definitions,
}

/// Reconstructs a [`ClassTypeSignature`]'s binary name (JVMS §4.2.1): its
/// package plus simple name, with any `.`-qualified inner-class suffix
/// joined onto the simple name with `$` — the same convention a compiled
/// inner class's own binary name already uses (`docs/classloader.md`'s
/// nesting section). Used only to resolve which class a signature names;
/// the class's *own* binary name (as loaded) is never reconstructed from
/// this — see `resolve_inner_classes`/`InnerClasses` for that.
/// Resolves a `TypeVariable` (JVMS §4.7.9.1) while lowering a signature.
///
/// A method-level type parameter of its own (`method_type_params`, `name ->`
/// its already-allocated `Type::ParamRef` into the enclosing `Type::Poly`)
/// shadows the declaring class's own type parameters (`declarations`, real
/// `SymbolKind::TypeParameter` symbols looked up via ordinary
/// `Scope::lookup` — see [`ClassLoader::enter_class_type_parameters`]).
/// `method_type_params` is `None` while lowering a class's own type
/// parameter bounds, or a field/non-generic-method signature, where there
/// is no method-level binder in scope at all.
struct TypeVarEnv<'a> {
    declarations: ScopeId,
    method_type_params: Option<&'a HashMap<String, TypeId>>,
}

impl TypeVarEnv<'_> {
    fn class_only(declarations: ScopeId) -> Self {
        Self {
            declarations,
            method_type_params: None,
        }
    }
}

fn class_type_signature_binary_name(signature: &ClassTypeSignature) -> BinaryName {
    let mut simple_name = signature.simple_name.clone();
    for suffix in &signature.suffix {
        simple_name.push('$');
        simple_name.push_str(&suffix.name);
    }
    if signature.package.is_empty() {
        BinaryName::from_internal(simple_name)
    } else {
        BinaryName::from_internal(format!("{}/{}", signature.package.join("/"), simple_name))
    }
}

impl<'store, E: ClassPathEntry> ClassLoader<'store, E> {
    /// Convenience for the common case: one `ClassLoader` for the whole
    /// lifetime of `store`. Bootstraps its own [`Definitions`], which is
    /// only correct when nothing else ever bootstraps another one
    /// against the same `store` — [`Definitions::bootstrap`]'s own doc
    /// comment requires exactly one per `SemanticStore`/session, and
    /// calling this twice on the same store would silently mint two
    /// independent, non-`==` sets of `Object`/`Any`/`Nothing`/primitive
    /// `SymbolId`s in it. When several `ClassLoader`s need to share one
    /// `SemanticStore` (e.g. one per classpath root, or one per
    /// incremental recompilation unit), bootstrap [`Definitions`] once
    /// up front and use [`Self::with_definitions`] for every loader
    /// instead — and, to also share ordinary class/package `SymbolId`
    /// identity (not just the builtins), thread one [`LoadingSession`]
    /// through every loader's `with_definitions` call via
    /// [`Self::into_session`], the way this method already threads a
    /// fresh one through internally for its own single-loader use.
    pub fn new(class_path: E, store: &'store mut SemanticStore) -> Self {
        let definitions = Definitions::bootstrap(store);
        Self::with_definitions(class_path, store, definitions, LoadingSession::new())
    }

    /// Builds a `ClassLoader` against an already-bootstrapped, shared
    /// [`Definitions`] and [`LoadingSession`] — the construction path
    /// multiple `ClassLoader`s sharing one `SemanticStore` must use, so
    /// every one of them resolves `Object`/`Any`/`Nothing`/primitives (via
    /// `definitions`) and ordinary classes/packages (via `session`) to the
    /// exact same canonical `SymbolId`s/`TypeId`s instead of each minting
    /// its own (see [`Self::new`]'s doc comment). Get `session` back out
    /// via [`Self::into_session`] once this loader is done, to hand to the
    /// next one.
    pub fn with_definitions(
        class_path: E,
        store: &'store mut SemanticStore,
        definitions: Definitions,
        session: LoadingSession,
    ) -> Self {
        Self {
            class_path,
            store,
            session,
            repository: ClassRepository::new(),
            definitions,
        }
    }

    /// This loader's canonical [`Definitions`] — cheap to read back
    /// ([`Definitions`] is `Copy`), e.g. to hand to
    /// [`Self::with_definitions`] for a second `ClassLoader` sharing this
    /// one's `store`.
    pub fn definitions(&self) -> Definitions {
        self.definitions
    }

    /// Hands this loader's [`LoadingSession`] back out, once it is done, so
    /// a second `ClassLoader` sharing the same `store` can pick up its
    /// class/package `SymbolId` identities via
    /// [`Self::with_definitions`] instead of starting a fresh, disagreeing
    /// session of its own.
    pub fn into_session(self) -> LoadingSession {
        self.session
    }

    /// JVM-specific metadata for a `.class`-backed symbol previously
    /// returned by [`Self::load_class`] — `None` for a `.tasty`-backed
    /// symbol (not yet reconstructed from `.tasty`, see
    /// [`ClassfileMetadata`]'s doc comment), or a `SymbolId` no loader
    /// sharing this `session` has produced. Session-wide, not
    /// loader-local, like [`Self::origin`] — see [`LoadingSession`]'s own
    /// doc comment for why that is safe for `SymbolId`-keyed data.
    pub fn metadata(&self, id: SymbolId) -> Option<&ClassfileMetadata> {
        self.session.metadata.get(&id)
    }

    /// Where a class's bytes were read from (a directory root/JAR/JMOD
    /// path) — `None` for a `SymbolId` no loader sharing this `session`
    /// has produced (e.g. a `Definitions` builtin, which has no backing
    /// classpath resource).
    pub fn origin(&self, id: SymbolId) -> Option<&ClassOrigin> {
        self.session.origins.get(&id)
    }

    /// Loads (or returns the cached result for) the class named `name`.
    pub fn load_class(&mut self, name: &BinaryName) -> Result<SymbolId, ClassLoadError> {
        // Checked first, and separately from `self.repository` below: a
        // name found here was resolved successfully by *some* loader
        // sharing this `session` (this one or an earlier one), and that
        // positive result is valid regardless of which loader's
        // `class_path` produced it (`LoadingSession`'s own doc comment).
        // A negative or in-progress result, by contrast, is *not*
        // necessarily valid for a differently configured loader, so it is
        // never stored here -- only in the loader-local `self.repository`
        // consulted next.
        if let Some(&symbol) = self.session.resolved.get(name) {
            return Ok(symbol);
        }

        match self.repository.get(name).cloned() {
            Some(ClassEntry::Loaded(symbol)) => return Ok(symbol),
            Some(ClassEntry::Failed(_, error)) => return Err(error),
            Some(ClassEntry::Loading(symbol)) => {
                let error = ClassLoadError::CircularInheritance(name.clone());
                // Same reasoning as the `DependencyFailure` branch below:
                // this class's `Symbol` was already allocated by
                // `enter_class` and may already be reachable through a
                // legitimate mutual member-type reference, so it must not
                // be left looking like a merely-not-yet-completed
                // `SymbolInfo::Missing` once the repository says `Failed`.
                self.store.symbols.get_mut(symbol).info = SymbolInfo::Error;
                self.repository
                    .mark_failed(name.clone(), Some(symbol), error.clone());
                return Err(error);
            }
            None => {}
        }

        match self.load_uncached(name) {
            Ok(symbol) => {
                self.repository.mark_loaded(name.clone(), symbol);
                self.session.resolved.insert(name.clone(), symbol);
                Ok(symbol)
            }
            Err(error) => {
                // `load_uncached` can fail after `enter_class` already
                // allocated this class's `SymbolId` (e.g. a later
                // `DependencyFailure` on its superclass) -- that `Symbol`
                // is left permanently `SymbolInfo::Missing` (`SemanticStore`
                // has no removal API to actually roll the allocation
                // back), and is still reachable by anything holding this
                // `SymbolId` already: a legitimate mutual member-type
                // reference (`Self::resolve_member_class`'s `Loading`
                // tolerance) hands out exactly this `SymbolId` while the
                // load is in progress, with no way to later learn the load
                // it was borrowed from went on to fail. Marking it
                // `SymbolInfo::Error` here -- instead of leaving a
                // `Failed` repository entry as the only place this
                // failure is recorded -- makes that already-observable
                // half-built `Symbol` self-describing instead of silently
                // looking like a real, merely-not-yet-completed one.
                // Only `None` (no `enter_class` reached yet, e.g. a
                // `NotFound`/`InvalidClassFile` failure) or `Loading` (set
                // by `enter_class` at the very start of this name's own
                // `load_uncached` call, above) are reachable here -- `name`
                // had no repository entry when `load_class` started.
                let still_loading_symbol = self.repository.get(name).and_then(ClassEntry::symbol);
                if let Some(symbol) = still_loading_symbol {
                    self.store.symbols.get_mut(symbol).info = SymbolInfo::Error;
                }
                self.repository
                    .mark_failed(name.clone(), still_loading_symbol, error.clone());
                Err(error)
            }
        }
    }

    fn load_uncached(&mut self, name: &BinaryName) -> Result<SymbolId, ClassLoadError> {
        let resource = self
            .class_path
            .find_class(name)
            .map_err(|error| ClassLoadError::Io(name.clone(), Rc::new(error)))?
            .ok_or_else(|| ClassLoadError::NotFound(name.clone()))?;

        match resource.format() {
            ClassFormat::Class => {
                self.load_uncached_class(name, resource.bytes(), resource.origin().clone())
            }
            ClassFormat::Tasty => {
                self.load_uncached_tasty(name, resource.bytes(), resource.origin().clone())
            }
        }
    }

    /// Allocates a class's stable `SymbolId`, and its declarations scope,
    /// immediately — before any recursive supertype/member resolution.
    /// `dotty-core`'s enter-before-complete rule (`docs/classloader.md`
    /// §5) covers the `SymbolId`; the scope is allocated this early too so
    /// fields/methods can be entered into it as they're decoded, rather
    /// than collected separately and entered only once the whole class is
    /// known.
    fn enter_class(
        &mut self,
        name: &BinaryName,
        flags: ClassAccessFlags,
        origin: SymbolOrigin,
    ) -> (SymbolId, ScopeId) {
        let owner = self.session.packages.resolve(self.store, name);

        let text = self.store.names.intern(name.simple_name());
        let symbol_name = Name::new(text, Namespace::Type);

        // A JVM class file's own access_flags carry no ACC_PRIVATE/
        // ACC_PROTECTED bit (JVMS §4.1 Table 4.1-A) -- those only appear on
        // a *member* class's InnerClasses entry, not here -- so a top-level
        // class is only ever public or package-private. `flags` is shaped
        // exactly like a real `.class` file's access_flags for both
        // callers (`load_uncached_class` and, via
        // `tasty_symbol::decode_flags`, `load_uncached_tasty`), so this is
        // correct and complete for `.class` but not the final word for
        // `.tasty` -- see `load_uncached_tasty`'s own visibility patch,
        // right after it calls this, for where a `.tasty` class's real
        // (potentially more restrictive) `private`/`protected` visibility
        // is corrected in.
        let kind = if flags.is_interface() {
            SymbolKind::Trait
        } else {
            SymbolKind::Class
        };
        let visibility = if flags.is_public() {
            Visibility::Public
        } else {
            Visibility::Package(owner)
        };

        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_abstract() {
            symbol_flags = symbol_flags | SymbolFlags::ABSTRACT;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }

        // `java/lang/Object` is the one real, classpath-loadable JVM class
        // among `Definitions`'s builtins (`Any`/`Nothing` are Scala
        // compiler fictions with no `.class`/`.tasty` of their own) --
        // reusing `definitions.object_class`'s `SymbolId` here, instead of
        // allocating a fresh one, means a `Type::TypeRef` built from
        // `Definitions` (e.g. an implicit JVM superclass) and one built
        // from actually loading `java/lang/Object` off the classpath name
        // the exact same symbol, not two permanently distinct ones.
        let class_symbol = if name.as_internal() == "java/lang/Object" {
            let symbol = self.definitions.object_class;
            *self.store.symbols.get_mut(symbol) = Symbol {
                name: symbol_name,
                owner: Some(owner),
                kind,
                flags: symbol_flags,
                visibility,
                info: SymbolInfo::Missing,
                origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            };
            symbol
        } else {
            self.store.symbols.alloc(Symbol {
                name: symbol_name,
                owner: Some(owner),
                kind,
                flags: symbol_flags,
                visibility,
                info: SymbolInfo::Missing,
                origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            })
        };
        let declarations = self.store.scopes.alloc(Scope::new(Some(class_symbol)));

        (class_symbol, declarations)
    }

    /// Resolves `super_class`/`interfaces` into `Type::TypeRef`s and
    /// completes `class_symbol`'s `SymbolInfo`. `declarations` was already
    /// allocated by [`Self::enter_class`] and populated (for a `.class`
    /// symbol) by the field/method decode loop by the time this runs.
    fn complete_class(
        &mut self,
        class_symbol: SymbolId,
        declarations: ScopeId,
        super_class: Option<SymbolId>,
        interfaces: Vec<SymbolId>,
    ) {
        let mut parents = Vec::with_capacity(interfaces.len() + 1);
        if let Some(parent) = super_class {
            parents.push(self.type_ref(parent));
        }
        for parent in interfaces {
            parents.push(self.type_ref(parent));
        }

        let class_info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class: class_symbol,
            parents,
            declarations,
            self_type: None,
        }));
        self.store
            .symbols
            .set_info(class_symbol, SymbolInfo::Complete(class_info));
    }

    fn type_ref(&mut self, symbol: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            symbol,
        })
    }

    /// Allocates a field's `Symbol` and enters it into the class's
    /// declarations scope by name. Unlike a class, a field needs no
    /// enter-before-complete staging: its type is fully known the moment
    /// its descriptor (and, transitively, any class it references) is
    /// resolved, so it is allocated already `SymbolInfo::Complete`.
    fn enter_field(
        &mut self,
        class_symbol: SymbolId,
        declarations: ScopeId,
        field_name: &str,
        flags: FieldAccessFlags,
        resolved_type: TypeId,
        origin: SymbolOrigin,
    ) -> SymbolId {
        // Unlike a top-level class's access_flags, a field's *can* carry
        // ACC_PRIVATE/ACC_PROTECTED (JVMS §4.5 Table 4.5-A), so all four
        // Java visibilities are distinguishable here.
        let visibility = if flags.is_private() {
            Visibility::Private
        } else if flags.is_protected() {
            Visibility::Protected
        } else if flags.is_public() {
            Visibility::Public
        } else {
            let owner = self
                .store
                .symbols
                .get(class_symbol)
                .owner
                .expect("a loaded class always has a package owner");
            Visibility::Package(owner)
        };

        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_static() {
            symbol_flags = symbol_flags | SymbolFlags::STATIC;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        } else {
            symbol_flags = symbol_flags | SymbolFlags::MUTABLE;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }

        let text = self.store.names.intern(field_name);
        let symbol_name = Name::new(text, Namespace::Term);

        let field_symbol = self.store.symbols.alloc(Symbol {
            name: symbol_name,
            owner: Some(class_symbol),
            kind: SymbolKind::Field,
            flags: symbol_flags,
            visibility,
            info: SymbolInfo::Complete(resolved_type),
            origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });

        self.store
            .scopes
            .get_mut(declarations)
            .enter(symbol_name, field_symbol);
        field_symbol
    }

    /// Walks an already-parsed [`FieldType`] (JVMS §4.3.2) into a semantic
    /// `TypeId`: primitives map to their canonical [`Definitions`]
    /// identity, `Object` resolves via [`Self::resolve_member_class`] into
    /// a `Type::TypeRef`, and `Array` becomes a `Type::JavaArray`
    /// recursing into its component type.
    fn lower_field_type(
        &mut self,
        owner: &BinaryName,
        field_type: &FieldType,
    ) -> Result<TypeId, ClassLoadError> {
        match field_type {
            FieldType::Byte => Ok(self.definitions.byte),
            FieldType::Char => Ok(self.definitions.char),
            FieldType::Double => Ok(self.definitions.double),
            FieldType::Float => Ok(self.definitions.float),
            FieldType::Int => Ok(self.definitions.int),
            FieldType::Long => Ok(self.definitions.long),
            FieldType::Short => Ok(self.definitions.short),
            FieldType::Boolean => Ok(self.definitions.boolean),
            FieldType::Object(class_name) => {
                let dependency = BinaryName::from_internal(class_name);
                let symbol = self.resolve_member_class(owner, &dependency)?;
                Ok(self.type_ref(symbol))
            }
            FieldType::Array(component) => {
                let element = self.lower_field_type(owner, component)?;
                Ok(self.store.types.alloc(Type::JavaArray { element }))
            }
        }
    }

    /// Resolves an already-parsed [`FieldType`]'s optional `Signature`
    /// attribute and lowers the field's semantic type, in the order both a
    /// real field and a `Record` component need them. Shared by the
    /// field-building loop below and [`Self::resolve_record_components`]
    /// — the only difference between the two is that a record component
    /// has no access flags of its own in the class file format.
    ///
    /// When a `Signature` is present, it is strictly more precise than the
    /// erased descriptor (e.g. `List<T>` vs. erased `List`), so it is
    /// preferred as the resolved type; `field_type` is still parsed either
    /// way (it stays on [`crate::field_symbol::FieldSymbol`]/
    /// [`RecordComponentSymbol`] verbatim) but is only *lowered* when there
    /// is no signature to prefer instead.
    fn lower_field_like(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        field_type: &FieldType,
        attributes: &[Attribute<'_>],
        constant_pool: &ConstantPool,
    ) -> Result<(Option<FieldSignature>, TypeId), ClassLoadError> {
        let signature = self.resolve_optional_signature(
            owner,
            attributes,
            constant_pool,
            FieldSignature::parse,
        )?;
        let resolved_type = match &signature {
            Some(FieldSignature(reference)) => {
                let env = TypeVarEnv::class_only(declarations);
                self.lower_reference_type_signature(owner, &env, reference)?
            }
            None => self.lower_field_type(owner, field_type)?,
        };
        Ok((signature, resolved_type))
    }

    /// Enters each of `signature`'s declared type parameters (JVMS
    /// §4.7.9.1) as a real `SymbolKind::TypeParameter` symbol into
    /// `declarations` — a no-op when the class has no `Signature` attribute
    /// (the common, non-generic case) or declares none of its own. Every
    /// symbol is entered under its own `Namespace::Type` name *before* any
    /// bound is lowered (mirroring `enter_class`'s enter-before-complete
    /// staging one level down), so a bound that mentions another type
    /// parameter — an F-bounded parameter's own bound mentioning itself
    /// (`<T extends Comparable<T>>`), or one parameter's bound mentioning a
    /// sibling declared later (`<K extends Comparable<K>, V>`) — resolves
    /// correctly regardless of declaration order.
    ///
    /// Field/method signature lowering below resolves a `TypeVariable`
    /// reference against these same symbols via ordinary `Scope::lookup` on
    /// `declarations` (see [`Self::resolve_type_variable`]) — ordinary name
    /// lookup, since a type parameter and a same-named field/method never
    /// collide (`Namespace::Type` vs. `Namespace::Term`).
    fn enter_class_type_parameters(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        signature: Option<&ClassSignature>,
        origin: SymbolOrigin,
    ) -> Result<(), ClassLoadError> {
        let Some(signature) = signature else {
            return Ok(());
        };

        let class_symbol = self.store.scopes.get(declarations).owner;
        let mut entered = Vec::with_capacity(signature.type_parameters.len());
        for parameter in &signature.type_parameters {
            let text = self.store.names.intern(&parameter.name);
            let symbol_name = Name::new(text, Namespace::Type);
            let symbol = self.store.symbols.alloc(Symbol {
                name: symbol_name,
                owner: class_symbol,
                kind: SymbolKind::TypeParameter,
                flags: SymbolFlags::JAVA_DEFINED,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            self.store
                .scopes
                .get_mut(declarations)
                .enter(symbol_name, symbol);
            entered.push((symbol, parameter));
        }

        let env = TypeVarEnv::class_only(declarations);
        for (symbol, parameter) in entered {
            let high = match &parameter.class_bound {
                Some(bound) => self.lower_reference_type_signature(owner, &env, bound)?,
                None => self.type_ref(self.definitions.object_class),
            };
            let high = parameter.interface_bounds.iter().try_fold(
                high,
                |left, interface_bound| -> Result<TypeId, ClassLoadError> {
                    let right =
                        self.lower_reference_type_signature(owner, &env, interface_bound)?;
                    Ok(self.store.types.alloc(Type::And { left, right }))
                },
            )?;
            let low = self.type_ref(self.definitions.nothing_class);
            let bounds = self.store.types.alloc(Type::Bounds { low, high });
            self.store
                .symbols
                .set_info(symbol, SymbolInfo::Complete(bounds));
        }

        Ok(())
    }

    /// Resolves a `TypeVariable`'s name (JVMS §4.7.9.1) to the `TypeId` it
    /// denotes — see [`TypeVarEnv`]'s doc comment for the shadowing order.
    /// `None` when neither finds it (an unresolvable/out-of-scope type
    /// variable; the caller turns this into
    /// [`ClassLoadError::UnresolvedTypeVariable`]).
    fn resolve_type_variable(&mut self, env: &TypeVarEnv, name: &str) -> Option<TypeId> {
        if let Some(&param_ref) = env.method_type_params.and_then(|params| params.get(name)) {
            return Some(param_ref);
        }
        let text = self.store.names.intern(name);
        let lookup_name = Name::new(text, Namespace::Type);
        let symbol = self
            .store
            .scopes
            .get(env.declarations)
            .lookup(&lookup_name)?;
        Some(self.type_ref(symbol))
    }

    /// Walks an already-parsed [`TypeSignature`] (JVMS §4.7.9.1) into a
    /// semantic `TypeId`, the generic-signature counterpart of
    /// [`Self::lower_field_type`]: a base type lowers exactly like its
    /// descriptor equivalent (there is no generic form of a primitive), and
    /// a reference type delegates to
    /// [`Self::lower_reference_type_signature`].
    fn lower_type_signature(
        &mut self,
        owner: &BinaryName,
        env: &TypeVarEnv,
        signature: &TypeSignature,
    ) -> Result<TypeId, ClassLoadError> {
        match signature {
            TypeSignature::Base(field_type) => self.lower_field_type(owner, field_type),
            TypeSignature::Reference(reference) => {
                self.lower_reference_type_signature(owner, env, reference)
            }
        }
    }

    /// Walks an already-parsed [`ReferenceTypeSignature`] (JVMS §4.7.9.1)
    /// into a semantic `TypeId`: a type variable resolves via
    /// [`Self::resolve_type_variable`], an array recurses into its
    /// component the same way [`Self::lower_field_type`]'s `Array` arm
    /// does, and a class type delegates to
    /// [`Self::lower_class_type_signature`].
    fn lower_reference_type_signature(
        &mut self,
        owner: &BinaryName,
        env: &TypeVarEnv,
        signature: &ReferenceTypeSignature,
    ) -> Result<TypeId, ClassLoadError> {
        match signature {
            ReferenceTypeSignature::TypeVariable(name) => self
                .resolve_type_variable(env, name)
                .ok_or_else(|| ClassLoadError::UnresolvedTypeVariable(owner.clone(), name.clone())),
            ReferenceTypeSignature::Array(component) => {
                let element = self.lower_type_signature(owner, env, component)?;
                Ok(self.store.types.alloc(Type::JavaArray { element }))
            }
            ReferenceTypeSignature::Class(class_signature) => {
                self.lower_class_type_signature(owner, env, class_signature)
            }
        }
    }

    /// Walks an already-parsed [`ClassTypeSignature`] (JVMS §4.7.9.1) into
    /// a semantic `TypeId`: resolves the class it names the same
    /// mutual-reference-tolerant way [`Self::lower_field_type`]'s `Object`
    /// arm does, and — when it carries type arguments — wraps it in a
    /// `Type::Applied`.
    ///
    /// A `.`-qualified inner-class suffix (`Outer<T>.Inner<U>`) only keeps
    /// the *last* segment's own type arguments (`U`); an outer qualifier's
    /// type arguments are dropped rather than modeled as a fully qualified
    /// prefix chain. This is a deliberately partial scope choice: the
    /// common `java.util`-style generics this loader actually needs to
    /// resolve never use qualified inner-class generic syntax.
    fn lower_class_type_signature(
        &mut self,
        owner: &BinaryName,
        env: &TypeVarEnv,
        signature: &ClassTypeSignature,
    ) -> Result<TypeId, ClassLoadError> {
        let dependency = class_type_signature_binary_name(signature);
        let class_symbol = self.resolve_member_class(owner, &dependency)?;
        let tycon = self.type_ref(class_symbol);

        let type_arguments = signature
            .suffix
            .last()
            .map_or(&signature.type_arguments, |last| &last.type_arguments);
        if type_arguments.is_empty() {
            return Ok(tycon);
        }

        let mut args = Vec::with_capacity(type_arguments.len());
        for argument in type_arguments {
            args.push(self.lower_type_argument(owner, env, argument)?);
        }
        Ok(self.store.types.alloc(Type::Applied { tycon, args }))
    }

    /// Walks an already-parsed [`TypeArgument`] (JVMS §4.7.9.1) into a
    /// semantic `TypeId`: `Exact` lowers straight through (invariant, no
    /// wrapping), while `Extends`/`Super`/the bare wildcard `*` each become
    /// a `Type::Wildcard` around a `Type::Bounds` — `? extends Number` is
    /// `Bounds { low: Nothing, high: Number }`, `? super Number` is
    /// `Bounds { low: Number, high: Any }`, and bare `?`/`*` is
    /// `Bounds { low: Nothing, high: Any }`.
    fn lower_type_argument(
        &mut self,
        owner: &BinaryName,
        env: &TypeVarEnv,
        argument: &TypeArgument,
    ) -> Result<TypeId, ClassLoadError> {
        match argument {
            TypeArgument::Exact(reference) => {
                self.lower_reference_type_signature(owner, env, reference)
            }
            TypeArgument::Extends(reference) => {
                let high = self.lower_reference_type_signature(owner, env, reference)?;
                let low = self.type_ref(self.definitions.nothing_class);
                let bounds = self.store.types.alloc(Type::Bounds { low, high });
                Ok(self.store.types.alloc(Type::Wildcard { bounds }))
            }
            TypeArgument::Super(reference) => {
                let low = self.lower_reference_type_signature(owner, env, reference)?;
                let high = self.type_ref(self.definitions.any_class);
                let bounds = self.store.types.alloc(Type::Bounds { low, high });
                Ok(self.store.types.alloc(Type::Wildcard { bounds }))
            }
            TypeArgument::Wildcard => {
                let low = self.type_ref(self.definitions.nothing_class);
                let high = self.type_ref(self.definitions.any_class);
                let bounds = self.store.types.alloc(Type::Bounds { low, high });
                Ok(self.store.types.alloc(Type::Wildcard { bounds }))
            }
        }
    }

    /// The core visibility for a `.tasty` class's declared visibility.
    ///
    /// A qualified modifier keeps its qualifier when it names a package that
    /// encloses the class's own package (or is that package), which is the
    /// only qualifier a top-level class can meaningfully have. Any other
    /// qualifier this name-based reader cannot resolve falls back to the
    /// plain `Private`/`Protected` rather than widening to `Public`: too
    /// restrictive is safe, too permissive is the bug this guards against.
    fn tasty_visibility(
        &mut self,
        name: &BinaryName,
        declared: &tasty_symbol::DeclaredVisibility,
    ) -> Visibility {
        use tasty_symbol::{DeclaredQualifier, DeclaredVisibility};

        let (qualifier, protected) = match declared {
            DeclaredVisibility::Private => return Visibility::Private,
            DeclaredVisibility::Protected => return Visibility::Protected,
            DeclaredVisibility::PrivateWithin(qualifier) => (qualifier, false),
            DeclaredVisibility::ProtectedWithin(qualifier) => (qualifier, true),
        };
        let fallback = if protected {
            Visibility::Protected
        } else {
            Visibility::Private
        };
        let DeclaredQualifier::Package(path) = qualifier else {
            return fallback;
        };
        let own = name.package_path();
        let encloses = own == path
            || own
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('/'));
        if !encloses {
            return fallback;
        }
        let package = self.session.packages.resolve_package(self.store, path);
        if protected {
            Visibility::ProtectedWithin(package)
        } else {
            Visibility::PrivateWithin(package)
        }
    }

    /// `.tasty`-backed loading (`docs/classloader.md` §9): reconstructs
    /// only name/flags/superclass/interfaces via [`tasty_symbol::decode`]
    /// and recurses into the same [`Self::load_dependency`] used by the
    /// `.class` path, converging on the same `Symbol`/`Type::ClassInfo`
    /// shape. No [`ClassfileMetadata`] entry is produced — `fields`/
    /// `methods`/`signature`/nesting/annotations are not yet reconstructed
    /// from `.tasty`.
    fn load_uncached_tasty(
        &mut self,
        name: &BinaryName,
        bytes: &[u8],
        resource_origin: ClassOrigin,
    ) -> Result<SymbolId, ClassLoadError> {
        let decoded = tasty_symbol::decode(bytes, name)
            .map_err(|error| ClassLoadError::InvalidTastyFile(name.clone(), error))?;

        let origin = SymbolOrigin::Tasty(self.store.origins.register_tasty());
        let (class_symbol, declarations) = self.enter_class(name, decoded.flags, origin);
        self.session.origins.insert(class_symbol, resource_origin);
        self.repository.mark_loading(name.clone(), class_symbol);

        // `enter_class` derives visibility from `ClassAccessFlags` alone,
        // which (correctly, for `.class` — see its own doc comment) can
        // only ever produce `Public`/`Package` here. `.tasty` actually
        // knows the real, more restrictive `private`/`protected`
        // distinction (see `tasty_symbol::DeclaredVisibility`'s doc
        // comment for why `flags` itself can't carry it), so patch it in
        // now — the same "allocate first, correct once known" pattern
        // `resolve_semantic_owner`'s owner patch (`.class`'s nested-class
        // path) already uses.
        if let Some(declared) = &decoded.visibility {
            let visibility = self.tasty_visibility(name, declared);
            self.store.symbols.get_mut(class_symbol).visibility = visibility;
        }

        let super_class = self.load_dependency(name, decoded.super_class)?;
        let mut interfaces = Vec::with_capacity(decoded.interfaces.len());
        for dependency in decoded.interfaces {
            interfaces.push(self.load_dependency(name, dependency)?);
        }

        // `.tasty` annotation decoding is not built yet (unlike `.class`'s
        // `resolve_annotations`/`enter_annotations`), so every field/method
        // entered below simply keeps the empty `Symbol::annotations`
        // `enter_field`/`enter_method` already give it.
        for field in &decoded.fields {
            let resolved_type = self.lower_tasty_member_type(name, field.declared_type.as_ref());
            self.enter_field(
                class_symbol,
                declarations,
                &field.name,
                field.flags,
                resolved_type,
                origin,
            );
        }
        for method in &decoded.methods {
            let resolved_type = self.lower_tasty_method(name, method);
            self.enter_method(
                class_symbol,
                declarations,
                &method.name,
                method.flags,
                resolved_type,
                origin,
            );
        }

        self.complete_class(class_symbol, declarations, Some(super_class), interfaces);
        Ok(class_symbol)
    }

    /// Resolves a `.tasty` member's declared type (already reduced to a
    /// name by `tasty_symbol::decode`, best-effort) into a `TypeId`.
    ///
    /// Unlike [`Self::lower_field_type`]/[`Self::lower_method_descriptor`]
    /// (`.class` descriptors, which are always complete and exact), a
    /// `.tasty` member's declared-type *name* is itself only a best-effort
    /// reduction of an elaborated, post-typecheck type tree (see
    /// `tasty_symbol::DecodedTastyClass`'s doc comment) — so this never
    /// fails the class load. `None` (the decoder could not name the type)
    /// or a name that fails to load (a class this best-effort reduction
    /// got wrong, or one genuinely missing from the classpath) both fall
    /// back to `Type::Error` rather than propagating a [`ClassLoadError`]
    /// the way an unresolvable `.class` member type does — but, unlike a
    /// single shared generic message, each `ErrorType::message` names
    /// exactly which of the two happened and, for the second, carries the
    /// real underlying [`ClassLoadError`] (an ordinary
    /// [`ClassLoadError::NotFound`] on a genuinely missing dependency
    /// reads the same as any other class's, but an I/O error, a malformed
    /// class file, or a circular/dependency failure on a name this
    /// best-effort reduction actually got *right* is no longer
    /// indistinguishable from "the reduction guessed a name that doesn't
    /// exist" — both used to collapse into the exact same opaque string).
    fn lower_tasty_member_type(
        &mut self,
        owner: &BinaryName,
        declared_type: Option<&BinaryName>,
    ) -> TypeId {
        let Some(dependency) = declared_type else {
            let message = self.store.names.intern(
                "unresolved .tasty member type: tasty_symbol::decode could not reduce the \
                 declared type tree to a name",
            );
            return self.store.types.alloc(Type::Error(ErrorType { message }));
        };

        match self.resolve_member_class(owner, dependency) {
            Ok(symbol) => self.type_ref(symbol),
            Err(error) => {
                let text = format!(
                    "unresolved .tasty member type: best-effort name {dependency} failed to load: {error}"
                );
                let message = self.store.names.intern(&text);
                self.store.types.alloc(Type::Error(ErrorType { message }))
            }
        }
    }

    /// Lowers a [`tasty_symbol::DecodedTastyMethod`] into a `Type::Method`'s
    /// `TypeId`, the `.tasty` counterpart of
    /// [`Self::lower_method_descriptor`]. `<init>`'s result is always
    /// [`Definitions::unit`] rather than going through
    /// [`Self::lower_tasty_member_type`] — see the comment where
    /// `tasty_symbol::decode` gives it a `None` return type for why its
    /// return-type tree isn't a value type to resolve a name from at all.
    /// No parameter is ever `varargs`: a `.tasty` repeated parameter
    /// (`T*`) is encoded as its own distinct type-tree shape this decoder
    /// does not yet recognize (see `tasty_symbol`'s doc comments), rather
    /// than a flag alongside an ordinary array type the way JVMS
    /// `ACC_VARARGS` is.
    fn lower_tasty_method(
        &mut self,
        owner: &BinaryName,
        method: &tasty_symbol::DecodedTastyMethod,
    ) -> TypeId {
        let mut params = Vec::with_capacity(method.parameters.len());
        for parameter in &method.parameters {
            let ty = self.lower_tasty_member_type(owner, parameter.declared_type.as_ref());
            let text = self.store.names.intern(&parameter.name);
            params.push(MethodParam {
                name: TermName::new(text),
                ty,
                erased: false,
                varargs: false,
            });
        }

        let result = if method.name == "<init>" {
            self.definitions.unit
        } else {
            self.lower_tasty_member_type(owner, method.return_type.as_ref())
        };

        self.store.types.alloc(Type::Method(MethodType {
            params,
            result,
            kind: MethodKind::Plain,
        }))
    }

    fn load_uncached_class(
        &mut self,
        name: &BinaryName,
        bytes: &[u8],
        resource_origin: ClassOrigin,
    ) -> Result<SymbolId, ClassLoadError> {
        let mut reader = Reader::new(bytes);
        let class_file = ClassFile::decode(&mut reader)
            .map_err(|error| ClassLoadError::InvalidClassFile(name.clone(), error))?;

        // Checked before any symbol allocation: a version this decoder
        // cannot safely interpret (newer than the JDK-25 ceiling, older
        // than the JVMS has ever defined, or preview-features-only) must
        // not be given a semantic reading at all, not even a partial one.
        if !class_file.version.is_compatible() {
            return Err(ClassLoadError::UnsupportedClassVersion(
                name.clone(),
                class_file.version,
            ));
        }

        let actual_name = self.resolve_name(name, &class_file, class_file.this_class)?;
        if actual_name != *name {
            return Err(ClassLoadError::NameMismatch {
                requested: name.clone(),
                actual: actual_name,
            });
        }

        // Entered before any recursive resolution below, so a legitimate
        // mutual member-type reference back to `name` (see
        // `resolve_member_class`) can reuse this same `SymbolId` instead of
        // erroring, and a genuine supertype cycle back to `name` (via
        // `resolve_dependency` -> `load_class`) still hits `Loading` and is
        // rejected as `CircularInheritance`, exactly as before.
        let origin = SymbolOrigin::Classfile(self.store.origins.register_classfile());
        let (class_symbol, declarations) = self.enter_class(name, class_file.access_flags, origin);
        self.session.origins.insert(class_symbol, resource_origin);
        self.repository.mark_loading(name.clone(), class_symbol);

        // Patches `Symbol::owner` from the package (set by `enter_class`,
        // and always correct for `Visibility::Package` -- see
        // `resolve_semantic_owner`'s doc comment) to the real enclosing
        // class, for a nested/local/anonymous class. Done this early, via
        // `resolve_member_class`'s mutual-reference tolerance, so an outer
        // class that is itself still `Loading` (e.g. its own member type
        // references this inner class back) resolves to the in-progress
        // shell instead of recursing.
        if let Some(semantic_owner) = self.resolve_semantic_owner(name, &class_file)? {
            self.store.symbols.get_mut(class_symbol).owner = Some(semantic_owner);
        }

        // Parsed and entered before super_class/interfaces/fields/methods so
        // that a supertype's own generic arguments and every member's
        // signature can resolve a reference to one of this class's type
        // parameters -- see `enter_class_type_parameters`'s doc comment.
        let signature = self.resolve_optional_signature(
            name,
            &class_file.attributes,
            &class_file.constant_pool,
            ClassSignature::parse,
        )?;
        self.enter_class_type_parameters(name, declarations, signature.as_ref(), origin)?;

        let super_class = class_file
            .super_class
            .map(|index| self.resolve_dependency(name, &class_file, index))
            .transpose()?;

        let mut interfaces = Vec::with_capacity(class_file.interfaces.len());
        for index in &class_file.interfaces {
            interfaces.push(self.resolve_dependency(name, &class_file, *index)?);
        }

        let mut fields = Vec::with_capacity(class_file.fields.len());
        for field in &class_file.fields {
            let field_name =
                self.resolve_member_name(name, &class_file.constant_pool, field.name_index)?;
            let field_type = field
                .field_type(&class_file.constant_pool)
                .map_err(|error| ClassLoadError::MalformedDescriptor(name.clone(), error))?;
            let (signature, resolved_type) = self.lower_field_like(
                name,
                declarations,
                &field_type,
                &field.attributes,
                &class_file.constant_pool,
            )?;
            let annotations =
                self.resolve_annotations(name, &field.attributes, &class_file.constant_pool)?;

            let field_symbol = self.enter_field(
                class_symbol,
                declarations,
                &field_name,
                field.access_flags,
                resolved_type,
                origin,
            );
            self.enter_annotations(field_symbol, &annotations);

            fields.push(FieldSymbol::new(
                field_name,
                field_symbol,
                field.access_flags,
                field_type,
                signature,
                annotations,
            ));
        }

        let mut methods = Vec::with_capacity(class_file.methods.len());
        for method in &class_file.methods {
            let method_name =
                self.resolve_member_name(name, &class_file.constant_pool, method.name_index)?;
            let descriptor = method
                .descriptor(&class_file.constant_pool)
                .map_err(|error| ClassLoadError::MalformedDescriptor(name.clone(), error))?;
            let signature = self.resolve_optional_signature(
                name,
                &method.attributes,
                &class_file.constant_pool,
                MethodSignature::parse,
            )?;
            let annotations =
                self.resolve_annotations(name, &method.attributes, &class_file.constant_pool)?;

            // `<clinit>` is a JVM static-initializer entry point, not a
            // source-level member -- see `enter_method`'s doc comment.
            let method_symbol = if method_name != "<clinit>" {
                let parameter_names = self.resolve_method_parameter_names(
                    name,
                    &method.attributes,
                    &class_file.constant_pool,
                )?;
                let resolved_type = self.lower_method_descriptor(
                    name,
                    declarations,
                    &descriptor,
                    signature.as_ref(),
                    &parameter_names,
                    method.access_flags.is_varargs(),
                )?;
                let method_symbol = self.enter_method(
                    class_symbol,
                    declarations,
                    &method_name,
                    method.access_flags,
                    resolved_type,
                    origin,
                );
                self.enter_annotations(method_symbol, &annotations);
                Some(method_symbol)
            } else {
                None
            };

            methods.push(MethodSymbol::new(
                method_name,
                method_symbol,
                method.access_flags,
                descriptor,
                signature,
                annotations,
            ));
        }

        let nest_host = self.resolve_optional_nest_host(name, &class_file)?;
        let nest_members = self.resolve_nest_members(name, &class_file)?;
        let permitted_subclasses = self.resolve_permitted_subclasses(name, &class_file)?;
        let inner_classes = self.resolve_inner_classes(name, &class_file)?;
        let enclosing_method = self.resolve_enclosing_method(name, &class_file)?;
        let record_components = self.resolve_record_components(name, declarations, &class_file)?;
        let annotations =
            self.resolve_annotations(name, &class_file.attributes, &class_file.constant_pool)?;
        self.enter_annotations(class_symbol, &annotations);

        self.session.metadata.insert(
            class_symbol,
            ClassfileMetadata {
                binary_name: name.clone(),
                access_flags: class_file.access_flags,
                fields,
                methods,
                signature,
                nest_host,
                nest_members,
                permitted_subclasses,
                inner_classes,
                enclosing_method,
                record_components,
                annotations,
            },
        );

        self.complete_class(class_symbol, declarations, super_class, interfaces);
        Ok(class_symbol)
    }

    /// Looks for a `NestHost` attribute (JVMS §4.7.28) — present only on
    /// a class that is a nest *member*, naming its nest host.
    ///
    /// Kept as `ClassRef::Unresolved`, deliberately *not* loaded: unlike
    /// a field/method's declared type (`docs/classloader.md` §9,
    /// Milestone 6), a nest host is a structural back-reference, not
    /// type information this class needs to be understood. Eagerly
    /// loading it would also cascade into loading every other nest
    /// member (via their own `NestHost`), any of which may in turn
    /// declare *this* still-`Loading` class as a superclass/interface —
    /// e.g. a sealed interface's own permitted-subclass records
    /// implementing it (`sealed_record_sample`) — which would collide
    /// with [`Self::resolve_dependency`]'s strict cycle detection over
    /// an ordering artifact, not a real supertype cycle.
    fn resolve_optional_nest_host(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Option<ClassRef>, ClassLoadError> {
        let index = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::NestHost(index) => Some(*index),
                _ => None,
            });
        let Some(index) = index else {
            return Ok(None);
        };

        self.resolve_name(owner, class_file, index)
            .map(|dependency| Some(ClassRef::Unresolved(dependency)))
    }

    /// Looks for a `NestMembers` attribute (JVMS §4.7.29) — present only
    /// on a nest *host*, listing its members. Kept `Unresolved`, for the
    /// same reason as [`Self::resolve_optional_nest_host`].
    fn resolve_nest_members(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Vec<ClassRef>, ClassLoadError> {
        let indices = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::NestMembers(indices) => Some(indices.clone()),
                _ => None,
            })
            .unwrap_or_default();

        indices
            .into_iter()
            .map(|index| {
                self.resolve_name(owner, class_file, index)
                    .map(ClassRef::Unresolved)
            })
            .collect()
    }

    /// Looks for a `PermittedSubclasses` attribute (JVMS §4.7.31) —
    /// present only on a `sealed` class/interface, listing the classes
    /// permitted to extend/implement it. Kept `Unresolved`, for the
    /// same reason as [`Self::resolve_optional_nest_host`]: a sealed
    /// type's permitted subclasses are exactly the kind of "class
    /// mentions its own not-yet-loaded implementors" shape that made
    /// eager `NestMembers` resolution collide with cycle detection.
    fn resolve_permitted_subclasses(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Vec<ClassRef>, ClassLoadError> {
        let indices = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::PermittedSubclasses(indices) => Some(indices.clone()),
                _ => None,
            })
            .unwrap_or_default();

        indices
            .into_iter()
            .map(|index| {
                self.resolve_name(owner, class_file, index)
                    .map(ClassRef::Unresolved)
            })
            .collect()
    }

    /// Looks for an `InnerClasses` attribute (JVMS §4.7.6), mapping
    /// each decoded entry into an [`InnerClassEntry`]. `inner_class`/
    /// `outer_class` are kept `Unresolved`, for the same reason as
    /// [`Self::resolve_optional_nest_host`]; `inner_name` resolves via
    /// the same "Utf8 index -> owned String" helper member names
    /// already use.
    fn resolve_inner_classes(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Vec<InnerClassEntry>, ClassLoadError> {
        let entries = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::InnerClasses(entries) => Some(entries.clone()),
                _ => None,
            })
            .unwrap_or_default();

        entries
            .into_iter()
            .map(|entry| {
                let inner_class = ClassRef::Unresolved(self.resolve_name(
                    owner,
                    class_file,
                    entry.inner_class_info_index,
                )?);
                let outer_class = entry
                    .outer_class_info_index
                    .map(|index| self.resolve_name(owner, class_file, index))
                    .transpose()?
                    .map(ClassRef::Unresolved);
                let inner_name = entry
                    .inner_name_index
                    .map(|index| self.resolve_member_name(owner, &class_file.constant_pool, index))
                    .transpose()?;

                Ok(InnerClassEntry {
                    inner_class,
                    outer_class,
                    inner_name,
                    access_flags: ClassAccessFlags(entry.inner_class_access_flags),
                })
            })
            .collect()
    }

    /// Looks for an `EnclosingMethod` attribute (JVMS §4.7.7) — present
    /// only on a local or anonymous class. `class` (the immediately
    /// enclosing class) is kept `Unresolved`, for the same reason as
    /// [`Self::resolve_optional_nest_host`]. `method` is `None` when
    /// the class is enclosed directly by a class body (an initializer)
    /// rather than a method/constructor; when present, it's resolved
    /// to its raw `(name, descriptor)` pair, not parsed/resolved
    /// further — see [`EnclosingMethodRef`]'s doc comment for why.
    fn resolve_enclosing_method(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Option<EnclosingMethodRef>, ClassLoadError> {
        let found = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::EnclosingMethod {
                    class_index,
                    method_index,
                } => Some((*class_index, *method_index)),
                _ => None,
            });
        let Some((class_index, method_index)) = found else {
            return Ok(None);
        };

        let class = ClassRef::Unresolved(self.resolve_name(owner, class_file, class_index)?);
        let method = method_index
            .map(|index| {
                class_file
                    .constant_pool
                    .name_and_type(index)
                    .map(|(name, descriptor)| (name.to_owned(), descriptor.to_owned()))
                    .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))
            })
            .transpose()?;

        Ok(Some(EnclosingMethodRef { class, method }))
    }

    /// Resolves `name`'s real semantic owner — the enclosing class, for a
    /// nested/local/anonymous class — distinct from `Visibility::Package`,
    /// which stays the actual package symbol regardless of nesting depth
    /// (a package-private nested class is still visible package-wide, not
    /// only within its enclosing class). Returns `None` for a top-level
    /// class, whose owner stays the package `enter_class` already set.
    ///
    /// JVMS §4.7.6 requires a nested class's own class file to carry a
    /// *self*-referencing `InnerClasses` entry (`inner_class` == `name`):
    /// for a member class this entry also carries `outer_class`; for a
    /// local/anonymous class it does not (confirmed via `javap -p -v`
    /// against the real `nested_sample/NestedSample$Inner.class` and
    /// `NestedSample$1LocalRunnable.class` fixtures — see
    /// `resolves_the_inner_classes_of_a_real_nest_host`'s doc comment).
    /// A local/anonymous class instead falls back to its `EnclosingMethod`
    /// attribute's own `class` reference.
    ///
    /// The owner this returns is always the enclosing *class*, never the
    /// specific enclosing *method* real `dotc` would use for a local class
    /// — matching a JVM `EnclosingMethod`'s raw `(name, descriptor)` pair
    /// back to one of that method's own overloads' `SymbolId` would need
    /// structural descriptor-vs-`Type` matching this loader does nowhere
    /// else. The enclosing class is a stable, reasonable approximation;
    /// the raw `(name, descriptor)` pair itself is still available via
    /// `ClassfileMetadata::enclosing_method` for anyone who needs it.
    ///
    /// Resolved through [`Self::resolve_member_class`], not
    /// [`Self::resolve_dependency`]: an outer class referencing this
    /// class back (e.g. a member field typed as its own inner class) is a
    /// legitimate mutual reference, not a supertype cycle.
    fn resolve_semantic_owner(
        &mut self,
        name: &BinaryName,
        class_file: &ClassFile<'_>,
    ) -> Result<Option<SymbolId>, ClassLoadError> {
        let self_entry = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::InnerClasses(entries) => entries.iter().find(|entry| {
                    self.resolve_name(name, class_file, entry.inner_class_info_index)
                        .is_ok_and(|inner_class| inner_class == *name)
                }),
                _ => None,
            });

        let outer_class_name = match self_entry.and_then(|entry| entry.outer_class_info_index) {
            Some(index) => Some(self.resolve_name(name, class_file, index)?),
            None => {
                let enclosing_method =
                    class_file
                        .attributes
                        .iter()
                        .find_map(|attribute| match attribute {
                            Attribute::EnclosingMethod { class_index, .. } => Some(*class_index),
                            _ => None,
                        });
                enclosing_method
                    .map(|index| self.resolve_name(name, class_file, index))
                    .transpose()?
            }
        };

        outer_class_name
            .map(|outer_class| self.resolve_member_class(name, &outer_class))
            .transpose()
    }

    /// Resolves a `Utf8` constant-pool reference (a field or method's
    /// `name_index`) to an owned `String`.
    fn resolve_member_name(
        &self,
        owner: &BinaryName,
        constant_pool: &dotty_classfile::constant_pool::ConstantPool,
        index: ConstantPoolIndex,
    ) -> Result<String, ClassLoadError> {
        constant_pool
            .utf8(index)
            .map(str::to_owned)
            .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))
    }

    /// Looks for a `Signature` attribute (JVMS §4.7.9.1) among
    /// `attributes`; `None` if there isn't one (the common case for a
    /// non-generic class/field/method). If there is one, resolves its
    /// constant-pool index and parses it with `parse` — generic so
    /// `ClassSignature::parse`/`MethodSignature::parse`/
    /// `FieldSignature::parse` can all reuse this one helper.
    fn resolve_optional_signature<S>(
        &self,
        owner: &BinaryName,
        attributes: &[Attribute<'_>],
        constant_pool: &ConstantPool,
        parse: impl FnOnce(&str) -> Result<S, SignatureError>,
    ) -> Result<Option<S>, ClassLoadError> {
        let index = attributes.iter().find_map(|attribute| match attribute {
            Attribute::Signature(index) => Some(*index),
            _ => None,
        });
        let Some(index) = index else {
            return Ok(None);
        };

        let text = constant_pool
            .utf8(index)
            .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))?;
        parse(text)
            .map(Some)
            .map_err(|error| ClassLoadError::MalformedSignature(owner.clone(), error))
    }

    /// Resolves a constant-pool `Class` reference to a [`BinaryName`],
    /// without loading it.
    fn resolve_name(
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
        index: ConstantPoolIndex,
    ) -> Result<BinaryName, ClassLoadError> {
        class_file
            .constant_pool
            .class_name(index)
            .map(BinaryName::from_internal)
            .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))
    }

    /// Resolves a constant-pool `Class` reference (a superclass or
    /// interface) and loads it, wrapping any failure as a
    /// [`ClassLoadError::DependencyFailure`] against `owner`.
    ///
    /// Unlike [`Self::resolve_member_class`], hitting an already-`Loading`
    /// entry here is *not* tolerated: a class being its own (in)direct
    /// supertype is a hard JVMS §5.3.5 error, not a legitimate mutual
    /// reference (`docs/classloader.md` §5) — `self.load_class(&dependency)`
    /// reports that as `CircularInheritance`, unchanged.
    fn resolve_dependency(
        &mut self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
        index: ConstantPoolIndex,
    ) -> Result<SymbolId, ClassLoadError> {
        let dependency = self.resolve_name(owner, class_file, index)?;
        self.load_dependency(owner, dependency)
    }

    /// Loads `dependency` (a superclass or interface, already resolved
    /// to a name) and wraps a failure as [`ClassLoadError::DependencyFailure`]
    /// — the format-agnostic half of dependency resolution shared by
    /// both the `.class` path (via [`Self::resolve_dependency`], which
    /// resolves a constant-pool index to a name first) and the `.tasty`
    /// path (which already has the name from [`tasty_symbol::decode`]).
    fn load_dependency(
        &mut self,
        owner: &BinaryName,
        dependency: BinaryName,
    ) -> Result<SymbolId, ClassLoadError> {
        self.load_class(&dependency)
            .map_err(|source| ClassLoadError::DependencyFailure {
                owner: owner.clone(),
                dependency,
                source: Rc::new(source),
            })
    }

    /// Resolves a class referenced by a member's declared type (a field
    /// or method's `Object`/array-of-`Object` descriptor entry),
    /// tolerant of `owner`'s own load reaching back around to
    /// `dependency` while `dependency` is still `Loading` — the
    /// "reusing an in-progress `SymbolId`" technique `docs/classloader.md`
    /// §5/§9 (Milestone 6) describes: two classes each having a field or
    /// method typed as the other is completely legitimate Java, unlike
    /// a supertype cycle, so hitting `Loading` here returns the
    /// in-progress `SymbolId` directly instead of erroring or re-entering
    /// [`Self::load_class`]. Every other state (`Loaded`/`Failed`/
    /// absent) just delegates to `load_class` as normal.
    fn resolve_member_class(
        &mut self,
        owner: &BinaryName,
        dependency: &BinaryName,
    ) -> Result<SymbolId, ClassLoadError> {
        let loading = match self.repository.get(dependency) {
            Some(ClassEntry::Loading(symbol)) => Some(*symbol),
            _ => None,
        };
        if let Some(symbol) = loading {
            return Ok(symbol);
        }

        self.load_class(dependency)
            .map_err(|source| ClassLoadError::DependencyFailure {
                owner: owner.clone(),
                dependency: dependency.clone(),
                source: Rc::new(source),
            })
    }

    /// Walks an already-parsed [`MethodDescriptor`] (JVMS §4.3.3) into a
    /// `Type::Method`'s `TypeId`, the same way [`Self::lower_field_type`]
    /// walks a field's type: each parameter lowers via
    /// [`Self::lower_field_type`], and a missing `return_type` (`void`)
    /// lowers to [`Definitions::unit`]. `parameter_names[i]` (from a
    /// `MethodParameters` attribute when present, see
    /// [`Self::resolve_method_parameter_names`]) names parameter `i`; a
    /// missing entry gets a stable synthetic `p{i}` name instead — a
    /// method's semantic shape does not depend on debug/source metadata
    /// existing. `varargs` marks the *last* parameter's
    /// `MethodParam::varargs` (JVMS guarantees `ACC_VARARGS` only ever
    /// applies to the last parameter), so `f(String... xs)` stays
    /// distinguishable from `f(String[] xs)`.
    ///
    /// `signature` (a method's optional `Signature` attribute) is preferred
    /// over the erased descriptor per-parameter/result — e.g. a bare `T`
    /// return type instead of its erased bound — but only when its
    /// parameter count matches the descriptor's exactly. A mismatch is not
    /// malformed input: JVMS §4.7.9.1 allows a `MethodSignature` to omit
    /// leading synthetic/mandated parameters (e.g. an inner class
    /// constructor's implicit outer-instance parameter) that the descriptor
    /// still carries, and this loader does not yet track which parameters
    /// those are — so a mismatched signature is simply not used, falling
    /// back to the descriptor entirely, rather than risk misaligning names/
    /// types across the two lists.
    ///
    /// A method that declares type parameters of its own delegates to
    /// [`Self::lower_generic_method_descriptor`] instead, wrapping the
    /// result in `Type::Poly` — JVMS requires such a method to carry a
    /// `Signature` attribute, so an aligned one is always available there.
    fn lower_method_descriptor(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        descriptor: &MethodDescriptor,
        signature: Option<&MethodSignature>,
        parameter_names: &[Option<String>],
        varargs: bool,
    ) -> Result<TypeId, ClassLoadError> {
        let aligned =
            signature.filter(|signature| signature.parameters.len() == descriptor.parameters.len());
        if let Some(signature) = aligned.filter(|signature| !signature.type_parameters.is_empty()) {
            return self.lower_generic_method_descriptor(
                owner,
                declarations,
                signature,
                parameter_names,
                varargs,
            );
        }
        let signature = aligned.filter(|signature| signature.type_parameters.is_empty());

        let last_index = descriptor.parameters.len().checked_sub(1);
        let env = TypeVarEnv::class_only(declarations);
        let mut params = Vec::with_capacity(descriptor.parameters.len());
        for (index, parameter) in descriptor.parameters.iter().enumerate() {
            let ty = match signature.map(|signature| &signature.parameters[index]) {
                Some(signature_type) => self.lower_type_signature(owner, &env, signature_type)?,
                None => self.lower_field_type(owner, parameter)?,
            };
            let synthetic_name = format!("p{index}");
            let param_name = parameter_names
                .get(index)
                .and_then(|name| name.as_deref())
                .unwrap_or(&synthetic_name);
            let text = self.store.names.intern(param_name);
            params.push(MethodParam {
                name: TermName::new(text),
                ty,
                erased: false,
                varargs: varargs && Some(index) == last_index,
            });
        }

        let result = match signature.and_then(|signature| signature.result.as_ref()) {
            Some(signature_type) => self.lower_type_signature(owner, &env, signature_type)?,
            None => match &descriptor.return_type {
                Some(field_type) => self.lower_field_type(owner, field_type)?,
                None => self.definitions.unit,
            },
        };

        Ok(self.store.types.alloc(Type::Method(MethodType {
            params,
            result,
            kind: MethodKind::Plain,
        })))
    }

    /// Lowers a method that declares generic type parameters of its own
    /// (`signature.type_parameters` non-empty, already confirmed aligned
    /// with the descriptor by [`Self::lower_method_descriptor`]) into
    /// `Type::Poly(PolyType { params, result: <a Type::Method TypeId> })`.
    ///
    /// Built via the same two-phase `TypeArena::reserve`/`fill` construction
    /// `docs/dotty-core-design.md` §8 describes for any binder whose own
    /// bounds may reference it (F-bounded polymorphism, e.g.
    /// `<T extends Comparable<T>> T max(T a, T b)`): the `Poly`'s `TypeId`
    /// is reserved first, each type parameter's `Type::ParamRef` is
    /// allocated against that reserved id, and only then are the type
    /// parameters' bounds and the method's own parameters/result lowered —
    /// with a `TypeVarEnv` whose `method_type_params` resolves a
    /// `TypeVariable` naming one of *this* method's own type parameters to
    /// its `ParamRef`, shadowing (per JVM scoping) an enclosing class type
    /// parameter of the same name.
    fn lower_generic_method_descriptor(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        signature: &MethodSignature,
        parameter_names: &[Option<String>],
        varargs: bool,
    ) -> Result<TypeId, ClassLoadError> {
        let poly_binder = self.store.types.reserve();
        let poly_id = poly_binder.id();

        let mut method_type_params = HashMap::with_capacity(signature.type_parameters.len());
        for (index, parameter) in signature.type_parameters.iter().enumerate() {
            let param_ref = self.store.types.alloc(Type::ParamRef {
                binder: poly_id,
                index: index as u32,
            });
            method_type_params.insert(parameter.name.clone(), param_ref);
        }
        let env = TypeVarEnv {
            declarations,
            method_type_params: Some(&method_type_params),
        };

        let mut type_params = Vec::with_capacity(signature.type_parameters.len());
        for parameter in &signature.type_parameters {
            let high = match &parameter.class_bound {
                Some(bound) => self.lower_reference_type_signature(owner, &env, bound)?,
                None => self.type_ref(self.definitions.object_class),
            };
            let high = parameter.interface_bounds.iter().try_fold(
                high,
                |left, interface_bound| -> Result<TypeId, ClassLoadError> {
                    let right =
                        self.lower_reference_type_signature(owner, &env, interface_bound)?;
                    Ok(self.store.types.alloc(Type::And { left, right }))
                },
            )?;
            let text = self.store.names.intern(&parameter.name);
            type_params.push(TypeParam {
                name: TypeName::new(text),
                bounds: high,
                declared_variance: None,
            });
        }

        let last_index = signature.parameters.len().checked_sub(1);
        let mut params = Vec::with_capacity(signature.parameters.len());
        for (index, parameter_signature) in signature.parameters.iter().enumerate() {
            let ty = self.lower_type_signature(owner, &env, parameter_signature)?;
            let synthetic_name = format!("p{index}");
            let param_name = parameter_names
                .get(index)
                .and_then(|name| name.as_deref())
                .unwrap_or(&synthetic_name);
            let text = self.store.names.intern(param_name);
            params.push(MethodParam {
                name: TermName::new(text),
                ty,
                erased: false,
                varargs: varargs && Some(index) == last_index,
            });
        }

        let result = match &signature.result {
            Some(signature_type) => self.lower_type_signature(owner, &env, signature_type)?,
            None => self.definitions.unit,
        };

        let method_type = self.store.types.alloc(Type::Method(MethodType {
            params,
            result,
            kind: MethodKind::Plain,
        }));

        Ok(self.store.types.fill(
            poly_binder,
            Type::Poly(PolyType {
                params: type_params,
                result: method_type,
            }),
        ))
    }

    /// Looks for a `MethodParameters` attribute (JVMS §4.7.24) among
    /// `attributes`. Each entry is `None` if its `name_index` is `0` (JVMS:
    /// "no name") or the attribute is absent entirely — an empty
    /// `Vec` when the attribute is missing, matching every other
    /// `resolve_optional_*` helper's "not present" shape. Callers fall
    /// back to a synthetic name for any index this returns `None` for or
    /// does not cover.
    fn resolve_method_parameter_names(
        &self,
        owner: &BinaryName,
        attributes: &[Attribute<'_>],
        constant_pool: &ConstantPool,
    ) -> Result<Vec<Option<String>>, ClassLoadError> {
        let entries = attributes.iter().find_map(|attribute| match attribute {
            Attribute::MethodParameters(entries) => Some(entries),
            _ => None,
        });
        let Some(entries) = entries else {
            return Ok(Vec::new());
        };

        entries
            .iter()
            .map(|entry| {
                entry
                    .name_index
                    .map(|index| self.resolve_member_name(owner, constant_pool, index))
                    .transpose()
            })
            .collect()
    }

    /// Allocates a method/constructor's `Symbol` and enters it into the
    /// class's declarations scope by name — like a field, a method needs
    /// no enter-before-complete staging, since its `Type::Method` is fully
    /// known the moment its descriptor (and any class types it
    /// references) resolves. `<clinit>` is deliberately never passed here
    /// (see the call site): it is a JVM static-initializer entry point,
    /// not a source-level member a Scala program could ever refer to by
    /// name, so it stays metadata-only rather than getting a `Symbol`.
    /// `<init>` becomes `SymbolKind::Constructor`; every other name
    /// becomes `SymbolKind::Method`.
    fn enter_method(
        &mut self,
        class_symbol: SymbolId,
        declarations: ScopeId,
        method_name: &str,
        flags: MethodAccessFlags,
        resolved_type: TypeId,
        origin: SymbolOrigin,
    ) -> SymbolId {
        let visibility = if flags.is_private() {
            Visibility::Private
        } else if flags.is_protected() {
            Visibility::Protected
        } else if flags.is_public() {
            Visibility::Public
        } else {
            let owner = self
                .store
                .symbols
                .get(class_symbol)
                .owner
                .expect("a loaded class always has a package owner");
            Visibility::Package(owner)
        };

        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_static() {
            symbol_flags = symbol_flags | SymbolFlags::STATIC;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        }
        if flags.is_abstract() {
            symbol_flags = symbol_flags | SymbolFlags::ABSTRACT;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }

        let kind = if method_name == "<init>" {
            SymbolKind::Constructor
        } else {
            SymbolKind::Method
        };

        let text = self.store.names.intern(method_name);
        let symbol_name = Name::new(text, Namespace::Term);

        let method_symbol = self.store.symbols.alloc(Symbol {
            name: symbol_name,
            owner: Some(class_symbol),
            kind,
            flags: symbol_flags,
            visibility,
            info: SymbolInfo::Complete(resolved_type),
            origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });

        self.store
            .scopes
            .get_mut(declarations)
            .enter(symbol_name, method_symbol);
        method_symbol
    }

    /// Looks for a `Record` attribute (JVMS §4.7.30) — present only on
    /// a `record` class, listing its components. `None` means the
    /// class isn't a record at all; `Some(vec![])` means it is a record
    /// with zero components — both are real, distinguishable states.
    fn resolve_record_components(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        class_file: &ClassFile<'_>,
    ) -> Result<Option<Vec<RecordComponentSymbol>>, ClassLoadError> {
        let components = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Record(components) => Some(components),
                _ => None,
            });
        let Some(components) = components else {
            return Ok(None);
        };

        let mut resolved = Vec::with_capacity(components.len());
        for component in components {
            let component_name =
                self.resolve_member_name(owner, &class_file.constant_pool, component.name_index)?;
            let descriptor_text = class_file
                .constant_pool
                .utf8(component.descriptor_index)
                .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))?;
            let field_type = FieldType::parse(descriptor_text).map_err(|error| {
                ClassLoadError::MalformedDescriptor(owner.clone(), error.into())
            })?;
            let (signature, resolved_type) = self.lower_field_like(
                owner,
                declarations,
                &field_type,
                &component.attributes,
                &class_file.constant_pool,
            )?;
            resolved.push(RecordComponentSymbol::new(
                component_name,
                field_type,
                signature,
                resolved_type,
            ));
        }

        Ok(Some(resolved))
    }

    /// Looks for `RuntimeVisibleAnnotations`/`RuntimeInvisibleAnnotations`
    /// (JVMS §4.7.16/4.7.17) among `attributes`, flattening both into
    /// one list — retention visibility isn't modeled, per
    /// `docs/classloader.md` §9's Milestone 7 scope decision. Shared by
    /// the class itself, each field, and each method.
    fn resolve_annotations(
        &mut self,
        owner: &BinaryName,
        attributes: &[Attribute<'_>],
        constant_pool: &ConstantPool,
    ) -> Result<Vec<SemanticAnnotation>, ClassLoadError> {
        attributes
            .iter()
            .filter_map(|attribute| match attribute {
                Attribute::RuntimeVisibleAnnotations(annotations)
                | Attribute::RuntimeInvisibleAnnotations(annotations) => Some(annotations),
                _ => None,
            })
            .flatten()
            .map(|annotation| self.resolve_annotation(owner, annotation, constant_pool))
            .collect()
    }

    /// Resolves one decoded [`Annotation`] into a [`SemanticAnnotation`].
    ///
    /// `annotation_type` is resolved best-effort, the same tolerant way
    /// [`Self::resolve_member_class`] resolves a member's declared type,
    /// but — unlike a field/method's declared type — a failure to load it
    /// does *not* fail the whole class: real-world bytecode routinely
    /// carries annotations (build/processor-only ones especially) whose
    /// interface is absent from the runtime classpath, and JVM reflection
    /// itself only fails lazily when that specific annotation is
    /// inspected, not at class-load time. So an unresolvable type falls
    /// back to `ClassRef::Unresolved` here rather than propagating a
    /// [`ClassLoadError`] — see [`Self::enter_annotations`] for what that
    /// means for the resulting `dotty_core::Annotation`.
    fn resolve_annotation(
        &mut self,
        owner: &BinaryName,
        annotation: &Annotation,
        constant_pool: &ConstantPool,
    ) -> Result<SemanticAnnotation, ClassLoadError> {
        let type_descriptor_text = constant_pool
            .utf8(annotation.type_index)
            .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))?;
        let parsed_type = FieldType::parse(type_descriptor_text)
            .map_err(|error| ClassLoadError::MalformedDescriptor(owner.clone(), error.into()))?;
        let annotation_type = match parsed_type {
            FieldType::Object(class_name) => {
                let dependency = BinaryName::from_internal(&class_name);
                match self.resolve_member_class(owner, &dependency) {
                    Ok(symbol) => ClassRef::Resolved(symbol),
                    Err(_) => ClassRef::Unresolved(dependency),
                }
            }
            // JVMS §4.7.16 guarantees an annotation's own type descriptor
            // names a class type; a primitive/array shape here can only
            // come from a malformed class file, not real javac output.
            _ => {
                return Err(ClassLoadError::MalformedReference(
                    owner.clone(),
                    PoolRefError::WrongKind {
                        index: annotation.type_index,
                        expected: EntryKind::Class,
                    },
                ));
            }
        };

        let elements = annotation
            .element_value_pairs
            .iter()
            .map(|(name_index, value)| {
                let element_name = self.resolve_member_name(owner, constant_pool, *name_index)?;
                let element_value = self.resolve_element_value(owner, value, constant_pool)?;
                Ok((element_name, element_value))
            })
            .collect::<Result<Vec<_>, ClassLoadError>>()?;

        Ok(SemanticAnnotation {
            annotation_type,
            elements,
        })
    }

    /// Allocates a `dotty_core::Annotation`/`AnnotationId` for each of
    /// `annotations` whose `annotation_type` resolved to a real `SymbolId`
    /// (see [`Self::resolve_annotation`]'s doc comment on why one that
    /// could not be loaded is silently skipped here rather than treated as
    /// an error), and attaches the resulting list to `symbol`'s own
    /// `Symbol::annotations` — dotty-core's canonical, argument-free "does
    /// this symbol carry annotation X" list. The full element values stay
    /// exclusively in `annotations` (the JVM-facing `SemanticAnnotation`/
    /// `AnnotationValue` sidecar shape already stored on
    /// [`ClassfileMetadata`]/[`FieldSymbol`]/[`MethodSymbol`]) — a
    /// classfile-decoded value has no typed source tree to become a
    /// `dotty_core::Annotation::tree`, so `tree` is always `None` here.
    fn enter_annotations(&mut self, symbol: SymbolId, annotations: &[SemanticAnnotation]) {
        let ids = annotations
            .iter()
            .filter_map(|annotation| match &annotation.annotation_type {
                ClassRef::Resolved(annotation_symbol) => {
                    let ty = self.type_ref(*annotation_symbol);
                    Some(self.store.annotations.alloc(CoreAnnotation::new(ty, None)))
                }
                ClassRef::Unresolved(_) => None,
            })
            .collect();
        self.store.symbols.get_mut(symbol).annotations = ids;
    }

    /// Resolves one decoded [`ElementValue`] (JVMS §4.7.16.1) into an
    /// [`AnnotationValue`]. Class names mentioned inside a value
    /// (`Enum`'s type, `Class`'s payload) are kept as raw strings, not
    /// resolved — see [`AnnotationValue`]'s doc comment.
    fn resolve_element_value(
        &mut self,
        owner: &BinaryName,
        value: &ElementValue,
        constant_pool: &ConstantPool,
    ) -> Result<AnnotationValue, ClassLoadError> {
        let integer =
            |index: ConstantPoolIndex| self.resolve_pool_integer(owner, constant_pool, index);

        match value {
            ElementValue::Byte(index) => integer(*index).map(AnnotationValue::Byte),
            ElementValue::Char(index) => integer(*index).map(AnnotationValue::Char),
            ElementValue::Int(index) => integer(*index).map(AnnotationValue::Int),
            ElementValue::Short(index) => integer(*index).map(AnnotationValue::Short),
            ElementValue::Boolean(index) => {
                integer(*index).map(|value| AnnotationValue::Boolean(value != 0))
            }
            ElementValue::Long(index) => self
                .resolve_pool_long(owner, constant_pool, *index)
                .map(AnnotationValue::Long),
            ElementValue::Float(index) => self
                .resolve_pool_float(owner, constant_pool, *index)
                .map(AnnotationValue::Float),
            ElementValue::Double(index) => self
                .resolve_pool_double(owner, constant_pool, *index)
                .map(AnnotationValue::Double),
            ElementValue::String(index) => constant_pool
                .utf8(*index)
                .map(|text| AnnotationValue::String(text.to_owned()))
                .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error)),
            ElementValue::Enum {
                type_name_index,
                const_name_index,
            } => {
                let type_descriptor = constant_pool
                    .utf8(*type_name_index)
                    .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))?
                    .to_owned();
                let const_name = constant_pool
                    .utf8(*const_name_index)
                    .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error))?
                    .to_owned();
                Ok(AnnotationValue::Enum {
                    type_descriptor,
                    const_name,
                })
            }
            ElementValue::Class(index) => constant_pool
                .utf8(*index)
                .map(|text| AnnotationValue::Class(text.to_owned()))
                .map_err(|error| ClassLoadError::MalformedReference(owner.clone(), error)),
            ElementValue::Annotation(nested) => self
                .resolve_annotation(owner, nested, constant_pool)
                .map(|annotation| AnnotationValue::Annotation(Box::new(annotation))),
            ElementValue::Array(values) => values
                .iter()
                .map(|value| self.resolve_element_value(owner, value, constant_pool))
                .collect::<Result<Vec<_>, ClassLoadError>>()
                .map(AnnotationValue::Array),
        }
    }

    fn resolve_pool_integer(
        &self,
        owner: &BinaryName,
        constant_pool: &ConstantPool,
        index: ConstantPoolIndex,
    ) -> Result<i32, ClassLoadError> {
        match constant_pool.get(index) {
            Some(ConstantPoolEntry::Integer(value)) => Ok(*value),
            Some(_) => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::WrongKind {
                    index,
                    expected: EntryKind::Integer,
                },
            )),
            None => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::InvalidIndex { index },
            )),
        }
    }

    fn resolve_pool_long(
        &self,
        owner: &BinaryName,
        constant_pool: &ConstantPool,
        index: ConstantPoolIndex,
    ) -> Result<i64, ClassLoadError> {
        match constant_pool.get(index) {
            Some(ConstantPoolEntry::Long(value)) => Ok(*value),
            Some(_) => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::WrongKind {
                    index,
                    expected: EntryKind::Long,
                },
            )),
            None => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::InvalidIndex { index },
            )),
        }
    }

    fn resolve_pool_float(
        &self,
        owner: &BinaryName,
        constant_pool: &ConstantPool,
        index: ConstantPoolIndex,
    ) -> Result<f32, ClassLoadError> {
        match constant_pool.get(index) {
            Some(ConstantPoolEntry::Float(value)) => Ok(*value),
            Some(_) => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::WrongKind {
                    index,
                    expected: EntryKind::Float,
                },
            )),
            None => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::InvalidIndex { index },
            )),
        }
    }

    fn resolve_pool_double(
        &self,
        owner: &BinaryName,
        constant_pool: &ConstantPool,
        index: ConstantPoolIndex,
    ) -> Result<f64, ClassLoadError> {
        match constant_pool.get(index) {
            Some(ConstantPoolEntry::Double(value)) => Ok(*value),
            Some(_) => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::WrongKind {
                    index,
                    expected: EntryKind::Double,
                },
            )),
            None => Err(ClassLoadError::MalformedReference(
                owner.clone(),
                PoolRefError::InvalidIndex { index },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::{
        ClassFormat, ClassOrigin, ClassPathError, ClassResource, CompositeClassPath,
    };
    use crate::tasty_symbol::TastyDecodeError;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    /// The class's `Type::ClassInfo`, read back through its `Symbol`'s
    /// `SymbolInfo::Complete` — the sole path to a class's parents/
    /// declarations scope after `ClassLoader::load_class` returns.
    fn class_info(store: &SemanticStore, id: SymbolId) -> &ClassInfo {
        let symbol = store.symbols.get(id);
        let SymbolInfo::Complete(info_id) = symbol.info else {
            panic!("expected the class to have complete info");
        };
        let Type::ClassInfo(class_info) = store.types.get(info_id) else {
            panic!("expected a ClassInfo");
        };
        class_info
    }

    /// A symbol's resolved (interned) simple name.
    fn symbol_name(store: &SemanticStore, id: SymbolId) -> &str {
        store.names.resolve(store.symbols.get(id).name.text())
    }

    /// The `SymbolId` a `Type::TypeRef` (a `ClassInfo` parent) points at.
    fn parent_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
        let Type::TypeRef { symbol, .. } = store.types.get(ty) else {
            panic!("expected a TypeRef");
        };
        *symbol
    }

    /// The resolved `TypeId` of the field or method named `member_name`,
    /// looked up through `class_symbol`'s declarations scope — the sole
    /// canonical source for a class's members.
    fn member_type_id(
        store: &mut SemanticStore,
        class_symbol: SymbolId,
        member_name: &str,
    ) -> TypeId {
        let declarations = class_info(store, class_symbol).declarations;
        let text = store.names.intern(member_name);
        let name = Name::new(text, Namespace::Term);
        let member_symbol = store
            .scopes
            .get(declarations)
            .lookup(&name)
            .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"));
        let SymbolInfo::Complete(type_id) = store.symbols.get(member_symbol).info else {
            panic!("expected the member to have complete info");
        };
        type_id
    }

    /// An in-memory classpath backed by a fixed name -> bytes map, for
    /// tests that need full control over what a class's bytes are
    /// (including synthetic, hand-built, invalid, or cyclic class files
    /// that `javac` cannot produce).
    struct InMemoryClassPath(HashMap<BinaryName, Vec<u8>>);

    impl ClassPathEntry for InMemoryClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(self.0.get(name).map(|bytes| {
                ClassResource::new(
                    bytes.clone(),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<memory>")),
                )
            }))
        }
    }

    /// Like [`InMemoryClassPath`], but tags every entry
    /// [`ClassFormat::Tasty`] instead of [`ClassFormat::Class`] — for
    /// tests that need control over `.tasty` bytes specifically (real
    /// fixture bytes for the happy path, or bytes deliberately missing a
    /// dependency to exercise an error path).
    struct InMemoryTastyClassPath(HashMap<BinaryName, Vec<u8>>);

    impl ClassPathEntry for InMemoryTastyClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(self.0.get(name).map(|bytes| {
                ClassResource::new(
                    bytes.clone(),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("<memory>")),
                )
            }))
        }
    }

    struct AlwaysErrors;

    impl ClassPathEntry for AlwaysErrors {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            Err(ClassPathError::from(std::io::Error::other("boom")))
        }
    }

    fn fixture_bytes(relative_path: &str) -> Vec<u8> {
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-classfile/tests/fixtures")
                .join(relative_path),
        )
        .expect("fixture should exist")
    }

    /// Like [`fixture_bytes`], but from `dotty-tasty`'s own fixture tree
    /// — for a real `scalac`-compiled `.tasty` fixture with fields/methods
    /// (`tasty_sample/`'s own `Dog`/`Animal` are deliberately too minimal
    /// to exercise those).
    fn dotty_tasty_fixture_bytes(relative_path: &str) -> Vec<u8> {
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures")
                .join(relative_path),
        )
        .expect("fixture should exist")
    }

    /// A hand-built, minimal, synthetic class file: JVMS §4.1 header +
    /// empty constant pool + access flags + a `this_class` index that
    /// does not resolve to any constant pool entry. `javac` cannot
    /// produce this; it exists purely to exercise `MalformedReference`.
    fn synthetic_malformed_this_class() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x01]); // constant_pool_count = 1 (no entries)
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags (ACC_PUBLIC | ACC_SUPER)
        bytes.extend_from_slice(&[0x00, 0x01]); // this_class = 1 (invalid: no such entry)
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    /// A hand-built, minimal, synthetic, otherwise-valid class file
    /// declaring `major_version` 70 (one past this project's JDK-25
    /// compatibility ceiling) — exercises `UnsupportedClassVersion`
    /// without needing a real class file from a JDK newer than this
    /// project targets.
    fn synthetic_future_version_class(this_name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 70]); // major = 70 (unsupported)
        bytes.extend_from_slice(&[0x00, 0x03]); // constant_pool_count = 3
        bytes.push(1); // #1 CONSTANT_Utf8
        bytes.extend_from_slice(&(this_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(this_name.as_bytes());
        bytes.push(7); // #2 CONSTANT_Class -> #1
        bytes.extend_from_slice(&[0x00, 0x01]);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&[0x00, 0x02]); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count
        bytes
    }

    /// A hand-built, minimal, synthetic class file with the given name and
    /// optional superclass, no interfaces/fields/methods/attributes. Used
    /// where a real JDK fixture would need a JDK classpath (`java/lang/*`
    /// classes: not available before Milestone 3 — see
    /// `docs/classloader.md` §3/§9) or where the shape under test (a
    /// deliberate inheritance cycle) cannot be produced by `javac` at all.
    fn synthetic_class(this_name: &str, super_name: Option<&str>) -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;

        let this_name_index = push_utf8(&mut pool, &mut next_index, this_name);
        let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
        let super_class_index = super_name.map(|super_name| {
            let super_name_index = push_utf8(&mut pool, &mut next_index, super_name);
            push_class(&mut pool, &mut next_index, super_name_index)
        });

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        bytes.extend_from_slice(&[0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0x45]);
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&super_class_index.unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count
        bytes
    }

    /// A hand-built, minimal, synthetic class file like [`synthetic_class`],
    /// but with one `public` `Object`-typed field per `(name,
    /// field_type_binary_name)` pair in `fields`, in order — used to
    /// engineer a specific field-resolution order (e.g. a mutual
    /// reference that must succeed before a later field that must fail).
    fn synthetic_class_with_object_fields(
        this_name: &str,
        super_name: Option<&str>,
        fields: &[(&str, &str)],
    ) -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;

        let this_name_index = push_utf8(&mut pool, &mut next_index, this_name);
        let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
        let super_class_index = super_name.map(|super_name| {
            let super_name_index = push_utf8(&mut pool, &mut next_index, super_name);
            push_class(&mut pool, &mut next_index, super_name_index)
        });

        let mut field_entries = Vec::new();
        for (field_name, field_type_name) in fields {
            let name_index = push_utf8(&mut pool, &mut next_index, field_name);
            let descriptor = format!("L{field_type_name};");
            let descriptor_index = push_utf8(&mut pool, &mut next_index, &descriptor);

            field_entries.extend_from_slice(&[0x00, 0x01]); // access_flags: PUBLIC
            field_entries.extend_from_slice(&name_index.to_be_bytes());
            field_entries.extend_from_slice(&descriptor_index.to_be_bytes());
            field_entries.extend_from_slice(&[0x00, 0x00]); // attributes_count
        }

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        bytes.extend_from_slice(&[0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0x45]);
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&super_class_index.unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
        bytes.extend_from_slice(&(fields.len() as u16).to_be_bytes()); // fields_count
        bytes.extend_from_slice(&field_entries);
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count
        bytes
    }

    /// The real `PoolSample.class` fixture references `java/lang/Object`
    /// (superclass) and `java/lang/Runnable` (interface); since
    /// Milestone 6, its `GREETING` field's type also forces resolving
    /// `java/lang/String`. No JDK classpath is used in these
    /// unit-level tests (that's what the JAR/JMOD-based integration
    /// tests further down exercise), so all three are stood in with
    /// hand-built, minimal, clearly synthetic classes on the same test
    /// classpath, alongside the real fixture bytes.
    fn pool_sample_classpath() -> HashMap<BinaryName, Vec<u8>> {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("PoolSample"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Runnable"),
            synthetic_class("java/lang/Runnable", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );
        classes
    }

    /// Calling `ClassLoader::new` twice on the same `store` violates
    /// `Definitions::bootstrap`'s own documented invariant ("exactly one
    /// `Definitions` per `SemanticStore`") and mints two independent
    /// builtin identities -- this pins down exactly what that misuse
    /// looks like, so `with_definitions`'s regression test below has
    /// something concrete to contrast against.
    #[test]
    fn two_class_loaders_bootstrapping_their_own_definitions_on_one_store_diverge() {
        let mut store = SemanticStore::new();

        let loader_a = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);
        let object_a = loader_a.definitions().object_class;
        drop(loader_a);

        let loader_b = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);
        let object_b = loader_b.definitions().object_class;
        drop(loader_b);

        assert_ne!(object_a, object_b);
    }

    /// The fix for the above: bootstrap `Definitions` once, and give it
    /// to every `ClassLoader` sharing that `store` via
    /// `with_definitions` instead of letting each mint its own.
    #[test]
    fn class_loaders_sharing_one_bootstrapped_definitions_agree_on_builtin_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let loader_a = ClassLoader::with_definitions(
            InMemoryClassPath(HashMap::new()),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let object_a = loader_a.definitions().object_class;
        drop(loader_a);

        let loader_b = ClassLoader::with_definitions(
            InMemoryClassPath(HashMap::new()),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let object_b = loader_b.definitions().object_class;
        drop(loader_b);

        assert_eq!(object_a, object_b);
    }

    /// `with_definitions` shares `Object`/`Any`/`Nothing`/primitive
    /// identity, but each `ClassLoader` used to allocate its own
    /// `Type::NoPrefix` regardless -- so a `Type::ClassInfo`/`Type::TypeRef`
    /// built by one loader still had a different `prefix` `TypeId` than
    /// one built by another loader sharing the same `Definitions`, even
    /// though both `prefix`es resolve to the exact same `Type::NoPrefix`
    /// value. Fixed by making `definitions.no_prefix` the one shared
    /// `TypeId` every loader reads instead of allocating its own.
    #[test]
    fn class_loaders_sharing_one_bootstrapped_definitions_agree_on_no_prefix_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let mut classes_a = HashMap::new();
        classes_a.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        let mut loader_a = ClassLoader::with_definitions(
            InMemoryClassPath(classes_a),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let object_a = loader_a
            .load_class(&BinaryName::from_internal("java/lang/Object"))
            .expect("java/lang/Object should load");
        drop(loader_a);
        let prefix_a = class_info(&store, object_a).prefix;

        let mut classes_b = HashMap::new();
        classes_b.insert(
            BinaryName::from_internal("A"),
            synthetic_class("A", Some("java/lang/Object")),
        );
        classes_b.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        let mut loader_b = ClassLoader::with_definitions(
            InMemoryClassPath(classes_b),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let a = loader_b
            .load_class(&BinaryName::from_internal("A"))
            .expect("A should load");
        drop(loader_b);
        let prefix_b = class_info(&store, a).prefix;

        assert_eq!(prefix_a, prefix_b);
        assert_eq!(prefix_a, definitions.no_prefix);
    }

    /// `with_definitions` shares builtin identity, and `no_prefix`
    /// identity, but each `ClassLoader` used to still allocate its own
    /// fresh `ClassRepository`/`PackageRegistry` regardless -- so an
    /// *ordinary* class or package (not a `Definitions` builtin) loaded by
    /// one loader got a different `SymbolId` than the same class/package
    /// loaded by a second loader sharing the same `store`, even though
    /// both name the exact same class. Fixed by threading one
    /// `LoadingSession` through every loader sharing a `store`, via
    /// `into_session`/`with_definitions`, instead of each loader minting
    /// its own.
    #[test]
    fn class_loaders_sharing_one_loading_session_agree_on_ordinary_class_and_package_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let mut classes_a = HashMap::new();
        classes_a.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes_a.insert(
            BinaryName::from_internal("java/util/List"),
            synthetic_class("java/util/List", Some("java/lang/Object")),
        );
        let mut loader_a = ClassLoader::with_definitions(
            InMemoryClassPath(classes_a),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let list_a = loader_a
            .load_class(&BinaryName::from_internal("java/util/List"))
            .expect("java/util/List should load");
        let session = loader_a.into_session();
        let package_a = store.symbols.get(list_a).owner.expect("List has an owner");

        let mut classes_b = HashMap::new();
        classes_b.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes_b.insert(
            BinaryName::from_internal("java/util/List"),
            synthetic_class("java/util/List", Some("java/lang/Object")),
        );
        classes_b.insert(
            BinaryName::from_internal("A"),
            synthetic_class("A", Some("java/util/List")),
        );
        let mut loader_b = ClassLoader::with_definitions(
            InMemoryClassPath(classes_b),
            &mut store,
            definitions,
            session,
        );
        let a = loader_b
            .load_class(&BinaryName::from_internal("A"))
            .expect("A should load");
        drop(loader_b);

        let list_b = parent_symbol(&store, class_info(&store, a).parents[0]);
        assert_eq!(
            list_a, list_b,
            "the second loader should reuse the first loader's List SymbolId, not mint a new one"
        );
        let package_b = store.symbols.get(list_b).owner.expect("List has an owner");
        assert_eq!(
            package_a, package_b,
            "the second loader should reuse the first loader's java/util package SymbolId"
        );
    }

    /// The counterpart regression to the test above, for the opposite
    /// mistake: a `LoadingSession` must share only *positive* results.
    /// Loader A's own classpath does not contain `Extra` -- it correctly
    /// fails with `NotFound`. Loader B, sharing that session, has a
    /// completely different classpath that *does* contain `Extra`; if
    /// loader A's negative result had been cached in the shared session
    /// (as it briefly was, before `LoadingSession` was split into a
    /// shared positive-only `resolved` map plus a loader-local
    /// `ClassRepository`), loader B would wrongly reuse it and report
    /// `NotFound` too, without ever consulting its own, different
    /// `class_path`.
    #[test]
    fn a_loading_sessions_negative_result_does_not_leak_into_a_second_loader_with_a_different_classpath()
     {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let mut loader_a = ClassLoader::with_definitions(
            InMemoryClassPath(HashMap::new()),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        loader_a
            .load_class(&BinaryName::from_internal("Extra"))
            .expect_err("loader A's own classpath has no Extra");
        let session = loader_a.into_session();

        let mut classes_b = HashMap::new();
        classes_b.insert(
            BinaryName::from_internal("Extra"),
            synthetic_class("Extra", None),
        );
        let mut loader_b = ClassLoader::with_definitions(
            InMemoryClassPath(classes_b),
            &mut store,
            definitions,
            session,
        );
        let extra = loader_b
            .load_class(&BinaryName::from_internal("Extra"))
            .expect("loader B's own classpath does have Extra, and must be consulted for it");
        drop(loader_b);

        assert_eq!(symbol_name(&store, extra), "Extra");
    }

    /// `into_session` used to transfer only `resolved`/`packages`,
    /// dropping the previous loader's `metadata`/`origins` maps along
    /// with the loader itself -- so a `SymbolId` a second loader picked up
    /// from the shared `resolved` map (without loading it itself) had no
    /// `ClassLoader::metadata`/`ClassLoader::origin` for it, even though
    /// that data really was recorded when the class was first loaded.
    /// Fixed by moving `metadata`/`origins` into `LoadingSession` itself
    /// (safe, unlike `ClassRepository`'s `Failed` entries, because they
    /// are keyed by the already-resolved `SymbolId`, not a
    /// classpath-dependent `BinaryName` -- see `LoadingSession`'s own doc
    /// comment).
    #[test]
    fn a_symbols_metadata_and_origin_survive_a_loading_session_hand_off() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);

        let mut classes_a = HashMap::new();
        classes_a.insert(
            BinaryName::from_internal("Foo"),
            synthetic_class("Foo", None),
        );
        let mut loader_a = ClassLoader::with_definitions(
            InMemoryClassPath(classes_a),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let foo = loader_a
            .load_class(&BinaryName::from_internal("Foo"))
            .expect("Foo should load from loader A's own classpath");
        assert!(loader_a.metadata(foo).is_some());
        assert!(loader_a.origin(foo).is_some());
        let session = loader_a.into_session();

        // Loader B's own classpath does not have `Foo` at all -- it can
        // only ever have reached `foo`'s `SymbolId` through the shared
        // session's `resolved` map, never its own `load_uncached`.
        let mut loader_b = ClassLoader::with_definitions(
            InMemoryClassPath(HashMap::new()),
            &mut store,
            definitions,
            session,
        );
        let foo_again = loader_b
            .load_class(&BinaryName::from_internal("Foo"))
            .expect("Foo should still resolve, via the shared session, not loader B's own empty classpath");
        assert_eq!(foo, foo_again);
        assert!(
            loader_b.metadata(foo).is_some(),
            "metadata recorded by loader A must still be visible through loader B"
        );
        assert!(
            loader_b.origin(foo).is_some(),
            "origin recorded by loader A must still be visible through loader B"
        );
        drop(loader_b);
    }

    /// `java/lang/Object` is the one `Definitions` builtin with a real,
    /// classpath-loadable backing class file -- loading it must resolve to
    /// the exact same `SymbolId` as `Definitions::object_class`, not a
    /// second, disconnected identity, or a `Type::TypeRef` built by
    /// `Definitions` (e.g. an implicit JVM superclass) and one built by
    /// actually loading `java/lang/Object` would name two different
    /// symbols despite both meaning the same class.
    #[test]
    fn loading_java_lang_object_reuses_the_definitions_object_class_identity() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let object_class = loader.definitions().object_class;

        let loaded_object = loader
            .load_class(&BinaryName::from_internal("java/lang/Object"))
            .expect("java/lang/Object should load");
        drop(loader);

        assert_eq!(loaded_object, object_class);
        assert_eq!(symbol_name(&store, loaded_object), "Object");
        let info = class_info(&store, loaded_object);
        assert!(info.parents.is_empty());
    }

    /// `load_uncached_class`/`load_uncached_tasty` used to receive only
    /// `resource.bytes()`, discarding `resource.origin()` -- so
    /// `SymbolOrigin::Classfile`/`Tasty`'s opaque ID had no way back to
    /// which directory/JAR/JMOD actually supplied a class's bytes. Fixed by
    /// threading `ClassOrigin` through into the loader's own `origins`
    /// table, queryable via `ClassLoader::origin`.
    #[test]
    fn loading_a_class_records_its_class_path_origin() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let object = loader
            .load_class(&BinaryName::from_internal("java/lang/Object"))
            .expect("java/lang/Object should load");

        assert_eq!(
            loader.origin(object),
            Some(&ClassOrigin::Directory(PathBuf::from("<memory>")))
        );
    }

    /// A `Definitions` builtin (`Object`/`Any`/`Nothing`/a primitive) has
    /// no backing classpath resource -- `origin` must not report a
    /// misleading one for it.
    #[test]
    fn a_definitions_builtin_has_no_recorded_origin() {
        let mut store = SemanticStore::new();
        let loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);

        assert_eq!(loader.origin(loader.definitions().object_class), None);
    }

    #[test]
    fn loads_a_real_fixture_with_a_super_class_and_an_interface() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");
        drop(loader);

        assert_eq!(symbol_name(&store, symbol), "PoolSample");
        let info = class_info(&store, symbol);
        assert_eq!(info.parents.len(), 2);
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[0])),
            "Object"
        );
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[1])),
            "Runnable"
        );
    }

    /// `pool_sample/PoolSample.class` declares 5 real `public static
    /// final` fields (confirmed via `javap -p -v`); this asserts every
    /// one decodes with the right name, type, and flags.
    #[test]
    fn loads_a_real_fixtures_fields() {
        use dotty_classfile::descriptor::FieldType;

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("PoolSample should have classfile metadata");
        let all_fields = &metadata.fields;
        let field = |name: &str| {
            all_fields
                .iter()
                .find(|field| field.name() == name)
                .unwrap_or_else(|| panic!("field {name} should exist"))
        };

        assert_eq!(all_fields.len(), 5);
        assert_eq!(field("ANSWER").field_type(), &FieldType::Int);
        assert_eq!(field("BIG_ANSWER").field_type(), &FieldType::Long);
        assert_eq!(field("HALF").field_type(), &FieldType::Float);
        assert_eq!(field("PI").field_type(), &FieldType::Double);
        assert_eq!(
            field("GREETING").field_type(),
            &FieldType::Object("java/lang/String".to_owned())
        );
        for name in ["ANSWER", "BIG_ANSWER", "HALF", "PI", "GREETING"] {
            assert!(field(name).flags().is_static(), "{name} should be static");
            assert!(field(name).flags().is_final(), "{name} should be final");
        }
    }

    /// The same 5 `public static final` fields as
    /// [`loads_a_real_fixtures_fields`], now checked through the
    /// canonical path: real `dotty-core` `Symbol`s entered into
    /// `PoolSample`'s declarations scope, not the `ClassfileMetadata`
    /// sidecar.
    #[test]
    fn enters_real_fixtures_fields_into_the_class_declarations_scope() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");
        drop(loader);

        let declarations = class_info(&store, symbol).declarations;
        let mut field_symbol = |field_name: &str| {
            let text = store.names.intern(field_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{field_name} should be entered in the class scope"))
        };

        for field_name in ["ANSWER", "BIG_ANSWER", "HALF", "PI", "GREETING"] {
            let field = store.symbols.get(field_symbol(field_name));
            assert_eq!(
                field.kind,
                SymbolKind::Field,
                "{field_name} should be a Field"
            );
            assert_eq!(
                field.visibility,
                Visibility::Public,
                "{field_name} should be public"
            );
            assert!(
                field.flags.contains(SymbolFlags::STATIC),
                "{field_name} should be static"
            );
            assert!(
                field.flags.contains(SymbolFlags::FINAL),
                "{field_name} should be final"
            );
            assert!(
                !field.flags.contains(SymbolFlags::MUTABLE),
                "{field_name} is final, so should not be MUTABLE"
            );
        }

        let answer_type = member_type_id(&mut store, symbol, "ANSWER");
        let int_symbol = parent_symbol(&store, answer_type);
        assert_eq!(symbol_name(&store, int_symbol), "Int");

        let greeting_type = member_type_id(&mut store, symbol, "GREETING");
        let string_symbol = parent_symbol(&store, greeting_type);
        assert_eq!(symbol_name(&store, string_symbol), "String");
    }

    /// `pool_sample/PoolSample.class` declares 3 real methods (confirmed
    /// via `javap -p -v`): the implicit `<init>()V` constructor,
    /// `run()V` (overriding `Runnable.run`), and `computeAnswer()I`
    /// (`private static`) — this asserts every one decodes with the
    /// right name, descriptor, and flags.
    #[test]
    fn loads_a_real_fixtures_methods() {
        use dotty_classfile::descriptor::{FieldType, MethodDescriptor};

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("PoolSample should have classfile metadata");
        let all_methods = &metadata.methods;
        let method = |name: &str| {
            all_methods
                .iter()
                .find(|method| method.name() == name)
                .unwrap_or_else(|| panic!("method {name} should exist"))
        };

        assert_eq!(all_methods.len(), 3);
        assert_eq!(
            method("<init>").descriptor(),
            &MethodDescriptor {
                parameters: vec![],
                return_type: None,
            }
        );
        assert_eq!(
            method("run").descriptor(),
            &MethodDescriptor {
                parameters: vec![],
                return_type: None,
            }
        );
        assert_eq!(
            method("computeAnswer").descriptor(),
            &MethodDescriptor {
                parameters: vec![],
                return_type: Some(FieldType::Int),
            }
        );
        assert!(method("computeAnswer").flags().is_private());
        assert!(method("computeAnswer").flags().is_static());
    }

    /// `FieldSymbol::symbol`/`MethodSymbol::symbol` name the exact same
    /// `Symbol` a scope lookup by name would find — proving the sidecar
    /// record's backlink is real, not just present, by cross-checking it
    /// against the canonical path [`enters_real_fixtures_fields_into_the_class_declarations_scope`]/
    /// [`loads_a_real_fixtures_methods`] already exercise separately.
    #[test]
    fn field_and_method_sidecar_records_carry_their_real_symbol_id() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("PoolSample should have classfile metadata");
        let field_symbol = metadata
            .fields
            .iter()
            .find(|field| field.name() == "ANSWER")
            .expect("ANSWER should exist")
            .symbol();
        let method_symbol = metadata
            .methods
            .iter()
            .find(|method| method.name() == "run")
            .expect("run should exist")
            .symbol();
        drop(loader);

        let declarations = class_info(&store, symbol).declarations;
        let mut lookup = |member_name: &str| {
            let text = store.names.intern(member_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"))
        };

        assert_eq!(field_symbol, lookup("ANSWER"));
        assert_eq!(method_symbol, Some(lookup("run")));
    }

    /// The same 3 methods as [`loads_a_real_fixtures_methods`], now
    /// checked through the canonical path: real `dotty-core` `Symbol`s
    /// entered into `PoolSample`'s declarations scope. `<init>` becomes a
    /// `Constructor`; `run`/`computeAnswer` become plain `Method`s, with
    /// `computeAnswer`'s `private static ()I` shape fully round-tripping
    /// through `Type::Method`.
    #[test]
    fn enters_real_fixtures_methods_into_the_class_declarations_scope() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");
        drop(loader);

        let declarations = class_info(&store, symbol).declarations;
        let lookup = |store: &mut SemanticStore, method_name: &str| {
            let text = store.names.intern(method_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{method_name} should be entered in the class scope"))
        };

        let init_id = lookup(&mut store, "<init>");
        assert_eq!(store.symbols.get(init_id).kind, SymbolKind::Constructor);

        let run_id = lookup(&mut store, "run");
        let run = store.symbols.get(run_id);
        assert_eq!(run.kind, SymbolKind::Method);
        assert_eq!(run.visibility, Visibility::Public);

        let compute_answer_id = lookup(&mut store, "computeAnswer");
        let compute_answer = store.symbols.get(compute_answer_id);
        assert_eq!(compute_answer.kind, SymbolKind::Method);
        assert_eq!(compute_answer.visibility, Visibility::Private);
        assert!(compute_answer.flags.contains(SymbolFlags::STATIC));

        let compute_answer_type = member_type_id(&mut store, symbol, "computeAnswer");
        let Type::Method(descriptor) = store.types.get(compute_answer_type) else {
            panic!("expected computeAnswer to be a Type::Method");
        };
        assert!(descriptor.params.is_empty());
        let int_symbol = parent_symbol(&store, descriptor.result);
        assert_eq!(symbol_name(&store, int_symbol), "Int");
    }

    /// A hand-built, minimal, synthetic class file with two methods:
    /// `<clinit>()V` (a static initializer) and `normal()V`. `javac`
    /// always emits `<clinit>` alongside real static field initializers,
    /// but building it by hand keeps this test independent of exactly
    /// which fixture happens to need one.
    fn synthetic_class_with_a_static_initializer() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x06]); // constant_pool_count = 6
        bytes.push(1); // #1 Utf8 "C"
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "<clinit>"
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(b"<clinit>");
        bytes.push(1); // #4 Utf8 "()V"
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"()V");
        bytes.push(1); // #5 Utf8 "normal"
        bytes.extend_from_slice(&6u16.to_be_bytes());
        bytes.extend_from_slice(b"normal");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count = 0
        bytes.extend_from_slice(&[0x00, 0x02]); // methods_count = 2
        bytes.extend_from_slice(&[0x00, 0x08]); // <clinit> access_flags = ACC_STATIC
        bytes.extend_from_slice(&3u16.to_be_bytes()); // name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // normal access_flags = ACC_PUBLIC
        bytes.extend_from_slice(&5u16.to_be_bytes()); // name_index = #5
        bytes.extend_from_slice(&4u16.to_be_bytes()); // descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn clinit_is_not_entered_into_the_class_declarations_scope() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_a_static_initializer(),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");

        let metadata = loader
            .metadata(symbol)
            .expect("C should have classfile metadata");
        assert_eq!(
            metadata.methods.len(),
            2,
            "<clinit> and normal should both be in the sidecar"
        );
        drop(loader);

        let declarations = class_info(&store, symbol).declarations;
        let clinit_text = store.names.intern("<clinit>");
        let clinit_name = Name::new(clinit_text, Namespace::Term);
        assert_eq!(
            store.scopes.get(declarations).lookup(&clinit_name),
            None,
            "<clinit> should not be entered as a Symbol"
        );

        let normal_text = store.names.intern("normal");
        let normal_name = Name::new(normal_text, Namespace::Term);
        let normal_symbol = store
            .scopes
            .get(declarations)
            .lookup(&normal_name)
            .expect("normal should be entered as a Symbol");
        assert_eq!(store.symbols.get(normal_symbol).kind, SymbolKind::Method);
    }

    /// Reads a fixture owned by this crate (as opposed to `fixture_bytes`,
    /// which reads `dotty-classfile`'s fixtures).
    fn own_fixture_bytes(relative_path: &str) -> Vec<u8> {
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(relative_path),
        )
        .expect("fixture should exist")
    }

    /// `generic_sample/GenericSample.class` has no user-declared
    /// superclass (implicitly `java/lang/Object`) and no interfaces;
    /// since Milestone 6, its `items` field's erased type
    /// (`java/util/List`) and its `first()` method's erased return type
    /// (`java/lang/Comparable` — `T`'s bound, since descriptors erase
    /// generics to the first bound) also force resolution, so synthetic
    /// stand-ins are needed for both, alongside the real fixture bytes.
    fn generic_sample_classpath() -> HashMap<BinaryName, Vec<u8>> {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("GenericSample"),
            own_fixture_bytes("generic_sample/GenericSample.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/util/List"),
            synthetic_class("java/util/List", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Comparable"),
            synthetic_class("java/lang/Comparable", None),
        );
        classes
    }

    /// `generic_sample/GenericSample.class`'s `items` field has a real
    /// `Signature` attribute (`Ljava/util/List<TT;>;`, confirmed via
    /// `javap -p -v`) since its declared type is generic.
    #[test]
    fn loads_a_real_fixtures_field_signature() {
        use dotty_classfile::signature::{
            ClassTypeSignature, FieldSignature, ReferenceTypeSignature, TypeArgument,
        };

        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("GenericSample should have classfile metadata");
        let all_fields = &metadata.fields;
        let items = all_fields
            .iter()
            .find(|field| field.name() == "items")
            .expect("items field should exist");

        assert_eq!(
            items.signature(),
            Some(&FieldSignature(ReferenceTypeSignature::Class(
                ClassTypeSignature {
                    package: vec!["java".to_owned(), "util".to_owned()],
                    simple_name: "List".to_owned(),
                    type_arguments: vec![TypeArgument::Exact(
                        ReferenceTypeSignature::TypeVariable("T".to_owned())
                    )],
                    suffix: vec![],
                }
            )))
        );
    }

    /// `generic_sample/GenericSample.class`'s `first` method has a real
    /// `Signature` attribute (`()TT;`, confirmed via `javap -p -v`)
    /// since its return type is a bare type variable.
    #[test]
    fn loads_a_real_fixtures_method_signature() {
        use dotty_classfile::signature::{MethodSignature, ReferenceTypeSignature, TypeSignature};

        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("GenericSample should have classfile metadata");
        let all_methods = &metadata.methods;
        let first = all_methods
            .iter()
            .find(|method| method.name() == "first")
            .expect("first method should exist");

        assert_eq!(
            first.signature(),
            Some(&MethodSignature {
                type_parameters: vec![],
                parameters: vec![],
                result: Some(TypeSignature::Reference(
                    ReferenceTypeSignature::TypeVariable("T".to_owned())
                )),
                throws: vec![],
            })
        );
    }

    /// `generic_sample/GenericSample.class` has a real class-level
    /// `Signature` attribute (`<T::Ljava/lang/Comparable<TT;>;>Ljava/lang/Object;`,
    /// confirmed via `javap -p -v`) since its declaration is generic.
    #[test]
    fn loads_a_real_fixtures_class_signature() {
        use dotty_classfile::signature::{
            ClassSignature, ClassTypeSignature, ReferenceTypeSignature, TypeArgument, TypeParameter,
        };

        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("GenericSample should have classfile metadata");
        assert_eq!(
            metadata.signature.clone(),
            Some(ClassSignature {
                type_parameters: vec![TypeParameter {
                    name: "T".to_owned(),
                    class_bound: None,
                    interface_bounds: vec![ReferenceTypeSignature::Class(ClassTypeSignature {
                        package: vec!["java".to_owned(), "lang".to_owned()],
                        simple_name: "Comparable".to_owned(),
                        type_arguments: vec![TypeArgument::Exact(
                            ReferenceTypeSignature::TypeVariable("T".to_owned())
                        )],
                        suffix: vec![],
                    })],
                }],
                superclass: ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "Object".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                },
                superinterfaces: vec![],
            })
        );
    }

    /// The `SymbolId` of the `SymbolKind::TypeParameter` symbol entered
    /// under `type_param_name` into `class_symbol`'s declarations scope by
    /// `ClassLoader::enter_class_type_parameters`.
    fn type_parameter_symbol(
        store: &mut SemanticStore,
        class_symbol: SymbolId,
        type_param_name: &str,
    ) -> SymbolId {
        let declarations = class_info(store, class_symbol).declarations;
        let text = store.names.intern(type_param_name);
        let name = Name::new(text, Namespace::Type);
        store
            .scopes
            .get(declarations)
            .lookup(&name)
            .unwrap_or_else(|| panic!("{type_param_name} should be entered in the class scope"))
    }

    /// `GenericSample<T extends Comparable<T>>`'s `T` becomes a real
    /// `SymbolKind::TypeParameter` symbol in its declarations scope, whose
    /// upper bound is `Comparable[T]` — a self (F-bounded) reference back
    /// to the very symbol being defined, resolved correctly regardless of
    /// entry order (`enter_class_type_parameters`'s enter-before-complete
    /// staging).
    #[test]
    fn enters_a_real_fixtures_f_bounded_class_type_parameter() {
        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");
        drop(loader);

        let type_param = type_parameter_symbol(&mut store, class_symbol, "T");
        assert_eq!(
            store.symbols.get(type_param).kind,
            SymbolKind::TypeParameter
        );

        let SymbolInfo::Complete(bounds_id) = store.symbols.get(type_param).info else {
            panic!("expected the type parameter to have complete info");
        };
        let Type::Bounds { low, high } = store.types.get(bounds_id) else {
            panic!("expected a Bounds type");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *low)), "Nothing");

        // `class_bound: None` means an implicit `Object` bound (JVMS
        // §4.7.9.1), *in addition to* the one interface bound -- so the
        // upper bound is the intersection `Object & Comparable[T]`, not a
        // bare `Comparable[T]`.
        let Type::And { left, right } = store.types.get(*high) else {
            panic!("expected T's upper bound to be an intersection with the implicit Object bound");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *left)), "Object");

        let Type::Applied { tycon, args } = store.types.get(*right) else {
            panic!("expected T's interface bound to be an Applied Comparable[T]");
        };
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, *tycon)),
            "Comparable"
        );
        assert_eq!(args.len(), 1);
        assert_eq!(parent_symbol(&store, args[0]), type_param);
    }

    /// `GenericSample.items`'s declared type `List<T>` lowers to a real
    /// `Type::Applied` referencing the class's own `T` type parameter
    /// symbol, not the erased `List` a descriptor-only lowering would give.
    #[test]
    fn lowers_a_real_fixtures_generic_field_type_using_the_class_type_parameter() {
        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");
        drop(loader);

        let type_param = type_parameter_symbol(&mut store, class_symbol, "T");
        let items_type = member_type_id(&mut store, class_symbol, "items");
        let Type::Applied { tycon, args } = store.types.get(items_type) else {
            panic!("expected items's type to be an Applied List[T]");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *tycon)), "List");
        assert_eq!(args.len(), 1);
        assert_eq!(parent_symbol(&store, args[0]), type_param);
    }

    /// `GenericSample.first()`'s declared return type is a bare `T` — no
    /// method-level type parameters of its own, just a reference to the
    /// enclosing class's `T` — so it lowers to a real reference to that
    /// exact symbol instead of the erased descriptor return type
    /// (`Comparable`, `T`'s own bound).
    #[test]
    fn lowers_a_real_fixtures_generic_method_result_using_the_class_type_parameter() {
        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(InMemoryClassPath(generic_sample_classpath()), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");
        drop(loader);

        let type_param = type_parameter_symbol(&mut store, class_symbol, "T");
        let first_type = member_type_id(&mut store, class_symbol, "first");
        let Type::Method(method) = store.types.get(first_type) else {
            panic!("expected first's type to be a Type::Method");
        };
        assert!(method.params.is_empty());
        assert_eq!(parent_symbol(&store, method.result), type_param);
    }

    /// A hand-built, minimal, synthetic class file with one field whose
    /// descriptor is not a valid field descriptor. `javac` cannot produce
    /// this; it exists purely to exercise `MalformedDescriptor`.
    fn synthetic_class_with_malformed_field_descriptor() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x05]); // constant_pool_count = 5
        bytes.push(1); // #1 Utf8
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "field"
        bytes.extend_from_slice(&5u16.to_be_bytes());
        bytes.extend_from_slice(b"field");
        bytes.push(1); // #4 Utf8 "X" (not a valid field descriptor)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"X");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&3u16.to_be_bytes()); // field name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // field descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x00]); // field attributes_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn malformed_descriptor_when_a_field_descriptor_does_not_parse() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_malformed_field_descriptor(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedDescriptor(name, _) if name.as_internal() == "C"
        ));
    }

    /// A hand-built, minimal, synthetic class file with one field whose
    /// descriptor is valid but whose `Signature` attribute is not a
    /// valid generic field signature. `javac` cannot produce this; it
    /// exists purely to exercise `MalformedSignature` for a field.
    fn synthetic_class_with_malformed_field_signature() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x07]); // constant_pool_count = 7
        bytes.push(1); // #1 Utf8 "C"
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "field"
        bytes.extend_from_slice(&5u16.to_be_bytes());
        bytes.extend_from_slice(b"field");
        bytes.push(1); // #4 Utf8 "I" (valid field descriptor)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"I");
        bytes.push(1); // #5 Utf8 "Signature"
        bytes.extend_from_slice(&9u16.to_be_bytes());
        bytes.extend_from_slice(b"Signature");
        bytes.push(1); // #6 Utf8 "@" (not a valid field signature)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"@");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&3u16.to_be_bytes()); // field name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // field descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x01]); // field attributes_count = 1
        bytes.extend_from_slice(&5u16.to_be_bytes()); // attribute_name_index = #5
        bytes.extend_from_slice(&2u32.to_be_bytes()); // attribute_length = 2
        bytes.extend_from_slice(&6u16.to_be_bytes()); // signature_index = #6
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn malformed_signature_when_a_field_signature_does_not_parse() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_malformed_field_signature(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedSignature(name, _) if name.as_internal() == "C"
        ));
    }

    /// A hand-built, minimal, synthetic class file with one field whose
    /// `Signature` attribute is `Ljava/util/List<+Ljava/lang/Number;>;` —
    /// an `extends`-wildcard type argument (`List<? extends Number>`).
    /// `javac` can produce this shape (any wildcard-typed field would do),
    /// but building it by hand keeps this test independent of a new
    /// fixture and lets the constant pool stay minimal.
    fn synthetic_class_with_wildcard_field_signature() -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;
        let this_name_index = push_utf8(&mut pool, &mut next_index, "C");
        let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
        let field_name_index = push_utf8(&mut pool, &mut next_index, "field");
        let descriptor_index = push_utf8(&mut pool, &mut next_index, "Ljava/util/List;");
        let signature_attr_name_index = push_utf8(&mut pool, &mut next_index, "Signature");
        let signature_index = push_utf8(
            &mut pool,
            &mut next_index,
            "Ljava/util/List<+Ljava/lang/Number;>;",
        );

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        bytes.extend_from_slice(&[0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0x45]);
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&field_name_index.to_be_bytes());
        bytes.extend_from_slice(&descriptor_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]); // field attributes_count = 1
        bytes.extend_from_slice(&signature_attr_name_index.to_be_bytes());
        bytes.extend_from_slice(&2u32.to_be_bytes()); // attribute_length = 2
        bytes.extend_from_slice(&signature_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn field_with_an_extends_wildcard_type_argument_lowers_to_a_bounded_wildcard() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_wildcard_field_signature(),
        );
        classes.insert(
            BinaryName::from_internal("java/util/List"),
            synthetic_class("java/util/List", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Number"),
            synthetic_class("java/lang/Number", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");
        drop(loader);
        let field_type = member_type_id(&mut store, class_symbol, "field");

        let Type::Applied { tycon, args } = store.types.get(field_type) else {
            panic!("expected field's type to be an Applied List[_]");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *tycon)), "List");
        assert_eq!(args.len(), 1);

        let Type::Wildcard { bounds } = store.types.get(args[0]) else {
            panic!("expected the type argument to be a Wildcard");
        };
        let Type::Bounds { low, high } = store.types.get(*bounds) else {
            panic!("expected the wildcard to carry Bounds");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *low)), "Nothing");
        assert_eq!(symbol_name(&store, parent_symbol(&store, *high)), "Number");
    }

    /// A hand-built, minimal, synthetic class file with one field whose
    /// `Signature` attribute references a `TypeVariable` (`TU;`) that no
    /// class-level type parameter declares — exercises
    /// `ClassLoadError::UnresolvedTypeVariable`.
    fn synthetic_class_with_unresolved_type_variable_field_signature() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x07]); // constant_pool_count = 7
        bytes.push(1); // #1 Utf8 "C"
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "field"
        bytes.extend_from_slice(&5u16.to_be_bytes());
        bytes.extend_from_slice(b"field");
        bytes.push(1); // #4 Utf8 "Ljava/lang/Object;" (valid field descriptor)
        bytes.extend_from_slice(&18u16.to_be_bytes());
        bytes.extend_from_slice(b"Ljava/lang/Object;");
        bytes.push(1); // #5 Utf8 "Signature"
        bytes.extend_from_slice(&9u16.to_be_bytes());
        bytes.extend_from_slice(b"Signature");
        bytes.push(1); // #6 Utf8 "TU;" (an unresolvable type variable)
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"TU;");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&3u16.to_be_bytes()); // field name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // field descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x01]); // field attributes_count = 1
        bytes.extend_from_slice(&5u16.to_be_bytes()); // attribute_name_index = #5
        bytes.extend_from_slice(&2u32.to_be_bytes()); // attribute_length = 2
        bytes.extend_from_slice(&6u16.to_be_bytes()); // signature_index = #6
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn unresolved_type_variable_when_a_field_signature_references_an_unknown_type_parameter() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_unresolved_type_variable_field_signature(),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::UnresolvedTypeVariable(name, variable)
                if name.as_internal() == "C" && variable == "U"
        ));
    }

    /// A hand-built, minimal, synthetic class file with one method whose
    /// descriptor is not a valid method descriptor. `javac` cannot
    /// produce this; it exists purely to exercise `MalformedDescriptor`
    /// for a method rather than a field.
    fn synthetic_class_with_malformed_method_descriptor() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x05]); // constant_pool_count = 5
        bytes.push(1); // #1 Utf8
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "method"
        bytes.extend_from_slice(&6u16.to_be_bytes());
        bytes.extend_from_slice(b"method");
        bytes.push(1); // #4 Utf8 "X" (not a valid method descriptor)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"X");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // methods_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // method access_flags
        bytes.extend_from_slice(&3u16.to_be_bytes()); // method name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // method descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x00]); // method attributes_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn malformed_descriptor_when_a_method_descriptor_does_not_parse() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_malformed_method_descriptor(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedDescriptor(name, _) if name.as_internal() == "C"
        ));
    }

    /// A hand-built, minimal, synthetic class file with one method whose
    /// descriptor is valid but whose `Signature` attribute is not a
    /// valid generic method signature. `javac` cannot produce this; it
    /// exists purely to exercise `MalformedSignature` for a method.
    fn synthetic_class_with_malformed_method_signature() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x07]); // constant_pool_count = 7
        bytes.push(1); // #1 Utf8 "C"
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "method"
        bytes.extend_from_slice(&6u16.to_be_bytes());
        bytes.extend_from_slice(b"method");
        bytes.push(1); // #4 Utf8 "()V" (valid method descriptor)
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"()V");
        bytes.push(1); // #5 Utf8 "Signature"
        bytes.extend_from_slice(&9u16.to_be_bytes());
        bytes.extend_from_slice(b"Signature");
        bytes.push(1); // #6 Utf8 "@" (not a valid method signature)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"@");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // methods_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // method access_flags
        bytes.extend_from_slice(&3u16.to_be_bytes()); // method name_index = #3
        bytes.extend_from_slice(&4u16.to_be_bytes()); // method descriptor_index = #4
        bytes.extend_from_slice(&[0x00, 0x01]); // method attributes_count = 1
        bytes.extend_from_slice(&5u16.to_be_bytes()); // attribute_name_index = #5
        bytes.extend_from_slice(&2u32.to_be_bytes()); // attribute_length = 2
        bytes.extend_from_slice(&6u16.to_be_bytes()); // signature_index = #6
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0
        bytes
    }

    #[test]
    fn malformed_signature_when_a_method_signature_does_not_parse() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_malformed_method_signature(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedSignature(name, _) if name.as_internal() == "C"
        ));
    }

    /// A hand-built, minimal, synthetic class file whose class-level
    /// `Signature` attribute is not a valid generic class signature.
    /// `javac` cannot produce this; it exists purely to exercise
    /// `MalformedSignature` for a class rather than a member.
    fn synthetic_class_with_malformed_class_signature() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&[0x00, 0x05]); // constant_pool_count = 5
        bytes.push(1); // #1 Utf8 "C"
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"C");
        bytes.push(7); // #2 Class -> #1
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.push(1); // #3 Utf8 "Signature"
        bytes.extend_from_slice(&9u16.to_be_bytes());
        bytes.extend_from_slice(b"Signature");
        bytes.push(1); // #4 Utf8 "@" (not a valid class signature)
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(b"@");
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class = #2
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count = 0
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // attributes_count = 1
        bytes.extend_from_slice(&3u16.to_be_bytes()); // attribute_name_index = #3
        bytes.extend_from_slice(&2u32.to_be_bytes()); // attribute_length = 2
        bytes.extend_from_slice(&4u16.to_be_bytes()); // signature_index = #4
        bytes
    }

    #[test]
    fn malformed_signature_when_a_class_signature_does_not_parse() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_malformed_class_signature(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedSignature(name, _) if name.as_internal() == "C"
        ));
    }

    /// Confirms Milestone 1's `ClassLoader` and Milestone 2's
    /// `JarClassPath` compose: the same PoolSample fixture, loaded from
    /// inside a real JAR (via a `CompositeClassPath` that falls through
    /// to the same synthetic `java/lang/Object`/`java/lang/Runnable`
    /// classes as the directory-based test above) instead of from a
    /// bare in-memory map. Run against both the STORED and the real
    /// DEFLATE-compressed fixture, since `JarClassPath` must work
    /// identically either way.
    fn assert_loads_pool_sample_from_jar(jar_file_name: &str) {
        use crate::class_path::CompositeClassPath;
        use crate::jar_class_path::JarClassPath;

        let jar_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pool_sample_jar")
            .join(jar_file_name);
        let jar_class_path = JarClassPath::new(jar_path, 21).unwrap();

        let mut synthetic_classes = HashMap::new();
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/Runnable"),
            synthetic_class("java/lang/Runnable", None),
        );
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );

        let composite = CompositeClassPath::new(vec![
            Box::new(jar_class_path),
            Box::new(InMemoryClassPath(synthetic_classes)),
        ]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(composite, &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load from the JAR");
        drop(loader);

        assert_eq!(symbol_name(&store, symbol), "PoolSample");
        let info = class_info(&store, symbol);
        assert_eq!(info.parents.len(), 2);
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[0])),
            "Object"
        );
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[1])),
            "Runnable"
        );
    }

    #[test]
    fn loads_pool_sample_from_a_stored_jar_via_composite_class_path() {
        assert_loads_pool_sample_from_jar("pool_sample_stored.jar");
    }

    #[test]
    fn loads_pool_sample_from_a_real_deflate_compressed_jar_via_composite_class_path() {
        assert_loads_pool_sample_from_jar("pool_sample_deflate.jar");
    }

    /// Confirms Milestone 8's `JarClassPath` release-version selection
    /// reaches all the way through to a decoded class's metadata: the real
    /// multi-release fixture (`tests/fixtures/multi_release_jar/`) has a
    /// distinct `@Deprecated(since = ...)` annotation on its base entry
    /// (none) and its `versions/11`/`versions/17` overrides, so which
    /// variant `ClassLoader` actually decoded is visible in the loaded
    /// symbol's `ClassfileMetadata::annotations` without needing any new
    /// symbol-level concept just for this test.
    fn assert_loads_mr_sample_annotation_for_release(
        release_version: u16,
        expected_since: Option<&str>,
    ) {
        use crate::annotation::AnnotationValue;
        use crate::class_path::CompositeClassPath;
        use crate::jar_class_path::JarClassPath;

        let jar_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi_release_jar/mr_sample.jar");
        let jar_class_path = JarClassPath::new(jar_path, release_version).unwrap();

        let mut synthetic_classes = HashMap::new();
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let composite = CompositeClassPath::new(vec![
            Box::new(jar_class_path),
            Box::new(InMemoryClassPath(synthetic_classes)),
        ]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(composite, &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("MrSample"))
            .expect("MrSample should load from the multi-release JAR");

        let metadata = loader
            .metadata(symbol)
            .expect("MrSample should have classfile metadata");
        let since = metadata.annotations.iter().find_map(|annotation| {
            annotation
                .elements
                .iter()
                .find(|(name, _)| name == "since")
                .map(|(_, value)| match value {
                    AnnotationValue::String(value) => value.clone(),
                    other => panic!("unexpected element value: {other:?}"),
                })
        });

        assert_eq!(since.as_deref(), expected_since);
    }

    #[test]
    fn loads_the_base_mr_sample_variant_for_a_release_below_every_versioned_directory() {
        assert_loads_mr_sample_annotation_for_release(9, None);
    }

    #[test]
    fn loads_the_v11_mr_sample_variant_for_a_release_between_11_and_16() {
        assert_loads_mr_sample_annotation_for_release(11, Some("11"));
        assert_loads_mr_sample_annotation_for_release(16, Some("11"));
    }

    #[test]
    fn loads_the_v17_mr_sample_variant_for_a_release_at_or_above_17() {
        assert_loads_mr_sample_annotation_for_release(17, Some("17"));
        assert_loads_mr_sample_annotation_for_release(25, Some("17"));
    }

    /// Confirms Milestone 1's `ClassLoader` and Milestone 3's
    /// `JdkClassPath` compose: a real class, packed inside a real JMOD
    /// (see `tests/fixtures/jdk_classpath/`), loaded from a
    /// `$JAVA_HOME/jmods`-shaped directory containing more than one
    /// `.jmod` file, falling through to the same synthetic
    /// `java/lang/Object`/`java/lang/Runnable` classes used by the
    /// directory- and JAR-based tests above.
    #[test]
    fn loads_pool_sample_from_a_jdk_class_path_with_multiple_jmods() {
        use crate::class_path::CompositeClassPath;
        use crate::jdk_class_path::JdkClassPath;

        let jmods_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_classpath");
        let jdk_class_path = JdkClassPath::new(jmods_dir).unwrap();

        let mut synthetic_classes = HashMap::new();
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/Runnable"),
            synthetic_class("java/lang/Runnable", None),
        );
        synthetic_classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );

        let composite = CompositeClassPath::new(vec![
            Box::new(jdk_class_path),
            Box::new(InMemoryClassPath(synthetic_classes)),
        ]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(composite, &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("pool/PoolSample"))
            .expect("pool/PoolSample should load from the JDK class path");
        drop(loader);

        assert_eq!(symbol_name(&store, symbol), "PoolSample");
        let info = class_info(&store, symbol);
        assert_eq!(info.parents.len(), 2);
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[0])),
            "Object"
        );
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, info.parents[1])),
            "Runnable"
        );
    }

    #[test]
    fn returns_the_same_cached_symbol_on_a_second_load() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()), &mut store);
        let name = BinaryName::from_internal("PoolSample");

        let first = loader.load_class(&name).unwrap();
        let second = loader.load_class(&name).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn not_found_when_the_classpath_has_nothing_for_the_name() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);

        let error = loader
            .load_class(&BinaryName::from_internal("Missing"))
            .unwrap_err();

        assert!(matches!(error, ClassLoadError::NotFound(name) if name.as_internal() == "Missing"));
    }

    #[test]
    fn io_error_from_the_classpath_is_wrapped_and_not_cached_as_not_found() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(AlwaysErrors, &mut store);

        let error = loader
            .load_class(&BinaryName::from_internal("Anything"))
            .unwrap_err();

        assert!(matches!(error, ClassLoadError::Io(name, _) if name.as_internal() == "Anything"));
    }

    #[test]
    fn invalid_class_file_when_the_bytes_do_not_decode() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::InvalidClassFile(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn unsupported_class_version_when_major_version_exceeds_the_compatibility_ceiling() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("FromTheFuture"),
            synthetic_future_version_class("FromTheFuture"),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("FromTheFuture"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::UnsupportedClassVersion(name, version)
                if name.as_internal() == "FromTheFuture" && version.major == 70
        ));
    }

    #[test]
    fn malformed_reference_when_this_class_does_not_resolve() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Broken"),
            synthetic_malformed_this_class(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedReference(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn name_mismatch_when_this_class_names_a_different_class() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("RequestedUnderTheWrongName"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("RequestedUnderTheWrongName"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::NameMismatch { requested, actual }
                if requested.as_internal() == "RequestedUnderTheWrongName"
                    && actual.as_internal() == "PoolSample"
        ));
    }

    #[test]
    fn dependency_failure_when_the_super_class_cannot_be_found() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("PoolSample"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );
        // Deliberately omit java/lang/Object and java/lang/Runnable so
        // resolving PoolSample's superclass fails.

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::DependencyFailure { owner, dependency, source }
                if owner.as_internal() == "PoolSample"
                    && dependency.as_internal() == "java/lang/Object"
                    && matches!(*source, ClassLoadError::NotFound(_))
        ));
    }

    #[test]
    fn invalid_tasty_file_when_the_bytes_do_not_decode() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryTastyClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::InvalidTastyFile(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn dependency_failure_when_a_tasty_classes_super_class_cannot_be_found() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Dog"),
            own_fixture_bytes("tasty_sample/Dog.tasty"),
        );
        // Deliberately omit java/lang/Object (and Animal) so resolving
        // Dog's implicit superclass fails.

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryTastyClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Dog"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::DependencyFailure { owner, dependency, source }
                if owner.as_internal() == "Dog"
                    && dependency.as_internal() == "java/lang/Object"
                    && matches!(*source, ClassLoadError::NotFound(_))
        ));
    }

    /// `case_class/Point.tasty`'s real `case class Point(x: Int, y: Int)`
    /// (`dotty-tasty`'s own fixture, reused here rather than duplicated —
    /// `tasty_loading.rs`'s `Dog`/`Animal` carry no fields/methods at
    /// all) exercises `.tasty` field/method reconstruction end to end:
    /// `x`/`y` are constructor `CASEACCESSOR_TAG` parameters (not
    /// separate `ValDef`s — see
    /// `tasty_symbol::decode_constructor_accessor_fields`'s doc comment),
    /// while `<init>`/`copy`/`hashCode`/... come from ordinary `DefDef`
    /// template stats.
    #[test]
    fn enters_a_real_tasty_case_classs_fields_and_methods_into_the_class_declarations_scope() {
        let mut point = HashMap::new();
        point.insert(
            BinaryName::from_internal("Point"),
            dotty_tasty_fixture_bytes("case_class/Point.tasty"),
        );

        let mut dependencies = HashMap::new();
        dependencies.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Product"),
            synthetic_class("scala/Product", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Serializable"),
            synthetic_class("scala/Serializable", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Int"),
            synthetic_class("scala/Int", None),
        );

        // `Point` is `.tasty`-backed but its dependencies are plain
        // `.class` bytes here -- `InMemoryTastyClassPath` tags every
        // entry it holds the same format, so the two need separate
        // classpath entries composed together, the way `.tasty` and
        // `.class` files really do sit side by side on a JVM classpath.
        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(point)),
            Box::new(InMemoryClassPath(dependencies)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let point = loader
            .load_class(&BinaryName::from_internal("Point"))
            .expect("Point should load from its real .tasty fixture");
        drop(loader);

        let declarations = class_info(&store, point).declarations;
        let mut lookup = |member_name: &str| {
            let text = store.names.intern(member_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"))
        };

        let x = lookup("x");
        assert_eq!(store.symbols.get(x).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(x).visibility, Visibility::Public);
        assert!(store.symbols.get(x).flags.contains(SymbolFlags::FINAL));

        let init = lookup("<init>");
        assert_eq!(store.symbols.get(init).kind, SymbolKind::Constructor);
        let copy = lookup("copy");
        assert_eq!(store.symbols.get(copy).kind, SymbolKind::Method);
        assert!(
            store
                .symbols
                .get(copy)
                .flags
                .contains(SymbolFlags::SYNTHETIC)
        );

        // `x`'s declared type is `Int`, encoded post-typecheck as a
        // `TYPEREF_TAG` node. `RawTree::name_refs` only picks up that
        // node's qualifier prefix ("scala", a plain UTF8 name-table
        // entry), never its own leaf name ("Int", which lives behind a
        // signature-shaped name-table entry `wire_name` doesn't render)
        // -- so `resolve_parent_name` reduces it to the bare, wrong name
        // "scala", not `None`. `lower_tasty_member_type` still tries to
        // load that guess, it just doesn't exist as a class on this
        // classpath (or anywhere), so this falls back to `Type::Error`
        // either way -- but the message below names exactly what was
        // guessed and why loading it failed, instead of the same opaque
        // string a genuinely un-nameable type tree would also produce
        // (see `ClassLoader::lower_tasty_member_type`'s doc comment).
        // Closing the underlying best-effort-reduction gap is a
        // documented follow-up, not silently claimed here.
        // `x`'s declared type is `Int`, encoded post-typecheck as a real
        // `TYPEREF_TAG` node with a `scala` `TERMREFpkg` prefix --
        // `resolve_reference_name` resolves this fully to `scala/Int`
        // (registered on this test's classpath below), so `x`'s type is
        // a genuine `Type::TypeRef` now, not a `Type::Error` guess.
        let x_type = member_type_id(&mut store, point, "x");
        let Type::TypeRef { symbol, .. } = store.types.get(x_type) else {
            panic!(
                "expected x's type to be a real Type::TypeRef, got {:?}",
                store.types.get(x_type)
            );
        };
        assert_eq!(symbol_name(&store, *symbol), "Int");

        let init_type = member_type_id(&mut store, point, "<init>");
        let Type::Method(init_method) = store.types.get(init_type) else {
            panic!("expected <init>'s type to be a Type::Method");
        };
        assert_eq!(init_method.params.len(), 2);
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, init_method.result)),
            "Unit"
        );
    }

    /// `lower_tasty_member_type`'s two fallback paths to `Type::Error`
    /// used to converge on the exact same generic message regardless of
    /// which one was actually hit; `enters_a_real_tasty_case_classs_...`
    /// above covers "a name was guessed but failed to load" end to end
    /// through a real fixture, so this covers the other path directly:
    /// `declared_type` genuinely `None` (`tasty_symbol::decode` itself
    /// could not reduce the type tree to any name at all).
    #[test]
    fn unresolved_tasty_member_type_names_the_reason_when_no_name_was_reduced_at_all() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);

        let owner = BinaryName::from_internal("Owner");
        let error_type = loader.lower_tasty_member_type(&owner, None);
        drop(loader);

        let Type::Error(error) = store.types.get(error_type) else {
            panic!("expected a Type::Error");
        };
        let message = store.names.resolve(error.message);
        assert!(message.contains("could not reduce the declared type tree to a name"));
    }

    /// `tasty_symbol::decodes_a_private_class_with_a_generic_mixin_naming_its_own_enclosing_class`
    /// already proves, at the decode level, that `tasty_symbol::decode`
    /// preserves `scala3-library/scala/math/Ordering.tasty`'s real
    /// private nested `Reverse`'s `PRIVATE_TAG` — this used to prove the
    /// *loader* actually uses that too, by loading `Reverse` for real
    /// (which also loads `Ordering` itself, `Reverse`'s declared
    /// interface). `Ordering` itself mixes in a synthetic `Serializable`
    /// — the same unresolvable implicit-import reference as `BigInt`'s
    /// own `Serializable`
    /// (`tasty_symbol::decoding_fails_when_one_of_several_mixins_is_an_unresolvable_implicit_import_reference`)
    /// — so loading `Ordering` (and therefore `Reverse`) through a real
    /// `ClassLoader` now fails with `UnresolvedSupertype` instead of
    /// silently widening or narrowing any visibility, per the same
    /// explicit-unresolved-over-misleading-guess reasoning. Asserts that
    /// failure explicitly; this crate has no other real `.tasty` fixture
    /// with a non-public top-level class and no unresolvable implicit-
    /// import mixin anywhere in its dependency chain, so the loader-level
    /// (as opposed to `tasty_symbol::decode`-level, still covered by the
    /// `tasty_symbol` test named above) proof that
    /// `load_uncached_tasty`'s visibility patch is actually wired up is
    /// lost along with this fixture's usability, until either a suitable
    /// fixture is found or real scope/import resolution (issue #7)
    /// closes the underlying gap.
    #[test]
    fn loading_a_private_tasty_class_fails_when_its_own_interface_has_an_unresolved_mixin() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("Reverse"),
            dotty_tasty_fixture_bytes("scala3-library/scala/math/Ordering.tasty"),
        );
        tasty.insert(
            BinaryName::from_internal("Ordering"),
            dotty_tasty_fixture_bytes("scala3-library/scala/math/Ordering.tasty"),
        );

        let mut classes = HashMap::new();
        for stub in [
            "java/lang/Object",
            "Comparator",
            "java/util/Comparator",
            "PartialOrdering",
            "scala/PartialOrdering",
            "scala/math/PartialOrdering",
            "Serializable",
            "scala/Serializable",
            "java/io/Serializable",
        ] {
            classes.insert(BinaryName::from_internal(stub), synthetic_class(stub, None));
        }

        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Reverse"))
            .expect_err("Reverse should fail to load, via Ordering's own unresolved mixin");
        drop(loader);

        assert!(matches!(error, ClassLoadError::DependencyFailure { .. }));
    }

    /// End-to-end regression for `resolve_reference_name`
    /// (`tasty_symbol.rs`): loads the real `scala/io/Source.tasty`
    /// fixture through a full `ClassLoader`. `Closeable`'s own reference
    /// carries a real `TERMREFpkg` prefix and would resolve to the real
    /// `java/io/Closeable` classpath `Symbol` (that same real-classpath-
    /// resolution proof, via `Dog`/`Animal`'s own real `TERMREFpkg`
    /// mixin, still stands end-to-end in `tasty_loading.rs`'s
    /// `loads_dog_with_animal_resolved_through_tasty_as_a_real_interface`
    /// — `Dog.tasty` has no implicit-import mixin) — but `Source` also
    /// mixes in `Iterator`, an unresolvable implicit-import reference
    /// (`tasty_symbol::tests::decoding_fails_when_a_generic_mixins_tycon_is_an_unresolvable_implicit_import_reference`
    /// documents this at the decode-only level), so loading `Source`
    /// itself now fails outright rather than silently entering a
    /// same-package-guessed `Iterator` dependency, per the same
    /// explicit-unresolved-over-misleading-guess reasoning.
    #[test]
    fn loading_source_tasty_fails_on_its_unresolvable_iterator_mixin() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("Source"),
            dotty_tasty_fixture_bytes("scala3-library/scala/io/Source.tasty"),
        );

        let mut classes = HashMap::new();
        for stub in ["java/lang/Object", "Iterator", "java/io/Closeable"] {
            classes.insert(BinaryName::from_internal(stub), synthetic_class(stub, None));
        }

        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Source"))
            .expect_err("Source should fail to load, via its own unresolved Iterator mixin");
        drop(loader);

        assert!(matches!(
            error,
            ClassLoadError::InvalidTastyFile(_, TastyDecodeError::UnresolvedSupertype)
        ));
    }

    #[test]
    fn circular_inheritance_is_detected_instead_of_recursing_forever() {
        // Synthetic: A's superclass is B, B's superclass is A. `javac`
        // rejects this at the source level, so it can only be produced
        // by hand-building the class files directly.
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("A"),
            synthetic_class("A", Some("B")),
        );
        classes.insert(
            BinaryName::from_internal("B"),
            synthetic_class("B", Some("A")),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("A"))
            .unwrap_err();

        fn contains_circular_inheritance(error: &ClassLoadError) -> bool {
            match error {
                ClassLoadError::CircularInheritance(_) => true,
                ClassLoadError::DependencyFailure { source, .. } => {
                    contains_circular_inheritance(source)
                }
                _ => false,
            }
        }

        assert!(contains_circular_inheritance(&error));
    }

    /// The circular-inheritance branch and the `DependencyFailure` branch
    /// in `ClassLoader::load_class` are two separate places that mark a
    /// failed load's already-allocated `Symbol` `SymbolInfo::Error` — this
    /// checks both sides of the cycle got it, not just the one the outer
    /// `load_class` call directly sees. `A`'s `Symbol` is marked by the
    /// `Loading` branch when the recursive `load_class("A")` call (from
    /// resolving `B`'s superclass) finds `A` already `Loading`; `B`'s is
    /// then marked by the `DependencyFailure` branch once that error
    /// propagates back out through `B`'s own still-`Loading` entry.
    #[test]
    fn circular_inheritance_marks_both_symbols_as_error() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("A"),
            synthetic_class("A", Some("B")),
        );
        classes.insert(
            BinaryName::from_internal("B"),
            synthetic_class("B", Some("A")),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        loader
            .load_class(&BinaryName::from_internal("A"))
            .unwrap_err();

        let a_symbol = match loader.repository.get(&BinaryName::from_internal("A")) {
            Some(ClassEntry::Failed(Some(symbol), _)) => *symbol,
            other => panic!("expected A to be Failed with a Symbol, got {other:?}"),
        };
        let b_symbol = match loader.repository.get(&BinaryName::from_internal("B")) {
            Some(ClassEntry::Failed(Some(symbol), _)) => *symbol,
            other => panic!("expected B to be Failed with a Symbol, got {other:?}"),
        };
        drop(loader);

        assert_eq!(store.symbols.get(a_symbol).info, SymbolInfo::Error);
        assert_eq!(store.symbols.get(b_symbol).info, SymbolInfo::Error);
    }

    /// Confirms the "reuse an in-progress shell" technique
    /// (`docs/classloader.md` §5/§9, Milestone 6): `Ping.other` is typed
    /// `Pong`, and `Pong.other` is typed `Ping` — a completely
    /// legitimate mutual reference `javac` compiles without complaint
    /// (see `tests/fixtures/ping_pong/generate.sh`), unlike the
    /// synthetic supertype cycle above, which must still hard-error.
    /// Checking both sides proves the cycle resolves to consistent,
    /// fully-completed data rather than a permanently-empty shell.
    #[test]
    fn resolves_a_legitimate_mutual_field_type_reference_between_two_real_classes() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual field reference");
        drop(loader);

        let ping_other_type = member_type_id(&mut store, ping, "other");
        let pong = parent_symbol(&store, ping_other_type);
        assert_eq!(symbol_name(&store, pong), "Pong");

        let pong_other_type = member_type_id(&mut store, pong, "other");
        let ping_again = parent_symbol(&store, pong_other_type);
        assert_eq!(symbol_name(&store, ping_again), "Ping");
    }

    /// The other half of `resolve_member_class`'s `Loading`-tolerance
    /// story: `MutA.other: MutB` and `MutB.other: MutA` mutually resolve
    /// exactly like `Ping`/`Pong` above, and `MutB`'s load completes in
    /// full -- but `MutA` also has a *second* field, `broken`, typed a
    /// class that is not on the classpath at all, resolved strictly
    /// after `other` (`lower_field_type` walks a class's own `fields` in
    /// order), so `MutA`'s own load fails only after `MutB` has already
    /// captured `MutA`'s in-progress `SymbolId` in a real,
    /// fully-completed `Type::TypeRef`. Before this fix, that `SymbolId`
    /// stayed `SymbolInfo::Missing` forever once `MutA`'s own load
    /// failed -- reachable through `MutB` (which really did load, and
    /// whose `Type::TypeRef` really does still point at it) looking
    /// exactly like a legitimate, merely-not-yet-completed symbol
    /// instead of a permanently abandoned one.
    #[test]
    fn a_dependency_failure_marks_a_symbol_shared_via_mutual_reference_as_error() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("MutA"),
            synthetic_class_with_object_fields(
                "MutA",
                Some("java/lang/Object"),
                &[("other", "MutB"), ("broken", "DoesNotExist")],
            ),
        );
        classes.insert(
            BinaryName::from_internal("MutB"),
            synthetic_class_with_object_fields(
                "MutB",
                Some("java/lang/Object"),
                &[("other", "MutA")],
            ),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);

        let mut_a_error = loader
            .load_class(&BinaryName::from_internal("MutA"))
            .expect_err("MutA's own load should fail on its second, unresolvable field");
        assert!(matches!(
            mut_a_error,
            ClassLoadError::DependencyFailure { dependency, .. }
                if dependency.as_internal() == "DoesNotExist"
        ));

        // MutB's own load already fully succeeded as a side effect of
        // loading MutA, so this is a cache hit, not a fresh load -- the
        // only way to reach MutB's already-completed "other" field
        // through public API alone.
        let mut_b = loader
            .load_class(&BinaryName::from_internal("MutB"))
            .expect("MutB should have already loaded successfully");
        drop(loader);

        let mut_b_other_type = member_type_id(&mut store, mut_b, "other");
        let mut_a = parent_symbol(&store, mut_b_other_type);
        assert_eq!(symbol_name(&store, mut_a), "MutA");
        assert_eq!(store.symbols.get(mut_a).info, SymbolInfo::Error);
    }

    /// `lower_field_type`'s `FieldType::Array` branch recurses into its
    /// component type; every other test in this file
    /// only exercises plain `Object` fields, so this specifically
    /// targets `Ping.others: Pong[]` (real `javac` output, see
    /// `tests/fixtures/ping_pong/generate.sh`) to prove an
    /// array-of-`Object` field resolves its component class the same
    /// way a bare `Object` field does.
    #[test]
    fn resolves_the_component_class_of_an_array_typed_field() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load");
        drop(loader);

        let others_type = member_type_id(&mut store, ping, "others");
        let Type::JavaArray { element } = store.types.get(others_type) else {
            panic!("expected Ping.others to be a JavaArray type");
        };
        let pong = parent_symbol(&store, *element);

        assert_eq!(symbol_name(&store, pong), "Pong");
    }

    /// Same "reuse an in-progress shell" technique as the field-level
    /// test above, applied to a mutual *method* parameter/return type:
    /// `Ping.exchange(Pong): Pong` and `Pong.exchange(Ping): Ping`.
    #[test]
    fn resolves_a_legitimate_mutual_method_type_reference_between_two_real_classes() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual method reference");
        drop(loader);

        let exchange_type = member_type_id(&mut store, ping, "exchange");
        let Type::Method(method) = store.types.get(exchange_type) else {
            panic!("expected Ping.exchange to be a Type::Method");
        };
        let &[parameter] = method.params.as_slice() else {
            panic!(
                "expected exactly one parameter, got {}",
                method.params.len()
            );
        };
        let parameter_symbol = parent_symbol(&store, parameter.ty);
        let return_symbol = parent_symbol(&store, method.result);

        assert_eq!(symbol_name(&store, parameter_symbol), "Pong");
        assert_eq!(symbol_name(&store, return_symbol), "Pong");
    }

    /// `nested_sample/NestedSample.class` (real `javac` output, see
    /// `docs/classloader.md` §9 Milestone 7) is a nest host with two
    /// members: `NestedSample$Inner` (a static member class) and
    /// `NestedSample$1LocalRunnable` (an anonymous local class
    /// implementing `Runnable`). Loading `NestedSample` also eagerly
    /// resolves `max`'s erased-to-bound `Comparable` parameter/return
    /// type and `makeLocalRunnable`'s `String` parameter/`Runnable`
    /// return type, so those need synthetic stand-ins too, alongside
    /// `java/lang/Object`.
    fn nested_sample_classpath() -> HashMap<BinaryName, Vec<u8>> {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("NestedSample"),
            fixture_bytes("nested_sample/NestedSample.class"),
        );
        classes.insert(
            BinaryName::from_internal("NestedSample$Inner"),
            fixture_bytes("nested_sample/NestedSample$Inner.class"),
        );
        classes.insert(
            BinaryName::from_internal("NestedSample$1LocalRunnable"),
            fixture_bytes("nested_sample/NestedSample$1LocalRunnable.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Runnable"),
            synthetic_class("java/lang/Runnable", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Comparable"),
            synthetic_class("java/lang/Comparable", None),
        );
        classes
    }

    /// `NestedSample.max`'s real declaration is
    /// `public static <T extends Comparable<T>> T max(T a, T b) throws
    /// IllegalStateException` — a generic static method whose erased
    /// descriptor is `(Ljava/lang/Comparable;Ljava/lang/Comparable;)
    /// Ljava/lang/Comparable;` and whose real `Signature` attribute is
    /// `<T::Ljava/lang/Comparable<TT;>;>(TT;TT;)TT;` (confirmed via
    /// `javap -p -v`) — so it lowers to a real `Type::Poly` wrapping a
    /// `Type::Method` whose params/result reference `T` via `ParamRef`,
    /// not the erased `Comparable` the descriptor alone would give.
    #[test]
    fn lowers_a_real_fixtures_generic_static_method_into_a_poly_wrapped_method() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        drop(loader);

        let max_type = member_type_id(&mut store, class_symbol, "max");
        let Type::Poly(poly) = store.types.get(max_type) else {
            panic!("expected max's type to be a Type::Poly");
        };
        assert_eq!(poly.params.len(), 1);
        let type_param = poly.params[0];
        assert_eq!(store.names.resolve(type_param.name.as_name().text()), "T");
        let method_result = poly.result;

        // T's bound is the implicit Object intersected with its one
        // interface bound, Comparable[T] -- same shape as a class-level
        // F-bounded type parameter (`class_bound: None` means Object).
        let Type::And { left, right } = store.types.get(type_param.bounds) else {
            panic!("expected T's bound to be an intersection with the implicit Object bound");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *left)), "Object");
        let Type::Applied { tycon, args } = store.types.get(*right) else {
            panic!("expected T's interface bound to be an Applied Comparable[T]");
        };
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, *tycon)),
            "Comparable"
        );
        assert_eq!(args.len(), 1);
        assert_eq!(
            store.types.get(args[0]),
            &Type::ParamRef {
                binder: max_type,
                index: 0
            }
        );

        let Type::Method(method) = store.types.get(method_result) else {
            panic!("expected max's Poly result to be a Type::Method");
        };
        assert_eq!(method.params.len(), 2);
        assert_eq!(
            store.names.resolve(method.params[0].name.as_name().text()),
            "a"
        );
        assert_eq!(
            store.names.resolve(method.params[1].name.as_name().text()),
            "b"
        );
        for param in &method.params {
            assert_eq!(
                store.types.get(param.ty),
                &Type::ParamRef {
                    binder: max_type,
                    index: 0
                }
            );
        }
        assert_eq!(
            store.types.get(method.result),
            &Type::ParamRef {
                binder: max_type,
                index: 0
            }
        );
    }

    /// Confirms `NestMembers` (JVMS §4.7.29) names both of
    /// `NestedSample`'s real members. Kept `Unresolved` deliberately —
    /// see `resolve_nest_members`'s doc comment — so this only checks
    /// the names, not that the members were loaded.
    #[test]
    fn resolves_the_nest_members_of_a_real_nest_host() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample should have classfile metadata");
        let mut member_names: Vec<String> = metadata
            .nest_members
            .iter()
            .map(|member| match member {
                ClassRef::Unresolved(name) => name.as_internal().to_owned(),
                unexpected => panic!("expected an unresolved nest member ref, got {unexpected:?}"),
            })
            .collect();
        member_names.sort_unstable();

        assert_eq!(
            member_names,
            vec!["NestedSample$1LocalRunnable", "NestedSample$Inner"]
        );
    }

    /// Confirms `NestHost` (JVMS §4.7.28) names a real member's host.
    /// Kept `Unresolved` deliberately — see `resolve_optional_nest_host`'s
    /// doc comment for why this attribute is never eagerly loaded.
    #[test]
    fn resolves_the_nest_host_of_a_real_nest_member() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$Inner should have classfile metadata");
        assert!(matches!(
            &metadata.nest_host,
            Some(ClassRef::Unresolved(name)) if name.as_internal() == "NestedSample"
        ));
    }

    /// `sealed_record_sample/Shape.class` (real `javac` output) is a
    /// `sealed interface Shape permits Circle, Square` — this confirms
    /// `PermittedSubclasses` (JVMS §4.7.31) names both real permitted
    /// subclasses, without loading them (deliberately: see
    /// `resolve_permitted_subclasses`'s doc comment for why that would
    /// collide with cycle detection here, since both are records
    /// implementing `Shape` itself).
    #[test]
    fn resolves_the_permitted_subclasses_of_a_real_sealed_interface() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape"))
            .expect("Shape should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape should have classfile metadata");
        let mut permitted_names: Vec<String> = metadata
            .permitted_subclasses
            .iter()
            .map(|permitted| match permitted {
                ClassRef::Unresolved(name) => name.as_internal().to_owned(),
                unexpected => {
                    panic!("expected an unresolved permitted-subclass ref, got {unexpected:?}")
                }
            })
            .collect();
        permitted_names.sort_unstable();

        assert_eq!(permitted_names, vec!["Shape$Circle", "Shape$Square"]);
    }

    /// `nested_sample/NestedSample.class`'s real `InnerClasses`
    /// attribute (JVMS §4.7.6) has two entries in different shapes,
    /// confirmed via `javap -p -v`: a `public static` member class
    /// (`Inner`, with an outer class and a simple name) and a local
    /// class (`LocalRunnable`, with a simple name but no outer class —
    /// local/anonymous classes have no enclosing-class reference of
    /// their own in this attribute).
    #[test]
    fn resolves_the_inner_classes_of_a_real_nest_host() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample should have classfile metadata");
        let entries = &metadata.inner_classes;
        let inner = |name: &str| {
            entries
                .iter()
                .find(|entry| match &entry.inner_class {
                    ClassRef::Unresolved(inner_name) => inner_name.as_internal() == name,
                    ClassRef::Resolved(_) => false,
                })
                .unwrap_or_else(|| panic!("expected an InnerClasses entry for {name}"))
        };

        let member = inner("NestedSample$Inner");
        assert_eq!(member.inner_name.as_deref(), Some("Inner"));
        assert!(matches!(
            &member.outer_class,
            Some(ClassRef::Unresolved(name)) if name.as_internal() == "NestedSample"
        ));
        assert!(member.access_flags.is_public());

        let local = inner("NestedSample$1LocalRunnable");
        assert_eq!(local.inner_name.as_deref(), Some("LocalRunnable"));
        assert!(local.outer_class.is_none());
    }

    /// `nested_sample/NestedSample$1LocalRunnable.class`'s real
    /// `EnclosingMethod` attribute (JVMS §4.7.7), confirmed via
    /// `javap -p -v`: this anonymous local class is enclosed by
    /// `NestedSample.makeLocalRunnable(String):Runnable`.
    #[test]
    fn resolves_the_enclosing_method_of_a_real_local_class() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$1LocalRunnable should have classfile metadata");
        let enclosing = metadata
            .enclosing_method
            .as_ref()
            .expect("NestedSample$1LocalRunnable should have an EnclosingMethod attribute");

        assert!(matches!(
            &enclosing.class,
            ClassRef::Unresolved(name) if name.as_internal() == "NestedSample"
        ));
        assert_eq!(
            enclosing.method,
            Some((
                "makeLocalRunnable".to_owned(),
                "(Ljava/lang/String;)Ljava/lang/Runnable;".to_owned()
            ))
        );
    }

    /// A top-level class's `Symbol::owner` stays its package — this real
    /// `javac` fixture has no `InnerClasses`/`EnclosingMethod` attribute
    /// naming itself, so `resolve_semantic_owner` finds nothing to patch.
    #[test]
    fn a_top_level_classs_owner_is_still_its_package() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let owner = store
            .symbols
            .get(symbol)
            .owner
            .expect("a loaded top-level class always has a package owner");
        assert_eq!(store.symbols.get(owner).kind, SymbolKind::Package);
    }

    /// `NestedSample$Inner`'s own `InnerClasses` self-entry carries an
    /// `outer_class` (confirmed via `javap -p -v`, see
    /// `resolves_the_inner_classes_of_a_real_nest_host`'s doc comment), so
    /// its `Symbol::owner` becomes `NestedSample`'s own `SymbolId` — not
    /// the `""` (unnamed) package `BinaryName::package_path` would derive
    /// from its JVM `$`-joined binary name alone.
    #[test]
    fn a_member_classs_owner_is_its_real_enclosing_class_not_its_package() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let inner = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        assert_eq!(store.symbols.get(inner).owner, Some(outer));
    }

    /// `NestedSample$1LocalRunnable`'s own `InnerClasses` self-entry has
    /// no `outer_class` (it's local, not a member class), so
    /// `resolve_semantic_owner` falls back to its `EnclosingMethod`
    /// attribute's `class` reference — still `NestedSample`, not the
    /// specific `makeLocalRunnable` method (a deliberate simplification,
    /// see `resolve_semantic_owner`'s doc comment).
    #[test]
    fn a_local_classs_owner_is_its_enclosing_class_via_enclosing_method() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let local = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        assert_eq!(store.symbols.get(local).owner, Some(outer));
    }

    /// `Symbol::owner` becomes the enclosing class, but
    /// `Visibility::Package` must not follow it: a package-private nested
    /// class is visible everywhere in its *package*, not only from within
    /// its enclosing class. `NestedSample$1LocalRunnable` is package-
    /// private (confirmed via `javap -p`: no `public` modifier) while its
    /// new semantic owner `NestedSample` is `public` — so this also
    /// guards against a regression that conflates the two, which
    /// `assert_eq!(visibility, owner's visibility)` would not catch here
    /// since the two happen to differ.
    #[test]
    fn a_nested_classs_package_visibility_still_names_the_real_package_not_the_enclosing_class() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let local = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        let local_symbol = store.symbols.get(local);
        assert_eq!(local_symbol.owner, Some(outer));

        let Visibility::Package(package) = local_symbol.visibility else {
            panic!(
                "expected NestedSample$1LocalRunnable to be package-private, got {:?}",
                local_symbol.visibility
            );
        };
        assert_ne!(
            package, outer,
            "the package-private boundary must be the real package, not the enclosing class"
        );
        assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    }

    /// `sealed_record_sample/Shape$Circle.class` (real `javac` output) is
    /// `record Circle(double radius) implements Shape` — confirms
    /// `Record` (JVMS §4.7.30) decodes its one real, non-generic
    /// component.
    #[test]
    fn resolves_the_record_components_of_a_real_record_class() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape$Circle"),
            fixture_bytes("sealed_record_sample/Shape$Circle.class"),
        );
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Record"),
            synthetic_class("java/lang/Record", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape$Circle"))
            .expect("Shape$Circle should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape$Circle should have classfile metadata");
        let components = metadata
            .record_components
            .as_ref()
            .expect("Shape$Circle should have a Record attribute");

        assert!(matches!(
            components.as_slice(),
            [only] if only.name() == "radius" && only.field_type() == &FieldType::Double
        ));
    }

    /// `sealed_record_sample/Shape.class` is the sealed *interface*, not
    /// a record — confirms `record_components()` distinguishes "not a
    /// record" (`None`) from "a record with zero components"
    /// (`Some(vec![])`).
    #[test]
    fn a_non_record_class_has_no_record_components() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape"))
            .expect("Shape should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape should have classfile metadata");
        assert!(metadata.record_components.is_none());
    }

    /// `nested_sample/NestedSample$Inner.class`'s real
    /// `@Deprecated(since = "1.0", forRemoval = true)` annotation
    /// (confirmed via `javap -p -v`) exercises `RuntimeVisibleAnnotations`
    /// (JVMS §4.7.16) end to end: since `java/lang/Deprecated` is on the
    /// classpath, its type resolves to a real `SymbolId` (unlike
    /// `resolves_every_element_value_variant_from_a_synthetic_annotation`'s
    /// unresolvable annotation type below), so the class's own `Symbol`
    /// also carries a matching real `dotty_core::Annotation`, and the raw
    /// sidecar's `String`/`Boolean` element values decode correctly.
    #[test]
    fn resolves_a_real_annotation_on_a_class() {
        let mut classes = nested_sample_classpath();
        classes.insert(
            BinaryName::from_internal("java/lang/Deprecated"),
            synthetic_class("java/lang/Deprecated", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$Inner should have classfile metadata");
        let annotations = &metadata.annotations;
        let deprecated_symbol = annotations
            .iter()
            .find_map(|annotation| match &annotation.annotation_type {
                ClassRef::Resolved(annotation_symbol) => Some(*annotation_symbol),
                ClassRef::Unresolved(_) => None,
            })
            .expect("NestedSample$Inner should carry a resolved @Deprecated annotation");
        let deprecated = annotations
            .iter()
            .find(|annotation| annotation.annotation_type == ClassRef::Resolved(deprecated_symbol))
            .expect("NestedSample$Inner should carry a @Deprecated annotation");

        let element = |name: &str| {
            deprecated
                .elements
                .iter()
                .find(|(element_name, _)| element_name == name)
                .map(|(_, value)| value)
                .unwrap_or_else(|| panic!("expected a {name} element"))
        };

        assert!(matches!(
            element("since"),
            AnnotationValue::String(value) if value == "1.0"
        ));
        assert!(matches!(
            element("forRemoval"),
            AnnotationValue::Boolean(true)
        ));

        drop(loader);
        assert_eq!(symbol_name(&store, deprecated_symbol), "Deprecated");

        let symbol_annotations = &store.symbols.get(symbol).annotations;
        assert_eq!(symbol_annotations.len(), 1);
        let core_annotation = store.annotations.get(symbol_annotations[0]);
        assert_eq!(core_annotation.tree, None);
        assert_eq!(parent_symbol(&store, core_annotation.ty), deprecated_symbol);
    }

    /// A hand-built, minimal, synthetic class file carrying one
    /// `RuntimeVisibleAnnotations` annotation with one element of every
    /// `ElementValue` tag kind (JVMS §4.7.16.1) that no real fixture
    /// exercises: `Byte`/`Char`/`Int`/`Long`/`Float`/`Double`/`Short`/
    /// `Enum`/`Class`/nested `Annotation`/`Array`. `javac` cannot
    /// produce every one of these in a single real annotation (a
    /// user-defined annotation type's element kinds are fixed by its
    /// declaration), so this closes the gap left after Milestone 7's
    /// `@Deprecated`-based test only covered `String`/`Boolean`, the
    /// same way other `synthetic_class_with_*` helpers in this module
    /// hand-build bytes for shapes `javac` can't produce.
    fn synthetic_class_with_varied_annotation_element_values() -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_integer(pool: &mut Vec<u8>, next_index: &mut u16, value: i32) -> u16 {
            let index = *next_index;
            pool.push(3); // CONSTANT_Integer
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_long(pool: &mut Vec<u8>, next_index: &mut u16, value: i64) -> u16 {
            let index = *next_index;
            pool.push(5); // CONSTANT_Long
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 2; // a Long "counts as two entries" per JVMS §4.4.5
            index
        }
        fn push_float(pool: &mut Vec<u8>, next_index: &mut u16, value: f32) -> u16 {
            let index = *next_index;
            pool.push(4); // CONSTANT_Float
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_double(pool: &mut Vec<u8>, next_index: &mut u16, value: f64) -> u16 {
            let index = *next_index;
            pool.push(6); // CONSTANT_Double
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 2; // a Double "counts as two entries" per JVMS §4.4.5
            index
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;

        let class_name_index = push_utf8(&mut pool, &mut next_index, "C");
        let this_class_index = push_class(&mut pool, &mut next_index, class_name_index);

        let annotation_type_index = push_utf8(&mut pool, &mut next_index, "LTag;");

        let byte_name = push_utf8(&mut pool, &mut next_index, "byteField");
        let byte_value = push_integer(&mut pool, &mut next_index, 7);

        let char_name = push_utf8(&mut pool, &mut next_index, "charField");
        let char_value = push_integer(&mut pool, &mut next_index, 'A' as i32);

        let int_name = push_utf8(&mut pool, &mut next_index, "intField");
        let int_value = push_integer(&mut pool, &mut next_index, 123_456);

        let long_name = push_utf8(&mut pool, &mut next_index, "longField");
        let long_value = push_long(&mut pool, &mut next_index, 9_000_000_000);

        let float_name = push_utf8(&mut pool, &mut next_index, "floatField");
        let float_value = push_float(&mut pool, &mut next_index, 3.5);

        let double_name = push_utf8(&mut pool, &mut next_index, "doubleField");
        let double_value = push_double(&mut pool, &mut next_index, 2.5);

        let short_name = push_utf8(&mut pool, &mut next_index, "shortField");
        let short_value = push_integer(&mut pool, &mut next_index, 9);

        let class_name = push_utf8(&mut pool, &mut next_index, "classField");
        let class_value = push_utf8(&mut pool, &mut next_index, "Ljava/lang/String;");

        let enum_name = push_utf8(&mut pool, &mut next_index, "enumField");
        let enum_type = push_utf8(&mut pool, &mut next_index, "LKind;");
        let enum_const = push_utf8(&mut pool, &mut next_index, "FOO");

        let annotation_name = push_utf8(&mut pool, &mut next_index, "annotationField");
        let nested_type = push_utf8(&mut pool, &mut next_index, "LNested;");
        let nested_element_name = push_utf8(&mut pool, &mut next_index, "value");
        let nested_string_value = push_utf8(&mut pool, &mut next_index, "nested-value");

        let array_name = push_utf8(&mut pool, &mut next_index, "arrayField");
        let array_element_1 = push_integer(&mut pool, &mut next_index, 1);
        let array_element_2 = push_integer(&mut pool, &mut next_index, 2);
        let array_element_3 = push_integer(&mut pool, &mut next_index, 3);

        let attribute_name_index =
            push_utf8(&mut pool, &mut next_index, "RuntimeVisibleAnnotations");

        let mut annotation_bytes = Vec::new();
        annotation_bytes.extend_from_slice(&annotation_type_index.to_be_bytes());
        annotation_bytes.extend_from_slice(&11u16.to_be_bytes()); // num_element_value_pairs

        annotation_bytes.extend_from_slice(&byte_name.to_be_bytes());
        annotation_bytes.push(b'B');
        annotation_bytes.extend_from_slice(&byte_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&char_name.to_be_bytes());
        annotation_bytes.push(b'C');
        annotation_bytes.extend_from_slice(&char_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&int_name.to_be_bytes());
        annotation_bytes.push(b'I');
        annotation_bytes.extend_from_slice(&int_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&long_name.to_be_bytes());
        annotation_bytes.push(b'J');
        annotation_bytes.extend_from_slice(&long_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&float_name.to_be_bytes());
        annotation_bytes.push(b'F');
        annotation_bytes.extend_from_slice(&float_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&double_name.to_be_bytes());
        annotation_bytes.push(b'D');
        annotation_bytes.extend_from_slice(&double_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&short_name.to_be_bytes());
        annotation_bytes.push(b'S');
        annotation_bytes.extend_from_slice(&short_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&class_name.to_be_bytes());
        annotation_bytes.push(b'c');
        annotation_bytes.extend_from_slice(&class_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&enum_name.to_be_bytes());
        annotation_bytes.push(b'e');
        annotation_bytes.extend_from_slice(&enum_type.to_be_bytes());
        annotation_bytes.extend_from_slice(&enum_const.to_be_bytes());

        annotation_bytes.extend_from_slice(&annotation_name.to_be_bytes());
        annotation_bytes.push(b'@');
        annotation_bytes.extend_from_slice(&nested_type.to_be_bytes());
        annotation_bytes.extend_from_slice(&1u16.to_be_bytes()); // nested num_element_value_pairs
        annotation_bytes.extend_from_slice(&nested_element_name.to_be_bytes());
        annotation_bytes.push(b's');
        annotation_bytes.extend_from_slice(&nested_string_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&array_name.to_be_bytes());
        annotation_bytes.push(b'[');
        annotation_bytes.extend_from_slice(&3u16.to_be_bytes()); // num_values
        for element in [array_element_1, array_element_2, array_element_3] {
            annotation_bytes.push(b'I');
            annotation_bytes.extend_from_slice(&element.to_be_bytes());
        }

        let mut attribute_body = Vec::new();
        attribute_body.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
        attribute_body.extend_from_slice(&annotation_bytes);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
        bytes.extend_from_slice(&[0x00, 0x01]); // attributes_count = 1
        bytes.extend_from_slice(&attribute_name_index.to_be_bytes());
        bytes.extend_from_slice(&(attribute_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&attribute_body);
        bytes
    }

    #[test]
    fn resolves_every_element_value_variant_from_a_synthetic_annotation() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_varied_annotation_element_values(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");

        let metadata = loader
            .metadata(symbol)
            .expect("C should have classfile metadata");
        let annotations = &metadata.annotations;
        let tag = annotations
            .first()
            .expect("C should carry the synthetic annotation");
        assert!(matches!(
            &tag.annotation_type,
            ClassRef::Unresolved(name) if name.as_internal() == "Tag"
        ));

        let element = |name: &str| {
            tag.elements
                .iter()
                .find(|(element_name, _)| element_name == name)
                .map(|(_, value)| value)
                .unwrap_or_else(|| panic!("expected a {name} element"))
        };

        assert!(matches!(element("byteField"), AnnotationValue::Byte(7)));
        assert!(matches!(
            element("charField"),
            AnnotationValue::Char(value) if *value == 'A' as i32
        ));
        assert!(matches!(element("intField"), AnnotationValue::Int(123_456)));
        assert!(matches!(
            element("longField"),
            AnnotationValue::Long(9_000_000_000)
        ));
        assert!(matches!(
            element("floatField"),
            AnnotationValue::Float(value) if *value == 3.5
        ));
        assert!(matches!(
            element("doubleField"),
            AnnotationValue::Double(value) if *value == 2.5
        ));
        assert!(matches!(element("shortField"), AnnotationValue::Short(9)));
        assert!(matches!(
            element("classField"),
            AnnotationValue::Class(value) if value == "Ljava/lang/String;"
        ));
        assert!(matches!(
            element("enumField"),
            AnnotationValue::Enum { type_descriptor, const_name }
                if type_descriptor == "LKind;" && const_name == "FOO"
        ));

        match element("annotationField") {
            AnnotationValue::Annotation(nested) => {
                assert!(matches!(
                    &nested.annotation_type,
                    ClassRef::Unresolved(name) if name.as_internal() == "Nested"
                ));
                assert!(matches!(
                    nested.elements.as_slice(),
                    [(name, AnnotationValue::String(value))]
                        if name == "value" && value == "nested-value"
                ));
            }
            unexpected => panic!("expected a nested annotation, got {unexpected:?}"),
        }

        match element("arrayField") {
            AnnotationValue::Array(values) => {
                let ints: Vec<i32> = values
                    .iter()
                    .map(|value| match value {
                        AnnotationValue::Int(value) => *value,
                        unexpected => panic!("expected an Int array element, got {unexpected:?}"),
                    })
                    .collect();
                assert_eq!(ints, vec![1, 2, 3]);
            }
            unexpected => panic!("expected an array element value, got {unexpected:?}"),
        }

        // "Tag" is not on this test's classpath, so it stays
        // `ClassRef::Unresolved` (per `resolve_annotation`'s doc comment)
        // and never gets a `dotty_core::Annotation` on the class's own
        // `Symbol` -- an annotation whose interface cannot be loaded is
        // not a class-load failure, but it also cannot become a real,
        // typed `Symbol::annotations` entry.
        drop(loader);
        assert!(store.symbols.get(symbol).annotations.is_empty());
    }

    /// A hand-built, minimal, synthetic class file with one field and one
    /// method, each carrying a single `RuntimeVisibleAnnotations` entry for
    /// an `Ann` annotation type that *is* on the classpath — exercises
    /// `ClassLoader::enter_annotations` on a field/method `Symbol`, not
    /// just a class's (already covered by `resolves_a_real_annotation_on_a_class`).
    fn synthetic_class_with_field_and_method_annotations() -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }
        fn annotations_attribute_body(type_index: u16) -> Vec<u8> {
            let mut body = Vec::new();
            body.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
            body.extend_from_slice(&type_index.to_be_bytes());
            body.extend_from_slice(&0u16.to_be_bytes()); // num_element_value_pairs
            body
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;
        let this_name_index = push_utf8(&mut pool, &mut next_index, "C");
        let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
        let field_name_index = push_utf8(&mut pool, &mut next_index, "field");
        let field_descriptor_index = push_utf8(&mut pool, &mut next_index, "I");
        let method_name_index = push_utf8(&mut pool, &mut next_index, "method");
        let method_descriptor_index = push_utf8(&mut pool, &mut next_index, "()V");
        let attr_name_index = push_utf8(&mut pool, &mut next_index, "RuntimeVisibleAnnotations");
        let ann_type_index = push_utf8(&mut pool, &mut next_index, "LAnn;");
        let annotation_body = annotations_attribute_body(ann_type_index);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        bytes.extend_from_slice(&[0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0x45]);
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&field_name_index.to_be_bytes());
        bytes.extend_from_slice(&field_descriptor_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]); // field attributes_count = 1
        bytes.extend_from_slice(&attr_name_index.to_be_bytes());
        bytes.extend_from_slice(&(annotation_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&annotation_body);
        bytes.extend_from_slice(&[0x00, 0x01]); // methods_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // method access_flags
        bytes.extend_from_slice(&method_name_index.to_be_bytes());
        bytes.extend_from_slice(&method_descriptor_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]); // method attributes_count = 1
        bytes.extend_from_slice(&attr_name_index.to_be_bytes());
        bytes.extend_from_slice(&(annotation_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&annotation_body);
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0 (class-level)
        bytes
    }

    #[test]
    fn enters_a_resolved_field_and_method_annotation_onto_their_own_symbols() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_field_and_method_annotations(),
        );
        classes.insert(
            BinaryName::from_internal("Ann"),
            synthetic_class("Ann", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");
        drop(loader);

        let declarations = class_info(&store, class_symbol).declarations;
        let mut member_symbol = |member_name: &str| {
            let text = store.names.intern(member_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"))
        };
        let field_symbol = member_symbol("field");
        let method_symbol = member_symbol("method");

        for symbol in [field_symbol, method_symbol] {
            let symbol_annotations = &store.symbols.get(symbol).annotations;
            assert_eq!(symbol_annotations.len(), 1);
            let core_annotation = store.annotations.get(symbol_annotations[0]);
            assert_eq!(core_annotation.tree, None);
            assert_eq!(
                symbol_name(&store, parent_symbol(&store, core_annotation.ty)),
                "Ann"
            );
        }
    }

    /// `tasty_visibility` (issue #16): a package qualifier that is the
    /// class's own or an enclosing package keeps its qualifier; anything else
    /// narrows to plain `Private`/`Protected`, never `Public`.
    #[test]
    fn tasty_visibility_keeps_an_enclosing_package_qualifier_and_narrows_the_rest() {
        use tasty_symbol::{DeclaredQualifier, DeclaredVisibility};

        let package = |path: &str| DeclaredQualifier::Package(path.to_owned());
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);
        let class = BinaryName::from_internal("a/b/C");

        let own =
            loader.tasty_visibility(&class, &DeclaredVisibility::PrivateWithin(package("a/b")));
        let outer =
            loader.tasty_visibility(&class, &DeclaredVisibility::ProtectedWithin(package("a")));
        let unrelated =
            loader.tasty_visibility(&class, &DeclaredVisibility::PrivateWithin(package("x/y")));
        // `a/bc` merely starts with the text of `a/b`; it does not enclose.
        let prefix_only = loader.tasty_visibility(
            &BinaryName::from_internal("a/bc/C"),
            &DeclaredVisibility::ProtectedWithin(package("a/b")),
        );
        let other = loader.tasty_visibility(
            &class,
            &DeclaredVisibility::ProtectedWithin(DeclaredQualifier::Other),
        );
        let a_b = loader.session.packages.resolve_package(loader.store, "a/b");
        let a = loader.session.packages.resolve_package(loader.store, "a");

        assert_eq!(own, Visibility::PrivateWithin(a_b));
        assert_eq!(outer, Visibility::ProtectedWithin(a));
        assert_eq!(unrelated, Visibility::Private);
        assert_eq!(prefix_only, Visibility::Protected);
        assert_eq!(other, Visibility::Protected);
    }
}
