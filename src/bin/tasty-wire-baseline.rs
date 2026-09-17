use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use dotty::tasty::{DEFDEF_TAG, DefDefHeaderItem, TYPEDEF_TAG, TastyFile, VALDEF_TAG};

struct Declaration {
    kind: &'static str,
    name_ref: u32,
    name_kind: String,
    name: Option<String>,
    parameter_tags: Option<Vec<u8>>,
    clause_tags: Option<Vec<u8>>,
}

struct FileBaseline {
    path: String,
    tasty_format: String,
    definitions: Vec<Declaration>,
    ast_tag_counts: BTreeMap<u8, usize>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let output = arguments
        .next()
        .ok_or_else(|| usage("missing output path"))?;
    let mut selection = None;
    let mut input = None;

    for argument in arguments {
        if let Some(path) = argument.strip_prefix("--select=") {
            if selection.replace(PathBuf::from(path)).is_some() {
                return Err(usage("selection file specified more than once"));
            }
        } else if argument.starts_with('-') {
            return Err(usage("unknown option"));
        } else if input.replace(PathBuf::from(argument)).is_some() {
            return Err(usage("input path specified more than once"));
        }
    }

    let input = input.ok_or_else(|| usage("missing input path"))?;
    let (root, mut paths) = collect_paths(&input)?;
    paths.sort();
    let selected = selected_paths(&root, paths, selection.as_deref())?;
    if selected.is_empty() {
        return Err("input contains no selected .tasty files".to_owned());
    }

    let mut files = Vec::with_capacity(selected.len());
    for (logical_path, path) in selected {
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let file = TastyFile::parse(&bytes)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        files.push(extract_file(&logical_path, &file)?);
    }

    let json = encode_json(&files);
    fs::write(&output, json).map_err(|error| format!("failed to write {output}: {error}"))?;
    Ok(())
}

fn usage(message: &str) -> String {
    format!(
        "{message}\nusage: tasty-wire-baseline <output.json> [--select=selection.txt] <directory>"
    )
}

fn collect_paths(input: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
    if input.is_file() {
        if input
            .extension()
            .is_none_or(|extension| extension != "tasty")
        {
            return Err(format!(
                "input file is not a .tasty file: {}",
                input.display()
            ));
        }
        let root = input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        return Ok((root, vec![input.to_path_buf()]));
    }
    if !input.is_dir() {
        return Err(format!("input path does not exist: {}", input.display()));
    }

    let mut directories = vec![input.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to inspect an entry in {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "tasty")
            {
                paths.push(path);
            }
        }
    }
    Ok((input.to_path_buf(), paths))
}

fn selected_paths(
    root: &Path,
    paths: Vec<PathBuf>,
    selection: Option<&Path>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let logical_paths = paths
        .into_iter()
        .map(|path| {
            let logical = path
                .strip_prefix(root)
                .map_err(|error| format!("{} is outside input root: {error}", path.display()))?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            Ok((logical, path))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let Some(selection) = selection else {
        return Ok(logical_paths);
    };
    let text = fs::read_to_string(selection)
        .map_err(|error| format!("failed to read {}: {error}", selection.display()))?;
    let available = logical_paths
        .iter()
        .map(|(logical, path)| (logical.as_str(), path))
        .collect::<std::collections::BTreeMap<_, _>>();
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|logical| {
            available
                .get(logical)
                .map(|path| (logical.to_owned(), (*path).clone()))
                .ok_or_else(|| {
                    format!(
                        "selection {} names missing fixture {logical:?}",
                        selection.display()
                    )
                })
        })
        .collect()
}

fn extract_file(path: &str, file: &TastyFile<'_>) -> Result<FileBaseline, String> {
    let index = file
        .ast_address_index()
        .map_err(|error| format!("failed to index ASTs in {path}: {error}"))?;
    let mut ast_tag_counts = BTreeMap::new();
    for node in index.iter_nodes() {
        *ast_tag_counts.entry(node.tag).or_insert(0) += 1;
    }

    let definitions = index
        .iter()
        .filter(|node| matches!(node.tag, VALDEF_TAG | DEFDEF_TAG | TYPEDEF_TAG))
        .map(|node| extract_declaration(file, node))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(FileBaseline {
        path: path.to_owned(),
        tasty_format: format!(
            "{}.{}.{}",
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        definitions,
        ast_tag_counts,
    })
}

fn extract_declaration(
    file: &TastyFile<'_>,
    node: &dotty::tasty::RawNode<'_>,
) -> Result<Declaration, String> {
    let definition = node
        .decode_definition()
        .map_err(|error| format!("failed to decode declaration at {}: {error}", node.offset))?;
    let (kind, name_ref) = match definition {
        dotty::tasty::DefinitionNode::ValDef { name, .. } => ("ValDef", name),
        dotty::tasty::DefinitionNode::DefDef { name, .. } => ("DefDef", name),
        dotty::tasty::DefinitionNode::TypeDef { name, .. } => ("TypeDef", name),
    };
    let name_entry = file
        .names()
        .entries()
        .get(name_ref as usize)
        .ok_or_else(|| {
            format!(
                "declaration at {} has invalid name reference {name_ref}",
                node.offset
            )
        })?;

    let (parameter_tags, clause_tags) = if node.tag == DEFDEF_TAG {
        let body = node
            .decode_defdef_body()
            .map_err(|error| format!("failed to decode DefDef at {}: {error}", node.offset))?;
        let mut parameters = Vec::new();
        let mut clauses = Vec::new();
        for item in body.header_items {
            match item {
                DefDefHeaderItem::Parameter(parameter) => parameters.push(parameter.tag()),
                DefDefHeaderItem::Clause(clause) => clauses.push(clause),
            }
        }
        (Some(parameters), Some(clauses))
    } else {
        (None, None)
    };

    Ok(Declaration {
        kind,
        name_ref,
        name_kind: format!("{:?}", name_entry.kind()),
        name: name_entry.as_utf8().map(str::to_owned),
        parameter_tags,
        clause_tags,
    })
}

fn encode_json(files: &[FileBaseline]) -> String {
    let mut output = String::from("{\n  \"schema_version\": 1,\n  \"files\": [\n");
    for (file_index, file) in files.iter().enumerate() {
        if file_index != 0 {
            output.push_str(",\n");
        }
        output.push_str("    {\n      \"path\": ");
        push_json_string(&mut output, &file.path);
        output.push_str(",\n      \"tasty_format\": ");
        push_json_string(&mut output, &file.tasty_format);
        output.push_str(",\n      \"ast_tag_counts\": {");
        for (tag_index, (tag, count)) in file.ast_tag_counts.iter().enumerate() {
            if tag_index != 0 {
                output.push(',');
            }
            output.push_str(&format!("\n        \"{tag}\": {count}"));
        }
        if !file.ast_tag_counts.is_empty() {
            output.push('\n');
        }
        output.push_str("      },\n      \"definitions\": [");
        for (declaration_index, declaration) in file.definitions.iter().enumerate() {
            if declaration_index != 0 {
                output.push(',');
            }
            output.push_str("\n        {\n          \"kind\": ");
            push_json_string(&mut output, declaration.kind);
            output.push_str(",\n          \"name_ref\": ");
            output.push_str(&declaration.name_ref.to_string());
            output.push_str(",\n          \"name_kind\": ");
            push_json_string(&mut output, &declaration.name_kind);
            output.push_str(",\n          \"name\": ");
            match &declaration.name {
                Some(name) => push_json_string(&mut output, name),
                None => output.push_str("null"),
            }
            if let Some(tags) = &declaration.parameter_tags {
                output.push_str(",\n          \"parameter_tags\": ");
                push_json_u8_array(&mut output, tags);
                output.push_str(",\n          \"clause_tags\": ");
                push_json_u8_array(
                    &mut output,
                    declaration.clause_tags.as_deref().unwrap_or(&[]),
                );
            }
            output.push_str("\n        }");
        }
        if !file.definitions.is_empty() {
            output.push('\n');
        }
        output.push_str("      ]\n    }");
    }
    output.push_str("\n  ]\n}\n");
    output
}

fn push_json_u8_array(output: &mut String, values: &[u8]) {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str(&value.to_string());
    }
    output.push(']');
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
}
