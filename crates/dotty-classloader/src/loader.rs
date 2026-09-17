use crate::binary_name::BinaryName;
use crate::class_path::ClassPathEntry;
use crate::error::ClassLoadError;
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::repository::{ClassEntry, ClassRepository};
use crate::symbol::{ClassRef, ClassSymbol};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{FieldSignature, SignatureError};
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
            Some(ClassEntry::Loading) => {
                let error = ClassLoadError::CircularInheritance(name.clone());
                self.repository
                    .borrow_mut()
                    .mark_failed(name.clone(), error.clone());
                return Err(error);
            }
            None => {}
        }

        self.repository.borrow_mut().mark_loading(name.clone());

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
            ));
        }

        let mut methods = Vec::with_capacity(class_file.methods.len());
        for method in &class_file.methods {
            let method_name =
                self.resolve_member_name(name, &class_file.constant_pool, method.name_index)?;
            let descriptor = method
                .descriptor(&class_file.constant_pool)
                .map_err(|error| ClassLoadError::MalformedDescriptor(name.clone(), error))?;
            methods.push(MethodSymbol::new(
                method_name,
                method.access_flags,
                descriptor,
            ));
        }

        Ok(Rc::new(ClassSymbol::new(
            name.clone(),
            class_file.access_flags,
            super_class,
            interfaces,
            fields,
            methods,
        )))
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
    /// and `java/lang/Runnable`. No JDK classpath exists yet (Milestone
    /// 3), so these are stood in with hand-built, minimal, clearly
    /// synthetic classes on the same test classpath, alongside the real
    /// fixture bytes.
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
            symbol.interfaces(),
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

        let field = |name: &str| {
            symbol
                .fields()
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

        let method = |name: &str| {
            symbol
                .methods()
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
    /// superclass (implicitly `java/lang/Object`) and no interfaces, so
    /// only a synthetic `java/lang/Object` fallback is needed alongside
    /// the real fixture bytes.
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

        let items = symbol
            .fields()
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
            symbol.interfaces(),
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
            symbol.interfaces(),
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
}
