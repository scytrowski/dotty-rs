use crate::binary_name::BinaryName;
use crate::class_path::ClassPathEntry;
use crate::error::ClassLoadError;
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::nesting::InnerClassEntry;
use crate::repository::{ClassEntry, ClassRepository};
use crate::semantic_type::{SemanticFieldType, SemanticMethodDescriptor};
use crate::symbol::{ClassRef, ClassSymbol};
use dotty_classfile::access_flags::ClassAccessFlags;
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolIndex};
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{ClassSignature, FieldSignature, MethodSignature, SignatureError};
use std::cell::RefCell;
use std::rc::Rc;

/// Loads `.class`-backed [`ClassSymbol`]s from a [`ClassPathEntry`],
/// caching results in a [`ClassRepository`].
///
/// Implements the "enter before complete" algorithm from
/// `docs/classloader.md` §5: a class is marked `Loading` before its
/// superclass/interfaces are resolved, so a class that (in)directly
/// references itself as a supertype is reported as
/// [`ClassLoadError::CircularInheritance`] instead of recursing forever.
pub struct ClassLoader<E> {
    class_path: E,
    repository: RefCell<ClassRepository>,
}

impl<E: ClassPathEntry> ClassLoader<E> {
    pub fn new(class_path: E) -> Self {
        Self {
            class_path,
            repository: RefCell::new(ClassRepository::new()),
        }
    }

    /// Loads (or returns the cached result for) the class named `name`.
    pub fn load_class(&self, name: &BinaryName) -> Result<Rc<ClassSymbol>, ClassLoadError> {
        // Cloned out of its own block so the borrow is dropped before any
        // of the arms below try to `borrow_mut()` the same repository.
        let cached = {
            let repository = self.repository.borrow();
            repository.get(name).cloned()
        };

        match cached {
            Some(ClassEntry::Loaded(symbol)) => return Ok(symbol),
            Some(ClassEntry::Failed(error)) => return Err(error),
            Some(ClassEntry::Loading(_)) => {
                let error = ClassLoadError::CircularInheritance(name.clone());
                self.repository
                    .borrow_mut()
                    .mark_failed(name.clone(), error.clone());
                return Err(error);
            }
            None => {}
        }

        match self.load_uncached(name) {
            Ok(symbol) => {
                self.repository
                    .borrow_mut()
                    .mark_loaded(name.clone(), symbol.clone());
                Ok(symbol)
            }
            Err(error) => {
                self.repository
                    .borrow_mut()
                    .mark_failed(name.clone(), error.clone());
                Err(error)
            }
        }
    }

    fn load_uncached(&self, name: &BinaryName) -> Result<Rc<ClassSymbol>, ClassLoadError> {
        let resource = self
            .class_path
            .find_class(name)
            .map_err(|error| ClassLoadError::Io(name.clone(), Rc::new(error)))?
            .ok_or_else(|| ClassLoadError::NotFound(name.clone()))?;

        let mut reader = Reader::new(resource.bytes());
        let class_file = ClassFile::decode(&mut reader)
            .map_err(|error| ClassLoadError::InvalidClassFile(name.clone(), error))?;

        let actual_name = self.resolve_name(name, &class_file, class_file.this_class)?;
        if actual_name != *name {
            return Err(ClassLoadError::NameMismatch {
                requested: name.clone(),
                actual: actual_name,
            });
        }

        // Registered before any recursive resolution below, so a
        // legitimate mutual member-type reference back to `name` (see
        // `resolve_member_class`) can reuse this same shell instead of
        // erroring, and a genuine supertype cycle back to `name` (via
        // `resolve_dependency` -> `load_class`) still hits `Loading` and
        // is rejected as `CircularInheritance`, exactly as before.
        let shell = Rc::new(ClassSymbol::new_shell(
            name.clone(),
            class_file.access_flags,
        ));
        self.repository
            .borrow_mut()
            .mark_loading(name.clone(), shell.clone());

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
            let semantic_type = self.resolve_semantic_field_type(name, &field_type)?;
            let signature = self.resolve_optional_signature(
                name,
                &field.attributes,
                &class_file.constant_pool,
                FieldSignature::parse,
            )?;
            fields.push(FieldSymbol::new(
                field_name,
                field.access_flags,
                field_type,
                signature,
                semantic_type,
            ));
        }

        let mut methods = Vec::with_capacity(class_file.methods.len());
        for method in &class_file.methods {
            let method_name =
                self.resolve_member_name(name, &class_file.constant_pool, method.name_index)?;
            let descriptor = method
                .descriptor(&class_file.constant_pool)
                .map_err(|error| ClassLoadError::MalformedDescriptor(name.clone(), error))?;
            let semantic_descriptor = self.resolve_semantic_method_descriptor(name, &descriptor)?;
            let signature = self.resolve_optional_signature(
                name,
                &method.attributes,
                &class_file.constant_pool,
                MethodSignature::parse,
            )?;
            methods.push(MethodSymbol::new(
                method_name,
                method.access_flags,
                descriptor,
                signature,
                semantic_descriptor,
            ));
        }

        let signature = self.resolve_optional_signature(
            name,
            &class_file.attributes,
            &class_file.constant_pool,
            ClassSignature::parse,
        )?;

        let nest_host = self.resolve_optional_nest_host(name, &class_file)?;
        let nest_members = self.resolve_nest_members(name, &class_file)?;
        let permitted_subclasses = self.resolve_permitted_subclasses(name, &class_file)?;
        let inner_classes = self.resolve_inner_classes(name, &class_file)?;

        shell.complete(
            super_class,
            interfaces,
            fields,
            methods,
            signature,
            nest_host,
            nest_members,
            permitted_subclasses,
            inner_classes,
            None,
            None,
            Vec::new(),
        );
        Ok(shell)
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
        &self,
        owner: &BinaryName,
        class_file: &ClassFile<'_>,
        index: ConstantPoolIndex,
    ) -> Result<ClassRef, ClassLoadError> {
        let dependency = self.resolve_name(owner, class_file, index)?;

        self.load_class(&dependency)
            .map(ClassRef::Resolved)
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
    /// "reusing an in-progress shell" technique `docs/classloader.md`
    /// §5/§9 (Milestone 6) describes: two classes each having a field or
    /// method typed as the other is completely legitimate Java, unlike
    /// a supertype cycle, so hitting `Loading` here returns the
    /// in-progress shell directly instead of erroring or re-entering
    /// [`Self::load_class`]. Every other state (`Loaded`/`Failed`/
    /// absent) just delegates to `load_class` as normal.
    fn resolve_member_class(
        &self,
        owner: &BinaryName,
        dependency: &BinaryName,
    ) -> Result<Rc<ClassSymbol>, ClassLoadError> {
        let cached = {
            let repository = self.repository.borrow();
            repository.get(dependency).cloned()
        };

        match cached {
            Some(ClassEntry::Loading(shell)) => Ok(shell),
            _ => self
                .load_class(dependency)
                .map_err(|source| ClassLoadError::DependencyFailure {
                    owner: owner.clone(),
                    dependency: dependency.clone(),
                    source: Rc::new(source),
                }),
        }
    }

    /// Walks an already-parsed [`FieldType`] (JVMS §4.3.2), resolving
    /// every `Object`/array-of-`Object` leaf into a [`SemanticFieldType::Object`]
    /// via [`Self::resolve_member_class`]. Base types map straight
    /// across; `Array` recurses into its component type.
    fn resolve_semantic_field_type(
        &self,
        owner: &BinaryName,
        field_type: &FieldType,
    ) -> Result<SemanticFieldType, ClassLoadError> {
        match field_type {
            FieldType::Byte => Ok(SemanticFieldType::Byte),
            FieldType::Char => Ok(SemanticFieldType::Char),
            FieldType::Double => Ok(SemanticFieldType::Double),
            FieldType::Float => Ok(SemanticFieldType::Float),
            FieldType::Int => Ok(SemanticFieldType::Int),
            FieldType::Long => Ok(SemanticFieldType::Long),
            FieldType::Short => Ok(SemanticFieldType::Short),
            FieldType::Boolean => Ok(SemanticFieldType::Boolean),
            FieldType::Object(class_name) => {
                let dependency = BinaryName::from_internal(class_name);
                let symbol = self.resolve_member_class(owner, &dependency)?;
                Ok(SemanticFieldType::Object(ClassRef::Resolved(symbol)))
            }
            FieldType::Array(component) => {
                let resolved = self.resolve_semantic_field_type(owner, component)?;
                Ok(SemanticFieldType::Array(Box::new(resolved)))
            }
        }
    }

    /// Walks an already-parsed [`MethodDescriptor`] (JVMS §4.3.3) the
    /// same way [`Self::resolve_semantic_field_type`] walks a field's
    /// type, applied to every parameter and the return type.
    fn resolve_semantic_method_descriptor(
        &self,
        owner: &BinaryName,
        descriptor: &MethodDescriptor,
    ) -> Result<SemanticMethodDescriptor, ClassLoadError> {
        let mut parameters = Vec::with_capacity(descriptor.parameters.len());
        for parameter in &descriptor.parameters {
            parameters.push(self.resolve_semantic_field_type(owner, parameter)?);
        }

        let return_type = descriptor
            .return_type
            .as_ref()
            .map(|field_type| self.resolve_semantic_field_type(owner, field_type))
            .transpose()?;

        Ok(SemanticMethodDescriptor {
            parameters,
            return_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::{ClassOrigin, ClassPathError, ClassResource};
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

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

    #[test]
    fn loads_a_real_fixture_with_a_super_class_and_an_interface() {
        let loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        assert_eq!(symbol.name().as_internal(), "PoolSample");
        assert!(matches!(
            symbol.super_class(),
            Some(ClassRef::Resolved(super_symbol))
                if super_symbol.name().as_internal() == "java/lang/Object"
        ));
        assert!(matches!(
            symbol.interfaces().as_slice(),
            [ClassRef::Resolved(interface_symbol)]
                if interface_symbol.name().as_internal() == "java/lang/Runnable"
        ));
    }

    /// `pool_sample/PoolSample.class` declares 5 real `public static
    /// final` fields (confirmed via `javap -p -v`); this asserts every
    /// one decodes with the right name, type, and flags.
    #[test]
    fn loads_a_real_fixtures_fields() {
        use dotty_classfile::descriptor::FieldType;

        let loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        let all_fields = symbol.fields();
        let field = |name: &str| {
            all_fields
                .iter()
                .find(|field| field.name() == name)
                .unwrap_or_else(|| panic!("field {name} should exist"))
        };

        assert_eq!(symbol.fields().len(), 5);
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

    /// `pool_sample/PoolSample.class` declares 3 real methods (confirmed
    /// via `javap -p -v`): the implicit `<init>()V` constructor,
    /// `run()V` (overriding `Runnable.run`), and `computeAnswer()I`
    /// (`private static`) — this asserts every one decodes with the
    /// right name, descriptor, and flags.
    #[test]
    fn loads_a_real_fixtures_methods() {
        use dotty_classfile::descriptor::{FieldType, MethodDescriptor};

        let loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load");

        let all_methods = symbol.methods();
        let method = |name: &str| {
            all_methods
                .iter()
                .find(|method| method.name() == name)
                .unwrap_or_else(|| panic!("method {name} should exist"))
        };

        assert_eq!(symbol.methods().len(), 3);
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

        let loader = ClassLoader::new(InMemoryClassPath(generic_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        let all_fields = symbol.fields();
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

        let loader = ClassLoader::new(InMemoryClassPath(generic_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        let all_methods = symbol.methods();
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

        let loader = ClassLoader::new(InMemoryClassPath(generic_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("GenericSample"))
            .expect("GenericSample should load");

        assert_eq!(
            symbol.signature(),
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let error = loader
            .load_class(&BinaryName::from_internal("C"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedSignature(name, _) if name.as_internal() == "C"
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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
        let jar_class_path = JarClassPath::new(jar_path).unwrap();

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

        let loader = ClassLoader::new(composite);
        let symbol = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .expect("PoolSample should load from the JAR");

        assert_eq!(symbol.name().as_internal(), "PoolSample");
        assert!(matches!(
            symbol.super_class(),
            Some(ClassRef::Resolved(super_symbol))
                if super_symbol.name().as_internal() == "java/lang/Object"
        ));
        assert!(matches!(
            symbol.interfaces().as_slice(),
            [ClassRef::Resolved(interface_symbol)]
                if interface_symbol.name().as_internal() == "java/lang/Runnable"
        ));
    }

    #[test]
    fn loads_pool_sample_from_a_stored_jar_via_composite_class_path() {
        assert_loads_pool_sample_from_jar("pool_sample_stored.jar");
    }

    #[test]
    fn loads_pool_sample_from_a_real_deflate_compressed_jar_via_composite_class_path() {
        assert_loads_pool_sample_from_jar("pool_sample_deflate.jar");
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

        let loader = ClassLoader::new(composite);
        let symbol = loader
            .load_class(&BinaryName::from_internal("pool/PoolSample"))
            .expect("pool/PoolSample should load from the JDK class path");

        assert_eq!(symbol.name().as_internal(), "pool/PoolSample");
        assert!(matches!(
            symbol.super_class(),
            Some(ClassRef::Resolved(super_symbol))
                if super_symbol.name().as_internal() == "java/lang/Object"
        ));
        assert!(matches!(
            symbol.interfaces().as_slice(),
            [ClassRef::Resolved(interface_symbol)]
                if interface_symbol.name().as_internal() == "java/lang/Runnable"
        ));
    }

    #[test]
    fn returns_the_same_cached_symbol_on_a_second_load() {
        let loader = ClassLoader::new(InMemoryClassPath(pool_sample_classpath()));
        let name = BinaryName::from_internal("PoolSample");

        let first = loader.load_class(&name).unwrap();
        let second = loader.load_class(&name).unwrap();

        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn not_found_when_the_classpath_has_nothing_for_the_name() {
        let loader = ClassLoader::new(InMemoryClassPath(HashMap::new()));

        let error = loader
            .load_class(&BinaryName::from_internal("Missing"))
            .unwrap_err();

        assert!(matches!(error, ClassLoadError::NotFound(name) if name.as_internal() == "Missing"));
    }

    #[test]
    fn io_error_from_the_classpath_is_wrapped_and_not_cached_as_not_found() {
        let loader = ClassLoader::new(AlwaysErrors);

        let error = loader
            .load_class(&BinaryName::from_internal("Anything"))
            .unwrap_err();

        assert!(matches!(error, ClassLoadError::Io(name, _) if name.as_internal() == "Anything"));
    }

    #[test]
    fn invalid_class_file_when_the_bytes_do_not_decode() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::InvalidClassFile(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn malformed_reference_when_this_class_does_not_resolve() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Broken"),
            synthetic_malformed_this_class(),
        );

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual field reference");

        let ping_fields = ping.fields();
        let ping_other = ping_fields
            .iter()
            .find(|field| field.name() == "other")
            .expect("Ping.other should exist");

        let pong = match ping_other.semantic_type() {
            SemanticFieldType::Object(ClassRef::Resolved(symbol)) => symbol.clone(),
            unexpected => panic!("expected Ping.other to resolve to a class, got {unexpected:?}"),
        };
        assert_eq!(pong.name().as_internal(), "Pong");

        let pong_fields = pong.fields();
        let pong_other = pong_fields
            .iter()
            .find(|field| field.name() == "other")
            .expect("Pong.other should exist");

        match pong_other.semantic_type() {
            SemanticFieldType::Object(ClassRef::Resolved(symbol)) => {
                assert_eq!(symbol.name().as_internal(), "Ping");
            }
            unexpected => panic!("expected Pong.other to resolve to a class, got {unexpected:?}"),
        }
    }

    /// `resolve_semantic_field_type`'s `FieldType::Array` branch
    /// recurses into its component type; every other test in this file
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load");

        let ping_fields = ping.fields();
        let others = ping_fields
            .iter()
            .find(|field| field.name() == "others")
            .expect("Ping.others should exist");

        match others.semantic_type() {
            SemanticFieldType::Array(component) => match component.as_ref() {
                SemanticFieldType::Object(ClassRef::Resolved(symbol)) => {
                    assert_eq!(symbol.name().as_internal(), "Pong");
                }
                unexpected => {
                    panic!(
                        "expected the array's component to resolve to a class, got {unexpected:?}"
                    )
                }
            },
            unexpected => panic!("expected Ping.others to be an array type, got {unexpected:?}"),
        }
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual method reference");

        let ping_methods = ping.methods();
        let exchange = ping_methods
            .iter()
            .find(|method| method.name() == "exchange")
            .expect("Ping.exchange should exist");

        assert!(matches!(
            exchange.semantic_descriptor().parameters.as_slice(),
            [SemanticFieldType::Object(ClassRef::Resolved(symbol))]
                if symbol.name().as_internal() == "Pong"
        ));
        assert!(matches!(
            &exchange.semantic_descriptor().return_type,
            Some(SemanticFieldType::Object(ClassRef::Resolved(symbol)))
                if symbol.name().as_internal() == "Pong"
        ));
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

    /// Confirms `NestMembers` (JVMS §4.7.29) names both of
    /// `NestedSample`'s real members. Kept `Unresolved` deliberately —
    /// see `resolve_nest_members`'s doc comment — so this only checks
    /// the names, not that the members were loaded.
    #[test]
    fn resolves_the_nest_members_of_a_real_nest_host() {
        let loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let mut member_names: Vec<String> = symbol
            .nest_members()
            .into_iter()
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
        let loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        assert!(matches!(
            symbol.nest_host(),
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

        let loader = ClassLoader::new(InMemoryClassPath(classes));
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape"))
            .expect("Shape should load");

        let mut permitted_names: Vec<String> = symbol
            .permitted_subclasses()
            .into_iter()
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
        let loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()));
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let entries = symbol.inner_classes();
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
}
