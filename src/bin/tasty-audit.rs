use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use dotty::tasty::{NodeCategory, StructuredNode, TastyFile};

#[derive(Default)]
struct Count {
    occurrences: usize,
    files: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("tasty-audit: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let root = env::args()
        .nth(1)
        .ok_or_else(|| "usage: tasty-audit <directory>".to_owned())?;
    let root = PathBuf::from(root);
    let fixtures = fixture_paths(&root)?;

    let mut total_bytes = 0_u64;
    let mut headers = BTreeMap::<(u32, u32, u32), usize>::new();
    let mut sections = BTreeMap::<String, Count>::new();
    let mut tags = BTreeMap::<u8, Count>::new();
    let mut structured_category5_tags = BTreeMap::<u8, Count>::new();
    let mut raw_category5_tags = BTreeMap::<u8, Count>::new();
    let mut visible_node_count = 0;

    for path in &fixtures {
        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        total_bytes = total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| "fixture byte count overflowed u64".to_owned())?;

        let file = TastyFile::parse(&bytes)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        file.validate()
            .map_err(|error| format!("failed to validate {}: {error}", path.display()))?;
        *headers
            .entry((
                file.header().major_version,
                file.header().minor_version,
                file.header().experimental_version,
            ))
            .or_default() += 1;

        let mut file_sections = BTreeSet::new();
        for section in file.sections().iter() {
            let name = section
                .standard_kind(file.names())
                .map(|kind| kind.as_str().to_owned())
                .unwrap_or_else(|| format!("name-ref-{}", section.name));
            let entry = sections.entry(name.clone()).or_default();
            entry.occurrences += 1;
            file_sections.insert(name);
        }
        for name in file_sections {
            sections
                .get_mut(&name)
                .expect("section was inserted above")
                .files += 1;
        }

        let index = file
            .ast_address_index()
            .map_err(|error| format!("failed to index {}: {error}", path.display()))?;
        let mut file_tags = BTreeSet::new();
        for node in index.iter_nodes() {
            visible_node_count += 1;
            tags.entry(node.tag).or_default().occurrences += 1;
            file_tags.insert(node.tag);
        }
        for tag in file_tags {
            tags.get_mut(&tag).expect("tag was inserted above").files += 1;
        }

        let mut file_structured_category5_tags = BTreeSet::new();
        let mut file_raw_category5_tags = BTreeSet::new();
        for raw in index.iter() {
            let target = match raw.decode_structured() {
                Ok(StructuredNode::Raw(_)) => {
                    file_raw_category5_tags.insert(raw.tag);
                    &mut raw_category5_tags
                }
                Ok(_) => {
                    file_structured_category5_tags.insert(raw.tag);
                    &mut structured_category5_tags
                }
                Err(error) => {
                    return Err(format!(
                        "failed to dispatch tag {} in {}: {error}",
                        raw.tag,
                        path.display()
                    ));
                }
            };
            target.entry(raw.tag).or_default().occurrences += 1;
        }
        for tag in file_structured_category5_tags {
            structured_category5_tags
                .get_mut(&tag)
                .expect("tag was inserted above")
                .files += 1;
        }
        for tag in file_raw_category5_tags {
            raw_category5_tags
                .get_mut(&tag)
                .expect("tag was inserted above")
                .files += 1;
        }
    }

    println!("TASTy corpus audit");
    println!("root: {}", root.display());
    println!("fixtures: {}", fixtures.len());
    println!("fixture_bytes: {total_bytes}");
    println!("visible_ast_nodes: {visible_node_count}");

    println!("\nheaders:");
    for ((major, minor, experimental), count) in headers {
        println!("  {major}.{minor}.{experimental}: {count} files");
    }

    println!("\nsections:");
    for (name, count) in sections {
        println!(
            "  {name}: {} occurrences in {} files",
            count.occurrences, count.files
        );
    }

    println!("\nvisible_ast_tags:");
    println!("  tag  category  occurrences  files");
    for (tag, count) in tags {
        let category = NodeCategory::from_tag(tag)
            .map(|category| format!("{category:?}"))
            .unwrap_or_else(|| "none".to_owned());
        println!(
            "  {tag:>3}  {category:<9}  {:>11}  {:>5}",
            count.occurrences, count.files
        );
    }

    println!("\ncategory5_dispatch:");
    println!("  tag  structured_occurrences  structured_files  raw_occurrences  raw_files");
    let category5_tags: BTreeSet<_> = structured_category5_tags
        .keys()
        .chain(raw_category5_tags.keys())
        .copied()
        .collect();
    for tag in category5_tags {
        println!(
            "  {tag:>3}  {:>22}  {:>16}  {:>14}  {:>9}",
            structured_category5_tags
                .get(&tag)
                .map_or(0, |count| count.occurrences),
            structured_category5_tags
                .get(&tag)
                .map_or(0, |count| count.files),
            raw_category5_tags
                .get(&tag)
                .map_or(0, |count| count.occurrences),
            raw_category5_tags.get(&tag).map_or(0, |count| count.files)
        );
    }

    Ok(())
}

fn fixture_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut directories = vec![root.to_owned()];
    let mut fixtures = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read an entry in {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "tasty") {
                fixtures.push(path);
            }
        }
    }

    fixtures.sort();
    Ok(fixtures)
}
