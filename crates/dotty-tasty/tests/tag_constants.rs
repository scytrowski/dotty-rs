//! Keeps the exported `*_TAG` constants in step with the tag tables of the
//! format reference (`docs/tasty-format-3.9.0.md`, §7.1).
//!
//! Both sides are read as text: the reference for its `number NAME` pairs, and
//! `src/ast.rs` and `src/term.rs` for their `pub const NAME_TAG: u8 = number;`
//! items. That way a
//! tag added to (or corrected in) either one without the other fails here,
//! instead of a consumer hard-coding the number (issue #15: `TYPEREFpkg`, 65).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The `NAME_TAG` constant name and value of every tag in §7.1's tables.
fn documented_tags() -> BTreeMap<String, u8> {
    let doc = read("../../docs/tasty-format-3.9.0.md");
    let start = doc
        .find("### 7.1. Complete tag list")
        .expect("the reference has a complete tag list");
    let end = doc[start..]
        .find("Unassigned values in category 5")
        .map(|offset| start + offset)
        .expect("the tag list ends before the category-five note");

    let mut tags = BTreeMap::new();
    let mut in_block = false;
    for line in doc[start..end].lines() {
        if line.starts_with("```") {
            in_block = !in_block;
            continue;
        }
        if !in_block {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for pair in tokens.chunks(2) {
            let [number, name] = pair else {
                panic!("odd token in a tag row: {line:?}");
            };
            let number: u8 = number
                .parse()
                .unwrap_or_else(|_| panic!("{number:?} is not a tag number in {line:?}"));
            let previous = tags.insert(format!("{}_TAG", name.to_uppercase()), number);
            assert_eq!(previous, None, "{name} is listed twice");
        }
    }
    tags
}

/// The `pub const NAME_TAG: u8 = number;` items of the modules that define
/// tags: `src/ast.rs` and `src/term.rs` (the category-one constants).
fn declared_tags() -> BTreeMap<String, u8> {
    let mut tags = BTreeMap::new();
    let source = format!("{}\n{}", read("src/ast.rs"), read("src/term.rs"));
    for line in source.lines() {
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": u8 = ") else {
            continue;
        };
        if !name.ends_with("_TAG") {
            continue;
        }
        let value = value.trim_end_matches(';');
        tags.insert(
            name.to_owned(),
            value.parse().unwrap_or_else(|_| panic!("{line:?}")),
        );
    }
    tags
}

#[test]
fn every_documented_tag_has_a_constant_with_the_documented_value() {
    let documented = documented_tags();
    let declared = declared_tags();

    let missing: Vec<_> = documented
        .iter()
        .filter(|(name, _)| !declared.contains_key(*name))
        .collect();
    let wrong: Vec<_> = documented
        .iter()
        .filter(|(name, value)| declared.get(*name).is_some_and(|actual| actual != *value))
        .collect();

    assert!(missing.is_empty(), "no constant for {missing:?}");
    assert!(
        wrong.is_empty(),
        "constants with the wrong value: {wrong:?}"
    );
}

#[test]
fn every_constant_is_a_documented_tag() {
    let documented = documented_tags();
    let undocumented: Vec<_> = declared_tags()
        .into_keys()
        .filter(|name| !documented.contains_key(name))
        .collect();

    assert!(
        undocumented.is_empty(),
        "constants for tags the reference does not list: {undocumented:?}"
    );
}

#[test]
fn every_constant_is_re_exported_from_the_crate_root() {
    let root = read("src/lib.rs");
    let not_exported: Vec<_> = declared_tags()
        .into_keys()
        .filter(|name| {
            !root
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|word| word == name)
        })
        .collect();

    assert!(
        not_exported.is_empty(),
        "not re-exported from lib.rs: {not_exported:?}"
    );
}

#[test]
fn typerefpkg_is_tag_65() {
    assert_eq!(dotty_tasty::tasty::TYPEREFPKG_TAG, 65);
    assert_eq!(dotty_tasty::TYPEREFPKG_TAG, 65);
}
