//! Structural inspection of independently generated, small Scala 3.9 fixtures.
//! This tooling uses the existing codec; it neither types source nor pickles semantics.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

use dotty_tasty::tasty::*;
use serde_json::{Value, json};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

fn tag_name(tag: u8) -> String {
    let name = match tag {
        PACKAGE_TAG => "PACKAGE",
        TYPEDEF_TAG => "TYPEDEF",
        TEMPLATE_TAG => "TEMPLATE",
        DEFDEF_TAG => "DEFDEF",
        PARAM_TAG => "PARAM",
        APPLY_TAG => "APPLY",
        SELECTIN_TAG => "SELECTin",
        NEW_TAG => "NEW",
        THIS_TAG => "THIS",
        TYPEREF_TAG => "TYPEREF",
        TYPEREFSYMBOL_TAG => "TYPEREFsymbol",
        TERMREFSYMBOL_TAG => "TERMREFsymbol",
        TERMREFPKG_TAG => "TERMREFpkg",
        TYPEREFPKG_TAG => "TYPEREFpkg",
        IDENT_TAG => "IDENT",
        SELECT_TAG => "SELECT",
        QUALTHIS_TAG => "QUALTHIS",
        IDENTTPT_TAG => "IDENTtpt",
        SHAREDTYPE_TAG => "SHAREDtype",
        SHAREDTERM_TAG => "SHAREDterm",
        TERMREFDIRECT_TAG => "TERMREFdirect",
        TYPEREFDIRECT_TAG => "TYPEREFdirect",
        INTCONST_TAG => "INTconst",
        STRINGCONST_TAG => "STRINGconst",
        ANNOTATION_TAG => "ANNOTATION",
        EMPTYCLAUSE_TAG => "EMPTYCLAUSE",
        SPLITCLAUSE_TAG => "SPLITCLAUSE",
        STABLE_TAG => "STABLE",
        ABSTRACT_TAG => "ABSTRACT",
        _ => return format!("tag-{tag}"),
    };
    name.into()
}

fn parse(bytes: &[u8]) -> Result<TastyFile<'_>> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("oracle input exceeds 1 MiB".into());
    }
    Ok(TastyFile::parse_and_validate_scala_3_9_with_max_ast_index_depth(bytes, 256)?)
}

struct Names<'a> {
    file: &'a TastyFile<'a>,
    sources: BTreeSet<u32>,
    fuel: Cell<usize>,
}

impl Names<'_> {
    fn name(&self, reference: u32) -> Result<Value> {
        self.at(reference, 0)
    }

    fn at(&self, reference: u32, depth: usize) -> Result<Value> {
        self.fuel.set(
            self.fuel
                .get()
                .checked_sub(1)
                .ok_or("oracle name budget exceeded")?,
        );
        if depth >= 128 {
            return Err("oracle name depth exceeds 128".into());
        }
        let entry = self.file.name(reference).ok_or("missing name")?;
        Ok(match entry {
            RawName::Utf8(text) => json!({"utf8": if self.sources.contains(&reference) {
                "<source>"
            } else { text }}),
            RawName::Qualified { prefix, selector } => json!({"qualified": [
                self.at(*prefix, depth + 1)?, self.at(*selector, depth + 1)?
            ]}),
            RawName::Signed {
                original,
                result_signature,
                parameter_signatures,
            } => {
                let parameters = parameter_signatures
                    .iter()
                    .map(
                        |p| match interpret_param_sig(*p).ok_or("invalid signature")? {
                            ParamSigValue::TypeParameterSectionLength(n) => {
                                Ok(json!({"type_parameters": n}))
                            }
                            ParamSigValue::TermParameter(r) => self.at(r, depth + 1),
                        },
                    )
                    .collect::<Result<Vec<_>>>()?;
                json!({"signed": self.at(*original, depth + 1)?,
                    "result": self.at(*result_signature, depth + 1)?, "parameters": parameters})
            }
            _ => return Err(format!("name kind outside oracle subset: {:?}", entry.kind()).into()),
        })
    }
}

fn modifiers(tail: &[DefinitionTail<'_>]) -> Result<Vec<String>> {
    tail.iter()
        .map(|t| match t {
            DefinitionTail::Modifier(tag) => Ok(tag_name(*tag)),
            DefinitionTail::Annotation(_) => Ok("ANNOTATION".into()),
            DefinitionTail::QualifiedModifier(_) => {
                Err("qualified modifier outside oracle subset".into())
            }
        })
        .collect()
}

fn is_sourcefile_annotation(file: &TastyFile<'_>, raw: &RawNode<'_>) -> Result<bool> {
    let annotation = raw.decode_annotation()?;
    let RawTree::NatAst {
        tag: TYPEREF_TAG,
        value,
        child,
        ..
    } = annotation.tycon
    else {
        return Ok(false);
    };
    let RawTree::Leaf(SimpleTerm {
        tag: TERMREFPKG_TAG,
        value: TermValue::NameRef(package),
        ..
    }) = *child
    else {
        return Ok(false);
    };
    Ok(file.render_name(value)? == "SourceFile"
        && file.render_name(package)? == "scala.annotation.internal")
}

/// Validate and normalize a fixture. Labels replace byte addresses, but retain
/// every target identity, structural edge, declaration order and parameter order.
pub fn inspect(bytes: &[u8], raw_addresses: bool) -> Result<Value> {
    let file = parse(bytes)?;
    let mut sources = BTreeSet::new();
    let mut attributes = Vec::new();
    if let Some(section) = file.section(StandardSection::Attributes) {
        for attribute in section.decode_attributes()? {
            match attribute {
                Attribute::SourceFile(r) => {
                    sources.insert(r);
                    attributes.push("SOURCEFILEattr");
                }
                _ => return Err("attribute outside oracle subset".into()),
            }
        }
    }
    let names = Names {
        file: &file,
        sources,
        fuel: Cell::new(100_000),
    };
    let index = file.ast_address_index_with_max_depth(256)?;
    if index.iter_nodes().count() > 10_000 || file.names().len() > 10_000 {
        return Err("oracle node/name budget exceeded".into());
    }
    let labels: BTreeMap<_, _> = index
        .iter_nodes()
        .enumerate()
        .map(|(i, node)| (node.offset as u32, format!("n{i}")))
        .collect();
    let label = |address: u32| -> Result<&str> {
        labels
            .get(&address)
            .map(String::as_str)
            .ok_or_else(|| "invalid node label".into())
    };
    let asts = file
        .section(StandardSection::Asts)
        .ok_or("missing ASTs")?
        .payload;
    let mut nodes = Vec::new();
    for node in index.iter_nodes() {
        let address = node.offset as u32;
        let tree =
            RawTree::decode_with_base_offset(&mut Reader::new(&asts[node.offset..]), node.offset)?;
        let children = index
            .children_of(address)
            .map(|n| label(n.offset as u32))
            .collect::<Result<Vec<_>>>()?;
        let parent = index.parent_of(address);
        let mut owner = parent;
        while let Some(found) = owner {
            if matches!(found.tag, PACKAGE_TAG | TYPEDEF_TAG | DEFDEF_TAG) {
                break;
            }
            owner = index.parent_of(found.offset as u32);
        }
        let mut row = json!({"id": label(address)?, "tag": tag_name(node.tag),
            "parent": parent.map(|p| label(p.offset as u32)).transpose()?,
            "owner": owner.map(|p| label(p.offset as u32)).transpose()?, "children": children});
        if raw_addresses {
            row["wire_address"] = json!(address);
        }
        let mut reference = None;
        match tree {
            RawTree::Leaf(term) => match term.value {
                TermValue::NameRef(r) => {
                    if node.tag == STRINGCONST_TAG && names.sources.contains(&r) {
                        let mut ancestor = parent;
                        let mut provenance = false;
                        while let Some(found) = ancestor {
                            if found.tag == ANNOTATION_TAG {
                                provenance = is_sourcefile_annotation(
                                    &file,
                                    index.get(found.offset as u32).ok_or("missing annotation")?,
                                )?;
                                break;
                            }
                            ancestor = index.parent_of(found.offset as u32);
                        }
                        row["name"] = if provenance {
                            names.name(r)?
                        } else {
                            json!({"utf8": file.names().get_utf8(r).ok_or("non-UTF8 string literal")?})
                        };
                    } else {
                        row["name"] = names.name(r)?;
                    }
                }
                TermValue::AstRef(r) => reference = Some(r),
                TermValue::Int(n) => row["value"] = json!(n),
                TermValue::Tag => (),
                other => return Err(format!("leaf outside oracle subset: {other:?}").into()),
            },
            RawTree::NatAst { tag, value, .. } => {
                if matches!(tag, TYPEREFSYMBOL_TAG | TERMREFSYMBOL_TAG) {
                    reference = Some(value);
                } else {
                    row["name"] = names.name(value)?;
                }
            }
            RawTree::Ast { .. } => (),
            RawTree::LengthNode(raw) => match raw.decode_structured()? {
                StructuredNode::DefDef(body) => {
                    row["name"] = names.name(body.name)?;
                    row["header"] = Value::Array(
                        body.header_items
                            .iter()
                            .map(|item| match item {
                                DefDefHeaderItem::Parameter(p) => {
                                    Ok(json!({"parameter": names.name(p.name())?}))
                                }
                                DefDefHeaderItem::Clause(tag) => {
                                    Ok(json!({"clause": tag_name(*tag)}))
                                }
                            })
                            .collect::<Result<Vec<_>>>()?,
                    );
                    row["has_rhs"] = json!(body.rhs.is_some());
                    row["modifiers"] = json!(modifiers(&body.tail)?);
                    row["role"] = json!(if file.render_name(body.name)? == "<init>" {
                        "compiler-default-constructor"
                    } else {
                        "source-method"
                    });
                }
                StructuredNode::TypeDef(DefinitionBody::TypeDef { name, tail, .. }) => {
                    row["name"] = names.name(name)?;
                    row["modifiers"] = json!(modifiers(&tail)?);
                    row["role"] = json!("source-class");
                }
                StructuredNode::Parameter(p) => {
                    row["name"] = names.name(p.name())?;
                }
                StructuredNode::SelectIn(s) => {
                    row["name"] = names.name(s.name)?;
                }
                StructuredNode::Package(_)
                | StructuredNode::Template(_)
                | StructuredNode::Apply(_)
                | StructuredNode::Annotation(_) => (),
                other => return Err(format!("node outside oracle subset: {other:?}").into()),
            },
        }
        if let Some(target) = reference {
            row["reference"] = json!({"target": label(target)?,
                "direction": if target > address { "forward" } else { "backward" }});
        }
        nodes.push(row);
    }
    let mut normalized_names = file
        .names()
        .iter()
        .map(|(r, _)| names.name(r))
        .collect::<Result<Vec<_>>>()?;
    normalized_names.sort_by_key(Value::to_string);
    let mut sections = file
        .sections()
        .iter()
        .map(|s| file.render_name(s.name))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    sections.sort();
    let position_nodes = if let Some(s) = file.section(StandardSection::Positions) {
        s.decode_positions()?
            .resolved_associations()?
            .iter()
            .map(|p| label(u32::try_from(p.address)?))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let header = file.header();
    Ok(
        json!({"schema": 1, "header": {"version": [header.major_version,
        header.minor_version, header.experimental_version], "tooling": header.tooling_version},
        "names": normalized_names, "sections": sections, "attributes": attributes,
        "position_nodes": position_nodes, "nodes": nodes}),
    )
}

/// Remove only the trailing compiler SourceFile annotation and optional
/// Positions/Comments sections from the Answer oracle. This is a derivative of
/// an existing pickle, not source semantic pickling. Retained node addresses
/// must remain unchanged: this narrow control never invents relocation logic.
pub fn reduced_answer(bytes: &[u8]) -> Result<Vec<u8>> {
    let file = parse(bytes)?;
    let roots = file.structured_asts()?;
    let [StructuredNode::Package(package)] = roots.as_slice() else {
        return Err("reduction requires one package".into());
    };
    let package_names = package.path.name_refs();
    if file.render_name(*package_names.first().ok_or("missing package name")?)? != "example"
        || package.stats.len() != 1
    {
        return Err("reduction requires example.Answer only".into());
    }
    let raw = package.stats.get(0).ok_or("missing class")?;
    let DefinitionBody::TypeDef {
        name,
        type_or_template,
        tail,
    } = raw.decode_definition_body()?
    else {
        return Err("reduction requires class TYPEDEF".into());
    };
    if file.render_name(name)? != "Answer" || tail.len() != 1 {
        return Err("reduction requires unmodified Answer annotation".into());
    }
    let DefinitionTail::Annotation(annotation) = &tail[0] else {
        return Err("reduction requires SourceFile annotation".into());
    };
    if !is_sourcefile_annotation(&file, annotation)? {
        return Err("refusing to remove a non-provenance annotation".into());
    }
    let mut class_bytes = Writer::new();
    StructuredNode::TypeDef(DefinitionBody::TypeDef {
        name,
        type_or_template,
        tail: vec![],
    })
    .encode(&mut class_bytes)?;
    let class_bytes = class_bytes.into_inner();
    let stats = RawNodes::decode(&mut Reader::new(&class_bytes))?;
    let mut ast_bytes = Writer::new();
    StructuredNode::Package(PackageNode {
        path: package.path.clone(),
        stats,
    })
    .encode(&mut ast_bytes)?;
    let mut builder = TastyFileBuilder::new(file.header().clone(), file.names().clone());
    builder.push_section(EncodedSection::raw(
        file.section(StandardSection::Asts)
            .ok_or("missing ASTs")?
            .name,
        ast_bytes.into_inner(),
    ));
    if let Some(s) = file.section(StandardSection::Attributes) {
        builder.push_section(EncodedSection::raw(s.name, s.payload));
    }
    let output = builder.encode_validated()?;
    let reduced = parse(&output)?;
    let old = file.ast_address_index()?;
    for node in reduced.ast_address_index()?.iter_nodes() {
        if old.get_node(node.offset as u32) != Some(node) {
            return Err("reduction changed a retained node address; relocation required".into());
        }
    }
    Ok(output)
}
