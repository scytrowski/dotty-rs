//! Reading names out of a TASTy name table.
//!
//! In Scala 3.9.0 compiler output, name references — in AST payloads and
//! inside composite name entries alike — are zero-based indexes into the name
//! table, which is what `dotty-tasty`'s `NameRef` means. For instance the
//! constructor name is entry 14 and the signed constructor entry says
//! `original: 14`.
//!
//! This module reads the table itself instead of using
//! `TastyFile::render_name` because it spells names differently: a signed name
//! reads as its original name (`render_name` reports it as unsupported), and
//! composite nesting is bounded.

use dotty_tasty::tasty::{NameTable, RawName};

use crate::error::UnpickleError;

/// The separators of the unique-name kinds Scala 3.9.0 registers
/// (`NameKinds.uniqueNameKinds`, plus `$jsclass` from `ExplicitJSClasses`).
/// Dotty's reader looks the separator up in that map and throws for any other,
/// so an unregistered separator is not a name it reads.
const UNIQUE_SEPARATORS: &[&str] = &[
    "$",
    "evidence$",
    "contextual$",
    "canThrow$",
    "try$",
    "ev$",
    "(param)",
    "$_lazy_implicit_$",
    "$lzy",
    "$lzyINIT",
    "$OFFSET",
    "bitmap$",
    "nonLocalReturnKey",
    "_$",
    "tailLabel",
    "$tailLocal",
    "$tmp",
    "ex",
    "ex$",
    "?",
    "'s",
    "$superArg$",
    "$doc",
    "$i",
    "$scrutinee",
    "$proxy",
    "$macro$",
    "$extension",
    "x",
    "matchAlts",
    "matchResult",
    "$given",
    "ilo",
    "boundary",
    "$jsclass",
];

/// Composite names nest a handful of levels at most; anything deeper is a
/// malformed (or cyclic) table rather than a real name.
const MAX_NAME_DEPTH: usize = 64;

/// The text of a string constant. `STRINGconst` carries a `NameRef`, and Dotty
/// reads it as `readName().toString`, so any valid name entry is a string: a
/// plain UTF-8 entry is its text, and a derived entry (qualified, expanded,
/// unique, ...) is its rendered spelling.
///
/// Unlike [`wire_name`], which spells a member for lookup and drops a
/// signature, this is the *value* of the name, so a signed entry (directly, or
/// nested in a derived one) renders as Dotty's `SignedName.mkString` does:
/// `f[with sig Signature(List(A, 1),R)]`. `Signature` is a case class without
/// its own `toString`, so its parts print as `List` and `String` do: term
/// parameters by name, type-parameter sections by their length, then the
/// result, with no space before the result.
pub(crate) fn string_value(names: &NameTable, reference: u32) -> Result<String, UnpickleError> {
    resolve(names, reference, reference, 0, Spelling::Value)
}

/// What a rendered name is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// A member name for lookup: a signature is dropped.
    Member,
    /// The value of the name, as Dotty's `Name.toString`: signatures show.
    Value,
}

/// Renders the name at the zero-based wire `reference` to its Scala spelling.
///
/// A signed name (a method name carrying its erased signature) reads as its
/// original name: the signature is not part of the name, and overloads are
/// told apart by definition address, not by signature text.
pub(crate) fn wire_name(names: &NameTable, reference: u32) -> Result<String, UnpickleError> {
    resolve(names, reference, reference, 0, Spelling::Member)
}

/// Whether the name at `reference` is a signed name (`SIGNED` or
/// `TARGETSIGNED`), which carries an erased signature that [`wire_name`] drops.
pub(crate) fn is_signed(names: &NameTable, reference: u32) -> bool {
    matches!(
        usize::try_from(reference)
            .ok()
            .and_then(|index| names.entries().get(index)),
        Some(RawName::Signed { .. } | RawName::TargetSigned { .. })
    )
}

/// Splits a possibly qualified name into its segments, outermost first.
///
/// `me.cytrowski.semantic` becomes `["me", "cytrowski", "semantic"]`. Splitting
/// the structure rather than the rendered text keeps a segment that itself
/// contains a dot intact.
pub(crate) fn qualified_segments(
    names: &NameTable,
    reference: u32,
) -> Result<Vec<String>, UnpickleError> {
    let mut segments = Vec::new();
    collect_segments(names, reference, reference, 0, &mut segments)?;
    Ok(segments)
}

/// The segments of a package path, outermost first, in the session's package
/// model (`dotty_core::Packages`), where the root package is the empty path.
///
/// Dotty spells the root package `<root>` and the default (empty) package
/// `<empty>`, the latter being what a unit with no `package` clause is written
/// against. The core has one root that is also the unnamed package, so a
/// leading `<root>` or `<empty>` segment is dropped, and a bare empty name is
/// the root too: `<empty>` and `""` are `[]`, `<root>.scala` is `["scala"]`.
pub(crate) fn package_segments(
    names: &NameTable,
    reference: u32,
) -> Result<Vec<String>, UnpickleError> {
    let mut segments = qualified_segments(names, reference)?;
    if segments
        .first()
        .is_some_and(|first| first == ROOT_PACKAGE || first == EMPTY_PACKAGE)
    {
        segments.remove(0);
    }
    if segments == [""] {
        segments.clear();
    }
    Ok(segments)
}

const ROOT_PACKAGE: &str = "<root>";
const EMPTY_PACKAGE: &str = "<empty>";

fn collect_segments(
    names: &NameTable,
    root: u32,
    reference: u32,
    depth: usize,
    segments: &mut Vec<String>,
) -> Result<(), UnpickleError> {
    if depth > MAX_NAME_DEPTH {
        return Err(UnpickleError::InvalidNameReference { reference: root });
    }
    let entry = usize::try_from(reference)
        .ok()
        .and_then(|index| names.entries().get(index));
    match entry {
        Some(RawName::Qualified { prefix, selector }) => {
            collect_segments(names, root, *prefix, depth + 1, segments)?;
            segments.push(wire_name(names, *selector)?);
        }
        _ => segments.push(wire_name(names, reference)?),
    }
    Ok(())
}

fn resolve(
    names: &NameTable,
    root: u32,
    reference: u32,
    depth: usize,
    spelling: Spelling,
) -> Result<String, UnpickleError> {
    if depth > MAX_NAME_DEPTH {
        return Err(UnpickleError::InvalidNameReference { reference: root });
    }
    let entry = usize::try_from(reference)
        .ok()
        .and_then(|index| names.entries().get(index))
        .ok_or(UnpickleError::InvalidNameReference { reference })?;
    let inner = |reference: u32| resolve(names, root, reference, depth + 1, spelling);

    Ok(match entry {
        RawName::Utf8(text) => text.clone(),
        RawName::Qualified { prefix, selector } => {
            format!("{}.{}", inner(*prefix)?, inner(*selector)?)
        }
        RawName::Expanded { prefix, selector } => {
            format!("{}$${}", inner(*prefix)?, inner(*selector)?)
        }
        RawName::ExpandPrefix { prefix, selector } => {
            format!("{}${}", inner(*prefix)?, inner(*selector)?)
        }
        RawName::Unique {
            separator,
            uniqid,
            underlying,
        } => {
            let separator = inner(*separator)?;
            if !UNIQUE_SEPARATORS.contains(&separator.as_str()) {
                return Err(UnpickleError::UnsupportedName { reference });
            }
            let underlying = match underlying {
                Some(underlying) => inner(*underlying)?,
                None => String::new(),
            };
            // Dotty's `UniqueNameKind.mkString`, with the two kinds that
            // override it: `$` names an anonymous number `$3$`, and
            // `contextual$` keeps a non-empty original as it is.
            let sanitized = underlying.replace(['<', '>'], "$");
            if underlying.is_empty() && separator == "$" {
                format!("${uniqid}$")
            } else if separator == "contextual$" && !underlying.is_empty() {
                sanitized
            } else {
                format!("{sanitized}{separator}{uniqid}")
            }
        }
        RawName::DefaultGetter { underlying, index } => {
            format!("{}$default${}", inner(*underlying)?, u64::from(*index) + 1)
        }
        RawName::SuperAccessor { underlying } => format!("super${}", inner(*underlying)?),
        RawName::InlineAccessor { underlying } => format!("inline${}", inner(*underlying)?),
        RawName::ObjectClass { underlying } => format!("{}$", inner(*underlying)?),
        RawName::BodyRetainer { underlying } => {
            format!("{}$retainedBody", inner(*underlying)?)
        }
        RawName::Signed {
            original,
            result_signature,
            parameter_signatures,
        }
        | RawName::TargetSigned {
            original,
            result_signature,
            parameter_signatures,
            ..
        } => {
            let original = inner(*original)?;
            if spelling == Spelling::Member {
                return Ok(original);
            }
            // Dotty's `readParamSig`: a negative value is the length of a
            // type-parameter section, anything else a name reference.
            let mut parameters = Vec::with_capacity(parameter_signatures.len());
            for signature in parameter_signatures {
                parameters.push(match u32::try_from(*signature) {
                    Ok(reference) => inner(reference)?,
                    Err(_) => signature
                        .checked_abs()
                        .ok_or(UnpickleError::InvalidNameReference { reference })?
                        .to_string(),
                });
            }
            format!(
                "{original}[with sig Signature(List({}),{})]",
                parameters.join(", "),
                inner(*result_signature)?
            )
        }
        RawName::Unknown { .. } => {
            return Err(UnpickleError::UnsupportedName { reference });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entry 0 is a placeholder standing for the leading section name every
    /// real table has (`ASTs`), so that the references in these tables match
    /// the positions they have in a real file.
    fn table(entries: Vec<RawName>) -> NameTable {
        let mut all = vec![RawName::Utf8("ASTs".to_owned())];
        all.extend(entries);
        NameTable::from_entries(all).unwrap()
    }

    fn utf8(text: &str) -> RawName {
        RawName::Utf8(text.to_owned())
    }

    #[test]
    fn a_string_value_is_any_valid_name_rendered_as_dotty_does() {
        let names = table(vec![
            utf8("a.b"),
            utf8("b"),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ]);

        // A plain entry is its text, not split.
        assert_eq!(string_value(&names, 1).unwrap(), "a.b");
        // A derived entry is its rendered name.
        assert_eq!(string_value(&names, 3).unwrap(), "a.b.b");
        assert_eq!(
            string_value(&names, 9),
            Err(UnpickleError::InvalidNameReference { reference: 9 })
        );
    }

    #[test]
    fn a_plain_name_is_its_text() {
        let names = table(vec![utf8("Foo")]);

        assert_eq!(wire_name(&names, 1).unwrap(), "Foo");
    }

    #[test]
    fn references_are_zero_based() {
        let names = table(vec![utf8("Foo")]);

        assert_eq!(wire_name(&names, 0).unwrap(), "ASTs");
    }

    #[test]
    fn a_qualified_name_joins_prefix_and_selector_with_a_dot() {
        let names = table(vec![
            utf8("java"),
            utf8("lang"),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ]);

        assert_eq!(wire_name(&names, 3).unwrap(), "java.lang");
    }

    #[test]
    fn nested_qualified_names_render_the_whole_path() {
        let names = table(vec![
            utf8("me"),
            utf8("cytrowski"),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
            utf8("semantic"),
            RawName::Qualified {
                prefix: 3,
                selector: 4,
            },
        ]);

        assert_eq!(wire_name(&names, 5).unwrap(), "me.cytrowski.semantic");
    }

    #[test]
    fn qualified_segments_split_a_path_outermost_first() {
        let names = table(vec![
            utf8("me"),
            utf8("cytrowski"),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
            utf8("semantic"),
            RawName::Qualified {
                prefix: 3,
                selector: 4,
            },
        ]);

        assert_eq!(
            qualified_segments(&names, 5).unwrap(),
            ["me", "cytrowski", "semantic"]
        );
    }

    #[test]
    fn qualified_segments_of_a_simple_name_is_that_name() {
        let names = table(vec![utf8("scala")]);

        assert_eq!(qualified_segments(&names, 1).unwrap(), ["scala"]);
    }

    #[test]
    fn qualified_segments_keep_a_segment_containing_a_dot_whole() {
        let names = table(vec![
            utf8("a"),
            utf8("b.c"),
            RawName::Qualified {
                prefix: 1,
                selector: 2,
            },
        ]);

        assert_eq!(qualified_segments(&names, 3).unwrap(), ["a", "b.c"]);
    }

    #[test]
    fn qualified_segments_of_a_missing_reference_is_an_invalid_reference() {
        let names = table(vec![utf8("a")]);

        assert_eq!(
            qualified_segments(&names, 9),
            Err(UnpickleError::InvalidNameReference { reference: 9 })
        );
    }

    #[test]
    fn an_object_class_name_gets_a_dollar_suffix() {
        let names = table(vec![utf8("O"), RawName::ObjectClass { underlying: 1 }]);

        assert_eq!(wire_name(&names, 2).unwrap(), "O$");
    }

    #[test]
    fn a_signed_name_reads_as_its_original_name() {
        let names = table(vec![
            utf8("<init>"),
            utf8("Unit"),
            RawName::Signed {
                original: 1,
                result_signature: 2,
                parameter_signatures: Vec::new(),
            },
        ]);

        assert_eq!(wire_name(&names, 3).unwrap(), "<init>");
    }

    #[test]
    fn a_unique_name_sanitizes_its_original_like_dotty() {
        let names = table(vec![
            utf8("$"),
            utf8("<init>"),
            RawName::Unique {
                separator: 1,
                uniqid: 2,
                underlying: Some(2),
            },
            utf8("contextual$"),
            RawName::Unique {
                separator: 4,
                uniqid: 1,
                underlying: Some(2),
            },
            RawName::Unique {
                separator: 4,
                uniqid: 1,
                underlying: None,
            },
        ]);

        // `str.sanitize` turns `<` and `>` into `$` before the separator.
        assert_eq!(wire_name(&names, 3).unwrap(), "$init$$2");
        // `contextual$` keeps a non-empty original alone, else `contextual$1`.
        assert_eq!(wire_name(&names, 5).unwrap(), "$init$");
        assert_eq!(wire_name(&names, 6).unwrap(), "contextual$1");
    }

    #[test]
    fn a_unique_name_with_an_unregistered_separator_is_unsupported() {
        // Dotty looks the separator up in `uniqueNameKinds` and throws.
        let names = table(vec![
            utf8("#"),
            utf8("a"),
            RawName::Unique {
                separator: 1,
                uniqid: 1,
                underlying: Some(2),
            },
        ]);

        assert_eq!(
            wire_name(&names, 3),
            Err(UnpickleError::UnsupportedName { reference: 3 })
        );
    }

    /// Entries: 1 `f`, 2 `Unit`, 3 `Int`, 4 `g`, then 5 a signed name with a
    /// term parameter and a two-long type-parameter section, 6 a target-signed
    /// name without parameters, 7 a qualified name whose prefix is entry 5.
    fn signed_names() -> NameTable {
        table(vec![
            utf8("f"),
            utf8("Unit"),
            utf8("Int"),
            utf8("g"),
            RawName::Signed {
                original: 1,
                result_signature: 2,
                parameter_signatures: vec![3, -2],
            },
            RawName::TargetSigned {
                original: 1,
                target: 4,
                result_signature: 2,
                parameter_signatures: Vec::new(),
            },
            RawName::Qualified {
                prefix: 5,
                selector: 4,
            },
        ])
    }

    #[test]
    fn a_signed_string_value_shows_its_signature_like_dotty() {
        let names = signed_names();

        // `SignedName.mkString`: `$underlying[with sig $sig]`, and the
        // case-class `Signature` prints as `Signature(List(..),result)`.
        assert_eq!(
            string_value(&names, 5).unwrap(),
            "f[with sig Signature(List(Int, 2),Unit)]"
        );
        assert_eq!(
            string_value(&names, 6).unwrap(),
            "f[with sig Signature(List(),Unit)]"
        );
    }

    #[test]
    fn a_signed_name_nested_in_a_derived_name_shows_its_signature() {
        let names = signed_names();

        assert_eq!(
            string_value(&names, 7).unwrap(),
            "f[with sig Signature(List(Int, 2),Unit)].g"
        );
        // The member spelling of the same entries still drops it.
        assert_eq!(wire_name(&names, 5).unwrap(), "f");
        assert_eq!(wire_name(&names, 7).unwrap(), "f.g");
    }

    #[test]
    fn a_target_signed_name_reads_as_its_original_name() {
        let names = table(vec![
            utf8("f"),
            utf8("Unit"),
            utf8("g"),
            RawName::TargetSigned {
                original: 1,
                target: 3,
                result_signature: 2,
                parameter_signatures: Vec::new(),
            },
        ]);

        assert_eq!(wire_name(&names, 4).unwrap(), "f");
    }

    #[test]
    fn a_default_getter_name_counts_parameters_from_one() {
        let names = table(vec![
            utf8("f"),
            RawName::DefaultGetter {
                underlying: 1,
                index: 0,
            },
        ]);

        assert_eq!(wire_name(&names, 2).unwrap(), "f$default$1");
    }

    #[test]
    fn a_unique_name_appends_separator_and_id_to_its_underlying_name() {
        let names = table(vec![
            utf8("x"),
            utf8("$"),
            RawName::Unique {
                separator: 2,
                uniqid: 1,
                underlying: Some(1),
            },
        ]);

        assert_eq!(wire_name(&names, 3).unwrap(), "x$1");
    }

    #[test]
    fn expanded_and_expand_prefix_names_use_their_own_separators() {
        let names = table(vec![
            utf8("Outer"),
            utf8("x"),
            RawName::Expanded {
                prefix: 1,
                selector: 2,
            },
            RawName::ExpandPrefix {
                prefix: 1,
                selector: 2,
            },
        ]);

        assert_eq!(wire_name(&names, 3).unwrap(), "Outer$$x");
        assert_eq!(wire_name(&names, 4).unwrap(), "Outer$x");
    }

    #[test]
    fn accessor_and_retainer_names_keep_their_conventional_prefixes() {
        let names = table(vec![
            utf8("m"),
            RawName::SuperAccessor { underlying: 1 },
            RawName::InlineAccessor { underlying: 1 },
            RawName::BodyRetainer { underlying: 1 },
        ]);

        assert_eq!(wire_name(&names, 2).unwrap(), "super$m");
        assert_eq!(wire_name(&names, 3).unwrap(), "inline$m");
        assert_eq!(wire_name(&names, 4).unwrap(), "m$retainedBody");
    }

    #[test]
    fn a_reference_past_the_table_is_an_invalid_reference() {
        let names = table(vec![utf8("Foo")]);

        assert_eq!(
            wire_name(&names, 9),
            Err(UnpickleError::InvalidNameReference { reference: 9 })
        );
    }

    #[test]
    fn an_unknown_name_entry_is_unsupported_not_guessed() {
        let names = table(vec![RawName::Unknown {
            tag: 200,
            payload: Vec::new(),
        }]);

        assert_eq!(
            wire_name(&names, 1),
            Err(UnpickleError::UnsupportedName { reference: 1 })
        );
    }

    #[test]
    fn a_name_nested_deeper_than_the_depth_limit_is_rejected() {
        // `dotty-tasty` rejects a cyclic table, but a long acyclic chain is a
        // valid table that no real name is deep enough to need.
        let mut entries = vec![utf8("Base")];
        for depth in 0..MAX_NAME_DEPTH + 10 {
            entries.push(RawName::ObjectClass {
                underlying: depth as u32 + 1,
            });
        }
        let top = entries.len() as u32;
        let names = table(entries);

        assert_eq!(
            wire_name(&names, top),
            Err(UnpickleError::InvalidNameReference { reference: top })
        );
    }

    #[test]
    fn the_fixture_package_and_class_names_resolve_from_the_real_table() {
        use dotty_tasty::tasty::{PACKAGE_TAG, TYPEDEF_TAG, TastyFile};

        let bytes = include_bytes!("../tests/fixtures/semantic/Foo.tasty");
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let package = file
            .asts()
            .unwrap()
            .iter()
            .find(|node| node.tag == PACKAGE_TAG)
            .unwrap()
            .decode_package()
            .unwrap();
        let class = package
            .stats
            .iter()
            .find(|node| node.tag == TYPEDEF_TAG)
            .unwrap()
            .decode_definition()
            .unwrap();

        assert_eq!(
            wire_name(file.names(), package.path_name().unwrap()).unwrap(),
            "me.cytrowski.tastyfixtures.semantic"
        );
        assert_eq!(wire_name(file.names(), class.name()).unwrap(), "Foo");
    }

    #[test]
    fn the_default_and_root_packages_are_the_empty_path() {
        let names = table(vec![
            utf8("<empty>"),
            utf8("<root>"),
            utf8("scala"),
            RawName::Qualified {
                prefix: 2,
                selector: 3,
            },
            utf8(""),
        ]);

        assert_eq!(package_segments(&names, 1).unwrap(), Vec::<String>::new());
        assert_eq!(package_segments(&names, 2).unwrap(), Vec::<String>::new());
        assert_eq!(package_segments(&names, 4).unwrap(), ["scala"]);
        assert_eq!(package_segments(&names, 5).unwrap(), Vec::<String>::new());
        // Only a leading segment is special.
        assert_eq!(package_segments(&names, 3).unwrap(), ["scala"]);
    }
}
