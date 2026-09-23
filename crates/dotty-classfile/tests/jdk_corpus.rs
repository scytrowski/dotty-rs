//! Decodes and validates real `.class` files from the checked-in JDK 23, 24,
//! 25, and 26 corpora. The shared `classes.txt` file defines corpus
//! membership; each version directory records its provenance, expected
//! class-file major, and inventory in `manifest.toml`.

use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{
    ClassSignature, ClassTypeSignature, ReferenceTypeSignature, TypeArgument, TypeParameter,
};
use std::fs;
use std::path::{Path, PathBuf};

struct Corpus {
    id: String,
    fixture_root: PathBuf,
    classes: Vec<String>,
    expected_jdk_major: u16,
    expected_class_file_major: u16,
    expected_count: usize,
    expected_bytes: u64,
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_corpus")
}

fn manifest_value(manifest: &str, wanted: &str, path: &Path) -> String {
    let value = manifest.lines().find_map(|line| {
        let line = line.split('#').next()?.trim();
        let (key, value) = line.split_once('=')?;
        (key.trim() == wanted).then(|| value.trim().trim_matches('"').to_owned())
    });
    value.unwrap_or_else(|| panic!("manifest {} lacks `{wanted}`", path.display()))
}

fn manifest_u16(manifest: &str, wanted: &str, path: &Path) -> u16 {
    manifest_value(manifest, wanted, path)
        .parse()
        .unwrap_or_else(|error| {
            panic!(
                "manifest {} has invalid `{wanted}`: {error}",
                path.display()
            )
        })
}

fn manifest_usize(manifest: &str, wanted: &str, path: &Path) -> usize {
    manifest_value(manifest, wanted, path)
        .parse()
        .unwrap_or_else(|error| {
            panic!(
                "manifest {} has invalid `{wanted}`: {error}",
                path.display()
            )
        })
}

fn manifest_u64(manifest: &str, wanted: &str, path: &Path) -> u64 {
    manifest_value(manifest, wanted, path)
        .parse()
        .unwrap_or_else(|error| {
            panic!(
                "manifest {} has invalid `{wanted}`: {error}",
                path.display()
            )
        })
}

fn corpora() -> Vec<Corpus> {
    let root = corpus_root();
    let mut directories: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("failed to read corpus root {}: {error}", root.display()))
        .map(|entry| entry.expect("corpus directory entry").path())
        .filter(|path| path.is_dir() && path.join("manifest.toml").is_file())
        .collect();
    directories.sort();

    assert!(
        !directories.is_empty(),
        "no JDK corpus manifests found in {}",
        root.display()
    );

    directories
        .into_iter()
        .map(|directory| {
            let manifest_path = directory.join("manifest.toml");
            let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
                panic!(
                    "failed to read corpus manifest {}: {error}",
                    manifest_path.display()
                )
            });
            let classes_path =
                directory.join(manifest_value(&manifest, "classes_file", &manifest_path));
            let classes = fs::read_to_string(&classes_path)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to read class list {}: {error}",
                        classes_path.display()
                    )
                })
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned)
                .collect();

            Corpus {
                id: manifest_value(&manifest, "id", &manifest_path),
                fixture_root: directory.join(manifest_value(
                    &manifest,
                    "fixture_root",
                    &manifest_path,
                )),
                classes,
                expected_jdk_major: manifest_u16(&manifest, "jdk_major", &manifest_path),
                expected_class_file_major: manifest_u16(
                    &manifest,
                    "class_file_major",
                    &manifest_path,
                ),
                expected_count: manifest_usize(&manifest, "fixture_count", &manifest_path),
                expected_bytes: manifest_u64(&manifest, "fixture_bytes", &manifest_path),
            }
        })
        .collect()
}

fn collect_class_paths(root: &Path, current: &Path, paths: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(current)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read fixture directory {}: {error}",
                current.display()
            )
        })
        .map(|entry| entry.expect("fixture directory entry").path())
        .collect();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_class_paths(root, &path, paths);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "class")
        {
            let relative = path
                .strip_prefix(root)
                .expect("fixture path is under its corpus root")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            paths.push(relative);
        }
    }
}

fn read_corpus_class(corpus: &Corpus, name: &str) -> Vec<u8> {
    let path = corpus.fixture_root.join(format!("{name}.class"));
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read {} class {}: {error}",
            corpus.id,
            path.display()
        )
    })
}

fn decode<'a>(corpus: &Corpus, name: &str, bytes: &'a [u8]) -> ClassFile<'a> {
    let mut reader = Reader::new(bytes);
    let class_file = ClassFile::decode(&mut reader)
        .unwrap_or_else(|error| panic!("{}/{} failed to decode: {error}", corpus.id, name));
    assert!(
        reader.is_at_end(),
        "{}/{} decoded but left unread trailing bytes",
        corpus.id,
        name
    );
    class_file
}

fn utf8_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Utf8(name)) => name.as_str(),
        other => panic!("expected Utf8 at {index:?}, got {other:?}"),
    }
}

fn class_name(pool: &ConstantPool, index: ConstantPoolIndex) -> &str {
    match pool.get(index) {
        Some(ConstantPoolEntry::Class { name_index }) => utf8_name(pool, *name_index),
        other => panic!("expected Class at {index:?}, got {other:?}"),
    }
}

#[test]
fn corpus_inventory_matches_each_manifest() {
    for corpus in corpora() {
        assert_eq!(
            corpus.classes.len(),
            corpus.expected_count,
            "{} class-list count differs from manifest",
            corpus.id
        );

        let mut expected_paths: Vec<String> = corpus
            .classes
            .iter()
            .map(|name| format!("{name}.class"))
            .collect();
        expected_paths.sort();
        let mut actual_paths = Vec::new();
        collect_class_paths(
            &corpus.fixture_root,
            &corpus.fixture_root,
            &mut actual_paths,
        );
        actual_paths.sort();
        assert_eq!(
            actual_paths, expected_paths,
            "{} class inventory",
            corpus.id
        );

        let actual_bytes: u64 = corpus
            .classes
            .iter()
            .map(|name| {
                corpus
                    .fixture_root
                    .join(format!("{name}.class"))
                    .metadata()
                    .unwrap_or_else(|error| panic!("{} metadata: {error}", corpus.id))
                    .len()
            })
            .sum();
        assert_eq!(
            actual_bytes, corpus.expected_bytes,
            "{} byte inventory",
            corpus.id
        );
    }
}

#[test]
fn every_corpus_class_decodes_completely_validates_and_matches_its_jdk() {
    for corpus in corpora() {
        for name in &corpus.classes {
            let bytes = read_corpus_class(&corpus, name);
            let class_file = decode(&corpus, name, &bytes);

            assert_eq!(
                class_file.version.major, corpus.expected_class_file_major,
                "{}/{} class-file major",
                corpus.id, name
            );
            assert_eq!(
                class_file.version.minor, 0,
                "{}/{} class-file minor",
                corpus.id, name
            );
            assert_eq!(
                class_file.version.major,
                corpus.expected_jdk_major + 44,
                "{}/{} JDK-to-major mapping",
                corpus.id,
                name
            );
            assert!(
                class_file.version.is_compatible(),
                "{}/{} version {} should be compatible",
                corpus.id,
                name,
                class_file.version
            );
            assert_eq!(
                class_file.validate_references(),
                Ok(()),
                "{}/{} failed reference validation",
                corpus.id,
                name
            );
        }
    }
}

#[test]
fn constant_desc_is_a_sealed_interface_in_every_corpus() {
    for corpus in corpora() {
        let bytes = read_corpus_class(&corpus, "java/lang/constant/ConstantDesc");
        let class_file = decode(&corpus, "ConstantDesc", &bytes);
        let pool = &class_file.constant_pool;

        assert!(
            class_file.access_flags.is_interface(),
            "{} interface",
            corpus.id
        );

        let permitted = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::PermittedSubclasses(classes) => Some(classes),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} expected PermittedSubclasses", corpus.id));

        let mut names: Vec<&str> = permitted
            .iter()
            .map(|index| class_name(pool, *index))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "java/lang/Double",
                "java/lang/Float",
                "java/lang/Integer",
                "java/lang/Long",
                "java/lang/String",
                "java/lang/constant/ClassDesc",
                "java/lang/constant/DynamicConstantDesc",
                "java/lang/constant/MethodHandleDesc",
                "java/lang/constant/MethodTypeDesc",
            ],
            "{} permitted subclasses",
            corpus.id
        );
    }
}

#[test]
fn day_of_week_is_a_real_enum_in_every_corpus() {
    for corpus in corpora() {
        let bytes = read_corpus_class(&corpus, "java/time/DayOfWeek");
        let class_file = decode(&corpus, "DayOfWeek", &bytes);

        assert!(class_file.access_flags.is_enum(), "{} enum", corpus.id);
    }
}

#[test]
fn array_list_has_a_real_inner_classes_attribute_in_every_corpus() {
    for corpus in corpora() {
        let bytes = read_corpus_class(&corpus, "java/util/ArrayList");
        let class_file = decode(&corpus, "ArrayList", &bytes);
        let pool = &class_file.constant_pool;

        let inner_classes = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::InnerClasses(entries) => Some(entries),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} expected InnerClasses", corpus.id));

        assert!(
            inner_classes.iter().any(|entry| {
                entry
                    .inner_name_index
                    .is_some_and(|index| utf8_name(pool, index) == "Itr")
            }),
            "{} expected an InnerClasses entry named Itr",
            corpus.id
        );
    }
}

#[test]
fn list_and_stream_are_real_interfaces_in_every_corpus() {
    for corpus in corpora() {
        for name in ["java/util/List", "java/util/stream/Stream"] {
            let bytes = read_corpus_class(&corpus, name);
            let class_file = decode(&corpus, name, &bytes);

            assert!(
                class_file.access_flags.is_interface(),
                "{}/{} should be an interface",
                corpus.id,
                name
            );
        }
    }
}

#[test]
fn stream_class_signature_parses_in_every_corpus() {
    for corpus in corpora() {
        let bytes = read_corpus_class(&corpus, "java/util/stream/Stream");
        let class_file = decode(&corpus, "Stream", &bytes);
        let pool = &class_file.constant_pool;

        let signature_index = class_file
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Signature(index) => Some(*index),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} expected class-level Signature", corpus.id));

        let signature = ClassSignature::parse(utf8_name(pool, signature_index))
            .unwrap_or_else(|error| panic!("{} Stream signature: {error}", corpus.id));

        let type_variable_t = ReferenceTypeSignature::TypeVariable("T".to_owned());
        let stream_of_t = ReferenceTypeSignature::Class(ClassTypeSignature {
            package: vec!["java".to_owned(), "util".to_owned(), "stream".to_owned()],
            simple_name: "Stream".to_owned(),
            type_arguments: vec![TypeArgument::Exact(type_variable_t.clone())],
            suffix: vec![],
        });

        assert_eq!(
            signature,
            ClassSignature {
                type_parameters: vec![TypeParameter {
                    name: "T".to_owned(),
                    class_bound: Some(ReferenceTypeSignature::Class(ClassTypeSignature {
                        package: vec!["java".to_owned(), "lang".to_owned()],
                        simple_name: "Object".to_owned(),
                        type_arguments: vec![],
                        suffix: vec![],
                    })),
                    interface_bounds: vec![],
                }],
                superclass: ClassTypeSignature {
                    package: vec!["java".to_owned(), "lang".to_owned()],
                    simple_name: "Object".to_owned(),
                    type_arguments: vec![],
                    suffix: vec![],
                },
                superinterfaces: vec![ClassTypeSignature {
                    package: vec!["java".to_owned(), "util".to_owned(), "stream".to_owned()],
                    simple_name: "BaseStream".to_owned(),
                    type_arguments: vec![
                        TypeArgument::Exact(type_variable_t),
                        TypeArgument::Exact(stream_of_t),
                    ],
                    suffix: vec![],
                }],
            },
            "{} Stream signature",
            corpus.id
        );
    }
}

#[test]
fn string_implements_its_real_interfaces_in_every_corpus() {
    for corpus in corpora() {
        let bytes = read_corpus_class(&corpus, "java/lang/String");
        let class_file = decode(&corpus, "String", &bytes);
        let pool = &class_file.constant_pool;

        let mut names: Vec<&str> = class_file
            .interfaces
            .iter()
            .map(|index| class_name(pool, *index))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "java/io/Serializable",
                "java/lang/CharSequence",
                "java/lang/Comparable",
                "java/lang/constant/Constable",
                "java/lang/constant/ConstantDesc",
            ],
            "{} String interfaces",
            corpus.id
        );
    }
}
