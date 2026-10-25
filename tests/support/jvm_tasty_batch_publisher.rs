use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_core::SourceId;
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;
use dotty_tasty::tasty::TastyFile;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const COUNTER_GAUGE: &str = include_str!("../fixtures/jvm_tasty_multiclass/CounterGauge.scala");

pub(crate) const MAX_BATCH_CLASSES: usize = 32;
pub(crate) const MAX_BATCH_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct BatchPublishPair<'a> {
    pub(crate) class: dotty_core::SymbolId,
    pub(crate) source_id: SourceId,
    pub(crate) classfile_path: &'a str,
    pub(crate) classfile_bytes: &'a [u8],
    pub(crate) tasty_path: &'a str,
    pub(crate) tasty_bytes: &'a [u8],
}

#[derive(Clone, Copy)]
pub(crate) enum BatchPublishFault {
    AfterStagedFiles(usize),
    BeforePromotion,
    AfterPromotion,
}

#[derive(Debug)]
pub(crate) enum BatchPublishError {
    InvalidManifest {
        class_index: usize,
        phase: &'static str,
        path: String,
        reason: String,
    },
    InvalidDestination {
        path: PathBuf,
        reason: &'static str,
        cleanup_error: Option<io::Error>,
    },
    DestinationExists {
        path: PathBuf,
        cleanup_error: Option<io::Error>,
    },
    Io {
        phase: &'static str,
        path: PathBuf,
        source: io::Error,
        cleanup_error: Option<io::Error>,
    },
}

impl std::fmt::Display for BatchPublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidManifest {
                class_index,
                phase,
                path,
                reason,
            } => write!(
                formatter,
                "invalid batch manifest at class {class_index} during {phase} ({path}): {reason}"
            ),
            Self::InvalidDestination {
                path,
                reason,
                cleanup_error,
            } => {
                write!(
                    formatter,
                    "invalid destination {}: {reason}{}",
                    path.display(),
                    cleanup_error
                        .as_ref()
                        .map(|error| format!("; staging cleanup failed: {error}"))
                        .unwrap_or_default()
                )
            }
            Self::DestinationExists {
                path,
                cleanup_error,
            } => write!(
                formatter,
                "destination {} already exists{}",
                path.display(),
                cleanup_error
                    .as_ref()
                    .map(|error| format!("; staging cleanup failed: {error}"))
                    .unwrap_or_default()
            ),
            Self::Io {
                phase,
                path,
                source,
                cleanup_error,
            } => write!(
                formatter,
                "batch publication failed during {phase} at {}: {source}{}",
                path.display(),
                cleanup_error
                    .as_ref()
                    .map(|error| format!("; staging cleanup failed: {error}"))
                    .unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for BatchPublishError {}

struct StagedBatchArtifact<'a> {
    relative_path: String,
    bytes: &'a [u8],
}

pub(crate) fn emitted_batch(
    source_index: u32,
) -> dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts {
    emit_paired_jvm_tasty_batch(
        COUNTER_GAUGE,
        "CounterGauge.scala",
        SourceId::from_index(source_index),
    )
    .unwrap()
}

pub(crate) fn batch_publish_pairs(
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
) -> Vec<BatchPublishPair<'_>> {
    batch
        .classes()
        .iter()
        .map(|class| BatchPublishPair {
            class: class.class(),
            source_id: class.source_id(),
            classfile_path: class.classfile_path(),
            classfile_bytes: class.classfile_bytes(),
            tasty_path: class.tasty_path(),
            tasty_bytes: class.tasty_bytes(),
        })
        .collect()
}

pub(crate) fn publish_paired_batch(
    destination: &Path,
    batch: &dotty::jvm_tasty_artifacts::PairedJvmTastyBatchArtifacts,
    fault: Option<BatchPublishFault>,
) -> Result<Vec<PathBuf>, BatchPublishError> {
    let pairs = batch_publish_pairs(batch);
    publish_batch_pairs(destination, &pairs, fault)
}

pub(crate) fn publish_batch_pairs(
    destination: &Path,
    pairs: &[BatchPublishPair<'_>],
    fault: Option<BatchPublishFault>,
) -> Result<Vec<PathBuf>, BatchPublishError> {
    let artifacts = validate_batch_pairs(pairs)?;
    let destination = validate_destination(destination)?;
    let parent = destination
        .parent()
        .expect("validated absolute destination has a parent");
    let staging = create_staging_directory(parent)?;

    if let Err((phase, path, source)) = write_and_check_staging(&staging, &artifacts, fault) {
        let cleanup_error = std::fs::remove_dir_all(&staging).err();
        return Err(BatchPublishError::Io {
            phase,
            path,
            source,
            cleanup_error,
        });
    }

    if matches!(fault, Some(BatchPublishFault::BeforePromotion)) {
        let source = injected_failure("injected failure before batch promotion");
        let cleanup_error = std::fs::remove_dir_all(&staging).err();
        return Err(BatchPublishError::Io {
            phase: "promotion",
            path: destination,
            source,
            cleanup_error,
        });
    }

    match validate_destination(&destination) {
        Ok(_) => {}
        Err(BatchPublishError::DestinationExists { path, .. }) => {
            let cleanup_error = std::fs::remove_dir_all(&staging).err();
            return Err(BatchPublishError::DestinationExists {
                path,
                cleanup_error,
            });
        }
        Err(error) => {
            let cleanup_error = std::fs::remove_dir_all(&staging).err();
            return Err(match error {
                BatchPublishError::Io {
                    phase,
                    path,
                    source,
                    ..
                } => BatchPublishError::Io {
                    phase,
                    path,
                    source,
                    cleanup_error,
                },
                BatchPublishError::InvalidDestination { path, reason, .. } => {
                    BatchPublishError::InvalidDestination {
                        path,
                        reason,
                        cleanup_error,
                    }
                }
                other => other,
            });
        }
    }

    if let Err(source) = std::fs::rename(&staging, &destination) {
        let cleanup_error = std::fs::remove_dir_all(&staging).err();
        return Err(BatchPublishError::Io {
            phase: "promotion",
            path: destination,
            source,
            cleanup_error,
        });
    }

    if matches!(fault, Some(BatchPublishFault::AfterPromotion)) {
        let source = injected_failure("injected failure after batch promotion");
        let cleanup_error = std::fs::remove_dir_all(&destination).err();
        return Err(BatchPublishError::Io {
            phase: "post-promotion rollback",
            path: destination,
            source,
            cleanup_error,
        });
    }

    Ok(artifacts
        .into_iter()
        .map(|artifact| destination.join(artifact.relative_path))
        .collect())
}

fn validate_batch_pairs<'a>(
    pairs: &[BatchPublishPair<'a>],
) -> Result<Vec<StagedBatchArtifact<'a>>, BatchPublishError> {
    if !(2..=MAX_BATCH_CLASSES).contains(&pairs.len()) {
        return Err(BatchPublishError::InvalidManifest {
            class_index: 0,
            phase: "manifest shape",
            path: String::new(),
            reason: "expected 2–32 class pairs".into(),
        });
    }
    let mut total_bytes = 0usize;
    for (class_index, pair) in pairs.iter().enumerate() {
        for (path, bytes) in [
            (pair.classfile_path, pair.classfile_bytes),
            (pair.tasty_path, pair.tasty_bytes),
        ] {
            total_bytes = total_bytes.checked_add(bytes.len()).ok_or_else(|| {
                invalid_manifest(
                    class_index,
                    "manifest size",
                    path,
                    "artifact byte count overflow",
                )
            })?;
            if total_bytes > MAX_BATCH_ARTIFACT_BYTES {
                return Err(invalid_manifest(
                    class_index,
                    "manifest size",
                    path,
                    "batch exceeds the 64 MiB publication limit",
                ));
            }
        }
    }
    let source_id = pairs[0].source_id;
    let mut classes = Vec::with_capacity(pairs.len());
    let mut paths = Vec::<String>::with_capacity(pairs.len() * 2);
    let mut files = Vec::with_capacity(pairs.len() * 2);

    for (class_index, pair) in pairs.iter().enumerate() {
        if pair.source_id != source_id {
            return Err(invalid_manifest(
                class_index,
                "source identity",
                pair.classfile_path,
                "all classes must come from one source session",
            ));
        }
        if classes.contains(&pair.class) {
            return Err(invalid_manifest(
                class_index,
                "class identity",
                pair.classfile_path,
                "duplicate canonical class identity",
            ));
        }
        classes.push(pair.class);

        let class_stem = validate_artifact_path(
            class_index,
            "class-file path",
            pair.classfile_path,
            ".class",
        )?;
        let tasty_stem =
            validate_artifact_path(class_index, "TASTy path", pair.tasty_path, ".tasty")?;
        for path in [pair.classfile_path, pair.tasty_path] {
            if paths.iter().any(|existing| existing == path) {
                return Err(invalid_manifest(
                    class_index,
                    "artifact paths",
                    path,
                    "duplicate output path",
                ));
            }
            paths.push(path.to_owned());
        }
        if class_stem != tasty_stem {
            return Err(invalid_manifest(
                class_index,
                "artifact mapping",
                pair.tasty_path,
                "classfile and TASTy sidecar must have the same package-qualified stem",
            ));
        }

        let mut reader = Reader::new(pair.classfile_bytes);
        let class_file = ClassFile::decode(&mut reader).map_err(|error| {
            invalid_manifest(
                class_index,
                "class-file decode",
                pair.classfile_path,
                format!("{error:?}"),
            )
        })?;
        if !reader.is_at_end() {
            return Err(invalid_manifest(
                class_index,
                "class-file decode",
                pair.classfile_path,
                "trailing bytes after class file",
            ));
        }
        class_file.validate_references().map_err(|error| {
            invalid_manifest(
                class_index,
                "class-file references",
                pair.classfile_path,
                format!("{error:?}"),
            )
        })?;
        if (class_file.version.major, class_file.version.minor) != (65, 0) {
            return Err(invalid_manifest(
                class_index,
                "class-file version",
                pair.classfile_path,
                format!(
                    "expected 65.0, found {}.{}",
                    class_file.version.major, class_file.version.minor
                ),
            ));
        }
        let class_name = class_file
            .constant_pool
            .class_name(class_file.this_class)
            .map_err(|error| {
                invalid_manifest(
                    class_index,
                    "class identity",
                    pair.classfile_path,
                    format!("{error:?}"),
                )
            })?;
        if class_name != class_stem {
            return Err(invalid_manifest(
                class_index,
                "class identity",
                pair.classfile_path,
                format!("path names {class_stem}, classfile names {class_name}"),
            ));
        }

        let tasty = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes).map_err(|error| {
            invalid_manifest(
                class_index,
                "TASTy decode",
                pair.tasty_path,
                format!("{error:?}"),
            )
        })?;
        if (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ) != (28, 9, 0)
        {
            return Err(invalid_manifest(
                class_index,
                "TASTy version",
                pair.tasty_path,
                "expected format 28.9.0",
            ));
        }
        validate_classfile_tasty_link(&class_file, pair.tasty_bytes).map_err(|error| {
            invalid_manifest(
                class_index,
                "classfile/TASTy link",
                pair.tasty_path,
                format!("{error:?}"),
            )
        })?;

        for (path, bytes) in [
            (pair.classfile_path, pair.classfile_bytes),
            (pair.tasty_path, pair.tasty_bytes),
        ] {
            files.push(StagedBatchArtifact {
                relative_path: path.to_owned(),
                bytes,
            });
        }
    }

    for path in &paths {
        if paths
            .iter()
            .any(|other| other != path && other.starts_with(&format!("{path}/")))
        {
            return Err(invalid_manifest(
                0,
                "artifact paths",
                path,
                "one artifact path is a directory prefix of another",
            ));
        }
    }
    Ok(files)
}

fn validate_artifact_path<'a>(
    class_index: usize,
    phase: &'static str,
    path: &'a str,
    extension: &str,
) -> Result<&'a str, BatchPublishError> {
    let invalid = || {
        invalid_manifest(
            class_index,
            phase,
            path,
            "expected a canonical package-relative path with its exact artifact extension",
        )
    };
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
        || path.len() > 1024
    {
        return Err(invalid());
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 2
        || segments
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(invalid());
    }
    let stem = path.strip_suffix(extension).ok_or_else(invalid)?;
    if stem.is_empty() || stem.rsplit('/').next().is_none_or(str::is_empty) {
        return Err(invalid());
    }
    Ok(stem)
}

fn invalid_manifest(
    class_index: usize,
    phase: &'static str,
    path: impl Into<String>,
    reason: impl Into<String>,
) -> BatchPublishError {
    BatchPublishError::InvalidManifest {
        class_index,
        phase,
        path: path.into(),
        reason: reason.into(),
    }
}

fn validate_destination(destination: &Path) -> Result<PathBuf, BatchPublishError> {
    use std::path::Component;

    if !destination.is_absolute()
        || destination
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(BatchPublishError::InvalidDestination {
            path: destination.to_owned(),
            reason: "destination must be an absolute normalized path",
            cleanup_error: None,
        });
    }

    let mut current = PathBuf::new();
    for component in destination.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(BatchPublishError::InvalidDestination {
                    path: current,
                    reason: "destination path contains a symlink",
                    cleanup_error: None,
                });
            }
            Ok(metadata) if current == destination => {
                let _ = metadata;
                return Err(BatchPublishError::DestinationExists {
                    path: destination.to_owned(),
                    cleanup_error: None,
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(BatchPublishError::InvalidDestination {
                    path: current,
                    reason: "destination parent is not a directory",
                    cleanup_error: None,
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if current == destination {
                    break;
                }
                return Err(BatchPublishError::InvalidDestination {
                    path: current,
                    reason: "destination parent does not exist",
                    cleanup_error: None,
                });
            }
            Err(source) => {
                return Err(BatchPublishError::Io {
                    phase: "destination validation",
                    path: current,
                    source,
                    cleanup_error: None,
                });
            }
        }
    }
    Ok(destination.to_owned())
}

fn create_staging_directory(parent: &Path) -> Result<PathBuf, BatchPublishError> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_STAGING_ID: AtomicUsize = AtomicUsize::new(0);

    for _ in 0..128 {
        let staging = parent.join(format!(
            ".dotty-jvm-tasty-batch-{}-{}",
            std::process::id(),
            NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
        ));
        match create_private_staging_directory(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(BatchPublishError::Io {
                    phase: "staging directory creation",
                    path: staging,
                    source,
                    cleanup_error: None,
                });
            }
        }
    }
    Err(BatchPublishError::Io {
        phase: "staging directory creation",
        path: parent.to_owned(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique batch staging directory",
        ),
        cleanup_error: None,
    })
}

fn create_private_staging_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

fn write_and_check_staging(
    staging: &Path,
    artifacts: &[StagedBatchArtifact<'_>],
    fault: Option<BatchPublishFault>,
) -> Result<(), (&'static str, PathBuf, io::Error)> {
    for (index, artifact) in artifacts.iter().enumerate() {
        let relative = Path::new(&artifact.relative_path);
        let output = staging.join(relative);
        let parent = output
            .parent()
            .expect("validated artifact path has a parent");
        std::fs::create_dir_all(parent)
            .map_err(|source| ("staging parents", parent.to_owned(), source))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|source| ("staging write", output.clone(), source))?;
        file.write_all(artifact.bytes)
            .and_then(|()| file.flush())
            .map_err(|source| ("staging write", output.clone(), source))?;
        if matches!(fault, Some(BatchPublishFault::AfterStagedFiles(count)) if count == index + 1) {
            return Err((
                "staging write",
                output,
                injected_failure("injected failure during batch staging"),
            ));
        }
    }

    let inventory = read_relative_files(staging)
        .map_err(|source| ("staging verification", staging.to_owned(), source))?;
    let mut expected = artifacts
        .iter()
        .map(|artifact| artifact.relative_path.clone())
        .collect::<Vec<_>>();
    expected.sort();
    if inventory != expected {
        return Err((
            "staging verification",
            staging.to_owned(),
            io::Error::new(
                io::ErrorKind::InvalidData,
                "staged file inventory differs from the validated manifest",
            ),
        ));
    }
    for artifact in artifacts {
        let path = staging.join(&artifact.relative_path);
        let staged = std::fs::read(&path)
            .map_err(|source| ("staging verification", path.clone(), source))?;
        if staged != artifact.bytes {
            return Err((
                "staging verification",
                path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "staged bytes differ from the validated manifest",
                ),
            ));
        }
    }
    Ok(())
}

fn injected_failure(message: &str) -> io::Error {
    io::Error::other(message)
}

pub(crate) fn read_relative_files(root: &Path) -> io::Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<String>) -> io::Result<()> {
        let mut entries = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("staging tree contains a symlink: {}", path.display()),
                ));
            }
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else if metadata.is_file() {
                let relative = path.strip_prefix(root).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })?;
                let relative = relative.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 staged path")
                })?;
                files.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("staging tree contains a special file: {}", path.display()),
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}
