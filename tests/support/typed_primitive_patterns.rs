use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::{PairedJvmTastyArtifacts, emit_paired_jvm_tasty_class};

const SOURCE: &str =
    include_str!("../fixtures/jvm_tasty_typed_primitive_patterns/TypedPrimitivePatterns.scala");

fn emit(methods: &str) -> PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class TypedPrimitivePatterns {{ {methods} }}"),
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn implicit_and_explicit_typed_binders_keep_pattern_wrappers() {
    use dotty_tasty::tasty::*;
    for ty in ["Int", "Boolean"] {
        for pattern in [
            format!("_: {ty}"),
            format!("n: {ty}"),
            format!("n @ (_: {ty})"),
        ] {
            let pair = emit(&format!(
                "def f(x: {ty}): Int = x match {{ case {pattern} => 7 }}"
            ));
            let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
            let index = file.ast_address_index().unwrap();
            assert_eq!(index.iter().filter(|n| n.tag == TYPED_TAG).count(), 1);
            assert_eq!(
                index.iter().filter(|n| n.tag == BIND_TAG).count(),
                usize::from(pattern.starts_with('n'))
            );
            let typed = index
                .iter()
                .find(|n| n.tag == TYPED_TAG)
                .unwrap()
                .decode_typed()
                .unwrap();
            assert!(matches!(
                typed.expression,
                RawTree::NatAst { tag: IDENT_TAG, .. }
            ));
            let RawTree::NatAst { tag, value, .. } = typed.type_tree else {
                panic!()
            };
            assert_eq!(tag, IDENTTPT_TAG);
            assert_eq!(file.render_name(value).unwrap(), ty);
        }
    }
}

#[test]
fn complete_typed_pattern_fixture_is_deterministic_and_preserves_pattern_nodes() {
    use dotty_tasty::tasty::*;
    let first = emit_paired_jvm_tasty_class(
        SOURCE,
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2480),
    )
    .unwrap();
    let second = emit_paired_jvm_tasty_class(
        SOURCE,
        "TypedPrimitivePatterns.scala",
        SourceId::from_index(2481),
    )
    .unwrap();
    assert_eq!(first.classfile_bytes(), second.classfile_bytes());
    assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    let file = TastyFile::parse_and_validate_scala_3_9(first.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    for (tag, count) in [
        (TYPED_TAG, 58),
        (BIND_TAG, 37),
        (MATCH_TAG, 47),
        (RETURN_TAG, 4),
    ] {
        assert_eq!(
            index.iter().filter(|n| n.tag == tag).count(),
            count,
            "tag {tag}"
        );
    }
}
