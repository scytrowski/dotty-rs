//! Decodes real JDK 25-compiled class files fully through `ClassFile::decode`
//! and spot-checks the attributes each fixture was built to exercise.
//! Sources live alongside their compiled `.class` files under
//! `tests/fixtures/`; indices below come from `javap -v` on the exact
//! checked-in bytes.

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::field::FieldInfo;
use dotty_classfile::method::MethodInfo;
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{MethodSignature, ReferenceTypeSignature, TypeSignature};

const POOL_SAMPLE: &[u8] = include_bytes!("fixtures/pool_sample/PoolSample.class");
const NESTED_SAMPLE: &[u8] = include_bytes!("fixtures/nested_sample/NestedSample.class");
const NESTED_SAMPLE_INNER: &[u8] =
    include_bytes!("fixtures/nested_sample/NestedSample$Inner.class");
const NESTED_SAMPLE_LOCAL_RUNNABLE: &[u8] =
    include_bytes!("fixtures/nested_sample/NestedSample$1LocalRunnable.class");
const SHAPE: &[u8] = include_bytes!("fixtures/sealed_record_sample/Shape.class");
const SHAPE_CIRCLE: &[u8] = include_bytes!("fixtures/sealed_record_sample/Shape$Circle.class");

fn decode(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    ClassFile::decode(&mut reader).unwrap()
}

fn utf8_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Utf8(name)) => name.as_str(),
        other => panic!("expected Utf8 at {index:?}, got {other:?}"),
    }
}

fn class_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Class { name_index }) => utf8_name(pool, *name_index),
        other => panic!("expected Class at {index:?}, got {other:?}"),
    }
}

fn find_method<'a, 'b>(class_file: &'b ClassFile<'a>, name: &str) -> &'b MethodInfo<'a> {
    class_file
        .methods
        .iter()
        .find(|method| utf8_name(&class_file.constant_pool, method.name_index) == name)
        .unwrap_or_else(|| panic!("method {name} not found"))
}

fn find_field<'a, 'b>(class_file: &'b ClassFile<'a>, name: &str) -> &'b FieldInfo<'a> {
    class_file
        .fields
        .iter()
        .find(|field| utf8_name(&class_file.constant_pool, field.name_index) == name)
        .unwrap_or_else(|| panic!("field {name} not found"))
}

#[test]
fn decodes_pool_sample_completely() {
    let class_file = decode(POOL_SAMPLE);

    assert_eq!(class_file.version.major, 69);
    assert_eq!(class_file.fields.len(), 5);
    assert_eq!(class_file.methods.len(), 3);
}

#[test]
fn decodes_nested_sample_top_level_attributes() {
    let class_file = decode(NESTED_SAMPLE);
    let pool = &class_file.constant_pool;

    assert!(
        class_file
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::NestMembers(m) if m.len() == 2)),
        "expected a NestMembers attribute with 2 entries"
    );
    assert!(
        class_file
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::InnerClasses(c) if c.len() == 2)),
        "expected an InnerClasses attribute with 2 entries"
    );
    assert!(
        class_file.attributes.iter().any(|attribute| matches!(
            attribute,
            Attribute::SourceFile(index) if utf8_name(pool, *index) == "NestedSample.java"
        )),
        "expected a SourceFile attribute"
    );
}

#[test]
fn decodes_nested_sample_generic_method_attributes() {
    let class_file = decode(NESTED_SAMPLE);
    let pool = &class_file.constant_pool;
    let max_method = find_method(&class_file, "max");

    assert!(
        max_method.attributes.iter().any(|attribute| matches!(
            attribute,
            Attribute::Exceptions(exceptions)
                if exceptions.len() == 1
                    && class_name(pool, exceptions[0]) == "java/lang/IllegalStateException"
        )),
        "expected max() to declare throws IllegalStateException"
    );
    assert!(
        max_method
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::Signature(_))),
        "expected max() to carry a Signature attribute"
    );

    let parameter_names: Vec<&str> = max_method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::MethodParameters(parameters) => Some(
                parameters
                    .iter()
                    .map(|parameter| {
                        utf8_name(pool, parameter.name_index.expect("named parameter"))
                    })
                    .collect(),
            ),
            _ => None,
        })
        .expect("expected a MethodParameters attribute");
    assert_eq!(parameter_names, vec!["a", "b"]);
}

#[test]
fn decodes_nested_sample_inner_deprecated_and_nest_host() {
    let class_file = decode(NESTED_SAMPLE_INNER);
    let pool = &class_file.constant_pool;

    assert!(class_file.attributes.contains(&Attribute::Deprecated));

    let nest_host = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::NestHost(index) => Some(*index),
            _ => None,
        })
        .expect("expected a NestHost attribute");
    assert_eq!(class_name(pool, nest_host), "NestedSample");
}

#[test]
fn decodes_nested_sample_local_runnable_enclosing_method() {
    let class_file = decode(NESTED_SAMPLE_LOCAL_RUNNABLE);
    let pool = &class_file.constant_pool;

    let (class_index, method_index) = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::EnclosingMethod {
                class_index,
                method_index,
            } => Some((*class_index, *method_index)),
            _ => None,
        })
        .expect("expected an EnclosingMethod attribute");

    assert_eq!(class_name(pool, class_index), "NestedSample");
    let method_index = method_index.expect("expected an enclosing method, not just a class");
    match pool.get(method_index) {
        Some(ConstantPoolEntry::NameAndType { name_index, .. }) => {
            assert_eq!(utf8_name(pool, *name_index), "makeLocalRunnable");
        }
        other => panic!("expected NameAndType at {method_index:?}, got {other:?}"),
    }
}

#[test]
fn decodes_shape_permitted_subclasses() {
    let class_file = decode(SHAPE);
    let pool = &class_file.constant_pool;

    let permitted = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::PermittedSubclasses(classes) => Some(classes.clone()),
            _ => None,
        })
        .expect("expected a PermittedSubclasses attribute");

    let mut names: Vec<&str> = permitted
        .iter()
        .map(|index| class_name(pool, *index))
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["Shape$Circle", "Shape$Square"]);
}

#[test]
fn decodes_shape_circle_record_component() {
    let class_file = decode(SHAPE_CIRCLE);
    let pool = &class_file.constant_pool;

    let components = class_file
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Record(components) => Some(components),
            _ => None,
        })
        .expect("expected a Record attribute");

    assert_eq!(components.len(), 1);
    assert_eq!(utf8_name(pool, components[0].name_index), "radius");
    assert_eq!(utf8_name(pool, components[0].descriptor_index), "D");
}

#[test]
fn resolves_pool_sample_field_types() {
    let class_file = decode(POOL_SAMPLE);
    let pool = &class_file.constant_pool;

    assert_eq!(
        find_field(&class_file, "ANSWER").field_type(pool),
        Ok(FieldType::Int)
    );
    assert_eq!(
        find_field(&class_file, "BIG_ANSWER").field_type(pool),
        Ok(FieldType::Long)
    );
    assert_eq!(
        find_field(&class_file, "HALF").field_type(pool),
        Ok(FieldType::Float)
    );
    assert_eq!(
        find_field(&class_file, "PI").field_type(pool),
        Ok(FieldType::Double)
    );
    assert_eq!(
        find_field(&class_file, "GREETING").field_type(pool),
        Ok(FieldType::Object("java/lang/String".to_owned()))
    );
}

#[test]
fn resolves_method_descriptors_across_fixtures() {
    let pool_sample = decode(POOL_SAMPLE);
    let compute_answer = find_method(&pool_sample, "computeAnswer");
    assert_eq!(
        compute_answer.descriptor(&pool_sample.constant_pool),
        Ok(MethodDescriptor {
            parameters: vec![],
            return_type: Some(FieldType::Int),
        })
    );

    let shape_circle = decode(SHAPE_CIRCLE);
    let radius = find_method(&shape_circle, "radius");
    assert_eq!(
        radius.descriptor(&shape_circle.constant_pool),
        Ok(MethodDescriptor {
            parameters: vec![],
            return_type: Some(FieldType::Double),
        })
    );
}

#[test]
fn validates_references_across_every_real_fixture() {
    for bytes in [
        POOL_SAMPLE,
        NESTED_SAMPLE,
        NESTED_SAMPLE_INNER,
        NESTED_SAMPLE_LOCAL_RUNNABLE,
        SHAPE,
        SHAPE_CIRCLE,
    ] {
        let class_file = decode(bytes);
        assert_eq!(class_file.validate_references(), Ok(()));
    }
}

#[test]
fn resolves_and_parses_the_real_max_signature() {
    let class_file = decode(NESTED_SAMPLE);
    let pool = &class_file.constant_pool;
    let max_method = find_method(&class_file, "max");

    let signature_index = max_method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Signature(index) => Some(*index),
            _ => None,
        })
        .expect("expected a Signature attribute");

    let signature = MethodSignature::parse(utf8_name(pool, signature_index)).unwrap();

    assert_eq!(signature.type_parameters.len(), 1);
    assert_eq!(signature.type_parameters[0].name, "T");
    assert_eq!(signature.type_parameters[0].class_bound, None);
    assert_eq!(signature.type_parameters[0].interface_bounds.len(), 1);
    assert_eq!(
        signature.parameters,
        vec![
            TypeSignature::Reference(ReferenceTypeSignature::TypeVariable("T".to_owned())),
            TypeSignature::Reference(ReferenceTypeSignature::TypeVariable("T".to_owned())),
        ]
    );
    assert_eq!(
        signature.result,
        Some(TypeSignature::Reference(
            ReferenceTypeSignature::TypeVariable("T".to_owned())
        ))
    );
    assert_eq!(signature.throws, vec![]);
}
