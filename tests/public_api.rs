use dotty::tasty::{NodeCategory, SimpleTerm, TermValue, Writer};

#[test]
fn exposes_the_tasty_api_under_the_dotty_namespace() {
    assert!(NodeCategory::is_known_tag(255));

    let term = SimpleTerm::new(70, TermValue::Int(42)).unwrap();
    let mut writer = Writer::new();
    term.encode(&mut writer).unwrap();

    assert_eq!(writer.as_slice(), &[70, 0xaa]);
}
