//! Structural pairing of default getters and marked parameters in one template.

use std::collections::{HashMap, HashSet};

use crate::{
    AstError, DEFDEF_TAG, DefinitionTail, GIVEN_TAG, HASDEFAULT_TAG, IMPLICIT_TAG, NameTable,
    ParameterNode, RawName, RawNode,
};

/// Offsets of sibling ordinary methods in the supplied template and the
/// zero-based term parameter ordinal, flattened across the target's clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultGetterSite {
    /// Zero-based method position in the planned member sequence.
    pub method: usize,
    /// Zero-based position in the ordered input sequence.
    pub ordinal: usize,
    /// Zero-based getter position in the retained metadata.
    pub getter: usize,
    /// The marked parameter carries either GIVEN or IMPLICIT.
    pub contextual: bool,
}

/// Matches Scala's structural DEFAULTGETTER name to the underlying method's
/// HASDEFAULT parameter. Supply all direct DEFDEF nodes of one template in
/// wire order, with offsets relative to the same base. Duplicate getter names
/// or multiple overloads claiming the same position have no unique match.
pub fn template_default_getters<'a, 'b: 'a>(
    names: &NameTable,
    definitions: impl IntoIterator<Item = &'a RawNode<'b>>,
) -> Result<Vec<DefaultGetterSite>, AstError> {
    let mut methods = Vec::new();
    let mut getters = Vec::new();
    let mut wanted = HashSet::new();
    let mut duplicate_getters = HashSet::new();
    for node in definitions {
        if node.tag != DEFDEF_TAG {
            continue;
        }
        let name = node.decode_definition()?.name();
        match names.get(name) {
            Some(RawName::DefaultGetter { underlying, index }) => {
                let Some(base) = names.get_utf8(*underlying) else {
                    continue;
                };
                if matches!(base, "<init>" | "$init$") {
                    continue;
                }
                let key = (base, *index as usize);
                if !wanted.insert(key) {
                    duplicate_getters.insert(key);
                }
                getters.push((node.offset, key, *underlying));
            }
            Some(RawName::Utf8(text)) => methods.push((text.as_str(), name, node)),
            _ => {}
        }
    }
    if getters.is_empty() {
        return Ok(Vec::new());
    }

    let bases: HashSet<_> = wanted.iter().map(|(base, _)| *base).collect();
    let ordinary_names: HashSet<_> = methods.iter().map(|(text, _, _)| *text).collect();
    let mut targets = HashMap::new();
    for (base, name, node) in methods {
        if !bases.contains(base) {
            continue;
        }
        let header = node.decode_defdef_body()?;
        for (ordinal, param) in header
            .parameters()
            .filter(|p| matches!(p, ParameterNode::TermParam { .. }))
            .enumerate()
        {
            let key = (base, ordinal);
            if !wanted.contains(&key) {
                continue;
            }
            let tail = param.decode_body()?.tail;
            if tail.contains(&DefinitionTail::Modifier(HASDEFAULT_TAG)) {
                let contextual = tail
                    .iter()
                    .any(|item| matches!(item, DefinitionTail::Modifier(GIVEN_TAG | IMPLICIT_TAG)));
                targets
                    .entry(key)
                    .and_modify(|slot| *slot = None)
                    .or_insert(Some((name, node.offset, contextual)));
            }
        }
    }
    Ok(getters
        .into_iter()
        .filter_map(|(getter, key, underlying)| {
            let (name, method, contextual) = (*targets.get(&key)?)?;
            // A textual collision suffices to make two symbols indistinguishable.
            // A positive match also needs the exact underlying name-table entry.
            if name != underlying
                || duplicate_getters.contains(&key)
                || ordinary_names
                    .contains(format!("{}$default${}", key.0, key.1 as u64 + 1).as_str())
            {
                return None;
            }
            Some(DefaultGetterSite {
                method,
                ordinal: key.1,
                getter,
                contextual,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PARAM_TAG, SPLITCLAUSE_TAG, TYPEPARAM_TAG, UNITCONST_TAG, Writer};

    // Name references 0=f, 1=structural f[DefaultGetter 0],
    // 2=plain f$default$1, 3=structural f[DefaultGetter 1], 4=x.
    fn names() -> NameTable {
        NameTable::from_entries(vec![
            RawName::Utf8("f".into()),
            RawName::DefaultGetter {
                underlying: 0,
                index: 0,
            },
            RawName::Utf8("f$default$1".into()),
            RawName::DefaultGetter {
                underlying: 0,
                index: 1,
            },
            RawName::Utf8("x".into()),
            RawName::Utf8("f".into()),
        ])
        .unwrap()
    }

    fn method(name: u32, parameters: &[(u8, bool)]) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_nat(name).unwrap();
        for &(tag, has_default) in parameters {
            writer.write_u8(tag);
            if tag == SPLITCLAUSE_TAG {
                continue;
            }
            let mut body = Writer::new();
            body.write_nat(4).unwrap();
            body.write_u8(UNITCONST_TAG);
            if has_default {
                body.write_u8(HASDEFAULT_TAG);
            }
            writer
                .write_length_prefixed_bytes(&body.into_inner())
                .unwrap();
        }
        writer.write_u8(UNITCONST_TAG);
        writer.into_inner()
    }

    fn sites(definitions: &[Vec<u8>]) -> Vec<DefaultGetterSite> {
        let mut at = 0;
        let nodes: Vec<_> = definitions
            .iter()
            .map(|payload| {
                let node = RawNode {
                    tag: DEFDEF_TAG,
                    offset: at,
                    payload,
                };
                at += payload.len() + 2;
                node
            })
            .collect();
        template_default_getters(&names(), &nodes).unwrap()
    }

    #[test]
    fn default_getter_needs_both_the_structural_name_and_the_parameter_marker() {
        let marked = method(0, &[(PARAM_TAG, true)]);
        let unmarked = method(0, &[(PARAM_TAG, false)]);
        let plain = method(2, &[]);
        let derived = method(1, &[]);
        assert!(sites(&[marked.clone(), plain.clone()]).is_empty());
        let different_name_entry = method(5, &[(PARAM_TAG, true)]);
        assert!(sites(&[different_name_entry.clone(), derived.clone()]).is_empty());
        assert!(sites(&[different_name_entry, marked.clone(), derived.clone()]).is_empty());
        assert!(sites(&[unmarked, derived.clone()]).is_empty());
        assert!(sites(&[marked.clone(), plain, derived.clone()]).is_empty());
        assert!(sites(&[marked, derived.clone(), derived]).is_empty());
    }

    #[test]
    fn default_getter_distinguishes_contextual_and_implicit_parameter_modifiers() {
        for marker in [GIVEN_TAG, IMPLICIT_TAG] {
            let mut payload = Writer::new();
            payload.write_nat(0).unwrap();
            payload.write_u8(PARAM_TAG);
            let mut parameter = Writer::new();
            parameter.write_nat(4).unwrap();
            parameter.write_u8(UNITCONST_TAG);
            parameter.write_u8(marker);
            parameter.write_u8(HASDEFAULT_TAG);
            payload
                .write_length_prefixed_bytes(&parameter.into_inner())
                .unwrap();
            payload.write_u8(UNITCONST_TAG);
            let found = sites(&[payload.into_inner(), method(1, &[])]);
            assert!(matches!(
                found.as_slice(),
                [DefaultGetterSite {
                    ordinal: 0,
                    contextual: true,
                    ..
                }]
            ));
        }
    }

    #[test]
    fn default_getter_selects_the_only_marked_overload_at_the_flattened_term_position() {
        let unmarked = method(0, &[(PARAM_TAG, false), (PARAM_TAG, false)]);
        let marked = method(
            0,
            &[
                (TYPEPARAM_TAG, true),
                (PARAM_TAG, false),
                (SPLITCLAUSE_TAG, false),
                (PARAM_TAG, true),
            ],
        );
        let getter = method(3, &[]);
        let unmarked_at = getter.len() + 2;
        let marked_at = unmarked_at + unmarked.len() + 2;
        assert_eq!(
            sites(&[getter.clone(), unmarked, marked.clone()]),
            vec![DefaultGetterSite {
                method: marked_at,
                ordinal: 1,
                getter: 0,
                contextual: false,
            }]
        );
        assert!(sites(&[getter.clone(), marked.clone(), marked.clone()]).is_empty());
        assert!(sites(&[method(1, &[]), marked]).is_empty());
    }
}
