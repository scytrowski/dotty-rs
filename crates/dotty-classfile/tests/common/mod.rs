use dotty_classfile::class_file::ClassFile;

/// Compares the modeled class structure while comparing the constant pool by
/// its encoded slots. The wire representation preserves float bit patterns,
/// including NaN payloads that `PartialEq` cannot compare.
pub fn assert_class_files_structurally_equivalent(
    expected: &ClassFile<'_>,
    actual: &ClassFile<'_>,
) {
    assert_eq!(actual.version, expected.version, "class-file version");
    assert_eq!(
        actual
            .constant_pool
            .encode()
            .expect("valid expected constant pool"),
        expected
            .constant_pool
            .encode()
            .expect("valid actual constant pool"),
        "constant-pool slots and payloads"
    );
    assert_eq!(actual.access_flags, expected.access_flags, "class flags");
    assert_eq!(actual.this_class, expected.this_class, "this_class");
    assert_eq!(actual.super_class, expected.super_class, "super_class");
    assert_eq!(actual.interfaces, expected.interfaces, "interfaces");
    assert_eq!(actual.fields.len(), expected.fields.len(), "fields count");
    for (index, (actual, expected)) in actual.fields.iter().zip(&expected.fields).enumerate() {
        assert_eq!(
            actual.access_flags, expected.access_flags,
            "field {index} flags"
        );
        assert_eq!(actual.name_index, expected.name_index, "field {index} name");
        assert_eq!(
            actual.descriptor_index, expected.descriptor_index,
            "field {index} descriptor"
        );
        assert_eq!(
            actual.attributes, expected.attributes,
            "field {index} attributes"
        );
    }
    assert_eq!(
        actual.methods.len(),
        expected.methods.len(),
        "methods count"
    );
    for (index, (actual, expected)) in actual.methods.iter().zip(&expected.methods).enumerate() {
        assert_eq!(
            actual.access_flags, expected.access_flags,
            "method {index} flags"
        );
        assert_eq!(
            actual.name_index, expected.name_index,
            "method {index} name"
        );
        assert_eq!(
            actual.descriptor_index, expected.descriptor_index,
            "method {index} descriptor"
        );
        assert_eq!(
            actual.attributes, expected.attributes,
            "method {index} attributes"
        );
    }
    assert_eq!(actual.attributes, expected.attributes, "class attributes");
}
