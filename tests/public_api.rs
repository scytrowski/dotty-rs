use dotty::tasty::{NodeCategory, SimpleTerm, TastyFile, TermValue, Writer};

#[test]
fn exposes_the_tasty_api_under_the_dotty_namespace() {
    assert!(NodeCategory::is_known_tag(255));

    let term = SimpleTerm::new(70, TermValue::Int(42)).unwrap();
    let mut writer = Writer::new();
    term.encode(&mut writer).unwrap();

    assert_eq!(writer.as_slice(), &[70, 0xaa]);
}

#[test]
fn resolves_source_files_through_the_public_tasty_facade() {
    let bytes = include_bytes!("../crates/dotty-tasty/tests/fixtures/simple_def/SimpleDef.tasty");
    let file = TastyFile::parse_scala_3_9(bytes).unwrap();

    assert_eq!(
        file.source_file(),
        Ok(Some(
            "IdeaProjects/TastyFixtures/src/main/scala/me/cytrowski/tastyfixtures/SimpleDef.scala",
        ))
    );
}
