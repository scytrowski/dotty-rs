use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusManifest {
    pub id: String,
    pub source_kind: String,
    pub scala_version: String,
    pub tasty_format: String,
    pub fixture_count: usize,
    pub fixture_bytes: u64,
    pub tasty_root: PathBuf,
    pub selection_file: Option<PathBuf>,
    pub expectation_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    root: PathBuf,
    manifest: CorpusManifest,
}

impl Corpus {
    pub fn load(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let manifest_path = root.join("manifest.toml");
        let manifest_text = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
            panic!(
                "failed to read corpus manifest {}: {error}",
                manifest_path.display()
            )
        });
        let manifest = CorpusManifest::parse(&manifest_text).unwrap_or_else(|error| {
            panic!(
                "failed to parse corpus manifest {}: {error}",
                manifest_path.display()
            )
        });
        Self { root, manifest }
    }

    pub fn manifest(&self) -> &CorpusManifest {
        &self.manifest
    }

    pub fn selection_path(&self) -> Option<PathBuf> {
        self.manifest
            .selection_file
            .as_ref()
            .map(|path| self.root.join(path))
    }

    pub fn expectation_path(&self) -> Option<PathBuf> {
        self.manifest
            .expectation_file
            .as_ref()
            .map(|path| self.root.join(path))
    }

    pub fn tasty_root_path(&self) -> PathBuf {
        self.root.join(&self.manifest.tasty_root)
    }

    pub fn fixture_paths(&self) -> Vec<PathBuf> {
        let root = self.tasty_root_path();
        let mut directories = vec![root];
        let mut fixtures = Vec::new();

        while let Some(directory) = directories.pop() {
            let entries = fs::read_dir(&directory).unwrap_or_else(|error| {
                panic!(
                    "failed to read corpus directory {}: {error}",
                    directory.display()
                )
            });

            for entry in entries {
                let entry = entry.unwrap_or_else(|error| {
                    panic!(
                        "failed to read an entry in corpus directory {}: {error}",
                        directory.display()
                    )
                });
                let path = entry.path();
                let file_type = entry.file_type().unwrap_or_else(|error| {
                    panic!("failed to inspect corpus path {}: {error}", path.display())
                });

                if file_type.is_dir() {
                    directories.push(path);
                } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "tasty")
                {
                    fixtures.push(path);
                }
            }
        }

        fixtures.sort();
        fixtures
    }

    pub fn selected_fixture_paths(&self) -> Vec<PathBuf> {
        let fixtures = self.fixture_paths();
        let Some(selection_path) = self.selection_path() else {
            return fixtures;
        };
        let selection = fs::read_to_string(&selection_path).unwrap_or_else(|error| {
            panic!(
                "failed to read corpus selection {}: {error}",
                selection_path.display()
            )
        });
        let tasty_root = self.tasty_root_path();
        let available: std::collections::BTreeMap<_, _> = fixtures
            .iter()
            .map(|path| {
                (
                    path.strip_prefix(&tasty_root)
                        .expect("fixture must be inside corpus tasty root")
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    path,
                )
            })
            .collect();

        selection
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|name| {
                available.get(name).copied().unwrap_or_else(|| {
                    panic!(
                        "corpus selection {} names missing fixture {:?}",
                        selection_path.display(),
                        name
                    )
                })
            })
            .cloned()
            .collect()
    }
}

impl CorpusManifest {
    fn parse(input: &str) -> Result<Self, String> {
        let field = |name: &str| -> Result<String, String> {
            input
                .lines()
                .filter_map(parse_assignment)
                .find_map(|(key, value)| (key == name).then_some(value.to_owned()))
                .ok_or_else(|| format!("missing manifest field {name:?}"))
        };
        let usize_field = |name: &str| -> Result<usize, String> {
            field(name)?
                .parse()
                .map_err(|error| format!("manifest field {name:?} is not a valid usize: {error}"))
        };
        let u64_field = |name: &str| -> Result<u64, String> {
            field(name)?
                .parse()
                .map_err(|error| format!("manifest field {name:?} is not a valid u64: {error}"))
        };

        Ok(Self {
            id: field("id")?,
            source_kind: field("source_kind")?,
            scala_version: field("scala_version")?,
            tasty_format: field("tasty_format")?,
            fixture_count: usize_field("fixture_count")?,
            fixture_bytes: u64_field("fixture_bytes")?,
            tasty_root: PathBuf::from(
                input
                    .lines()
                    .filter_map(parse_assignment)
                    .find_map(|(key, value)| (key == "tasty_root").then_some(value.to_owned()))
                    .unwrap_or_else(|| ".".to_owned()),
            ),
            selection_file: input
                .lines()
                .filter_map(parse_assignment)
                .find_map(|(key, value)| (key == "selection_file").then_some(PathBuf::from(value))),
            expectation_file: input.lines().filter_map(parse_assignment).find_map(
                |(key, value)| (key == "expectation_file").then_some(PathBuf::from(value)),
            ),
        })
    }
}

fn parse_assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.split('#').next()?.trim();
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    Some((key.trim(), value))
}

#[cfg(test)]
mod tests {
    use super::CorpusManifest;
    use std::path::PathBuf;

    #[test]
    fn parses_required_baseline_metadata() {
        let manifest = CorpusManifest::parse(
            r#"
            id = "example-1.0"
            source_kind = "jar"
            scala_version = "3.9.0"
            tasty_format = "28.9.0"
            fixture_count = 3
            fixture_bytes = 42
            tasty_root = "tasty"
            "#,
        )
        .unwrap();

        assert_eq!(manifest.id, "example-1.0");
        assert_eq!(manifest.source_kind, "jar");
        assert_eq!(manifest.scala_version, "3.9.0");
        assert_eq!(manifest.tasty_format, "28.9.0");
        assert_eq!(manifest.fixture_count, 3);
        assert_eq!(manifest.fixture_bytes, 42);
        assert_eq!(manifest.tasty_root, PathBuf::from("tasty"));
        assert_eq!(manifest.selection_file, None);
        assert_eq!(manifest.expectation_file, None);
    }

    #[test]
    fn defaults_the_tasty_root_to_the_manifest_directory() {
        let manifest = CorpusManifest::parse(
            r#"
            id = "example-1.0"
            source_kind = "directory"
            scala_version = "3.9.0"
            tasty_format = "28.9.0"
            fixture_count = 0
            fixture_bytes = 0
            "#,
        )
        .unwrap();

        assert_eq!(manifest.tasty_root, PathBuf::from("."));
        assert_eq!(manifest.selection_file, None);
        assert_eq!(manifest.expectation_file, None);
    }

    #[test]
    fn resolves_optional_manifest_files_relative_to_the_corpus_root() {
        let corpus = super::Corpus {
            root: PathBuf::from("/tmp/example-corpus"),
            manifest: CorpusManifest {
                id: "example-1.0".to_owned(),
                source_kind: "jar".to_owned(),
                scala_version: "3.9.0".to_owned(),
                tasty_format: "28.9.0".to_owned(),
                fixture_count: 0,
                fixture_bytes: 0,
                tasty_root: PathBuf::from("."),
                selection_file: Some(PathBuf::from("selection.txt")),
                expectation_file: Some(PathBuf::from("expectations/semantic.json")),
            },
        };

        assert_eq!(
            corpus.selection_path(),
            Some(PathBuf::from("/tmp/example-corpus/selection.txt"))
        );
        assert_eq!(
            corpus.expectation_path(),
            Some(PathBuf::from(
                "/tmp/example-corpus/expectations/semantic.json"
            ))
        );
        assert_eq!(
            corpus.tasty_root_path(),
            PathBuf::from("/tmp/example-corpus/./")
        );
    }
}
