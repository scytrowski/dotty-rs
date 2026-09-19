//! A category-five node whose length is written with a leading zero group is
//! addressed at its tag byte (regression: it used to be reported one byte
//! late, so a reference to its real address named no node).

use dotty_tasty::tasty::{APPLIEDTYPE_TAG, TastyFile};

const TUPLE16: &[u8] = include_bytes!("fixtures/scala3-library/scala/Tuple16.tasty");

#[test]
fn a_node_with_a_padded_length_is_indexed_at_its_tag() {
    let file = TastyFile::parse_compatible_with(TUPLE16, 28, 9, 0).unwrap();
    let payload = file
        .section(dotty_tasty::tasty::StandardSection::Asts)
        .unwrap()
        .payload;
    // `APPLIEDtype` whose length is the two bytes `0, 253`.
    assert_eq!(payload[326..329], [APPLIEDTYPE_TAG, 0, 253]);

    let index = file.ast_address_index().unwrap();

    assert_eq!(
        index.get_node(326).map(|node| node.tag),
        Some(APPLIEDTYPE_TAG)
    );
    assert_eq!(index.get_node(327), None);
}
