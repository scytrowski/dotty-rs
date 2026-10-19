use crate::crc32;
use crate::inflate::{InflateError, inflate};
use crate::zip_reader::{ZipReadError, ZipReader};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4B50;
/// The fixed-size portion of the end-of-central-directory record (JAR/ZIP
/// APPNOTE.TXT §4.3.16), not counting the variable-length comment.
const END_OF_CENTRAL_DIRECTORY_RECORD_SIZE: usize = 22;
/// The comment length field is a `u16`, so a comment can be at most this
/// many bytes.
const MAX_COMMENT_LENGTH: usize = 0xFFFF;
const CENTRAL_DIRECTORY_FILE_HEADER_SIGNATURE: u32 = 0x0201_4B50;
const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4B50;
/// ZIP `compression method` 0 (APPNOTE.TXT §4.4.5): no compression.
const COMPRESSION_METHOD_STORED: u16 = 0;
/// ZIP `compression method` 8 (APPNOTE.TXT §4.4.5): DEFLATE (RFC 1951).
const COMPRESSION_METHOD_DEFLATE: u16 = 8;
/// The largest a single entry may declare, or actually decompress to. A
/// `.class`/`.tasty` entry this crate actually needs to read is at most a
/// few megabytes; this is a generous but firm ceiling against a crafted
/// archive's own central-directory `uncompressed_size` field, or a
/// DEFLATE stream's length/distance back-reference amplification,
/// claiming or producing far more than that (a "ZIP bomb" — APPNOTE.TXT
/// places no bound on either at all).
const MAX_ENTRY_UNCOMPRESSED_SIZE: usize = 256 * 1024 * 1024;
/// Maximum central-directory buffer read while indexing any backing.
/// Real classpath archives are far smaller; this caps hostile index
/// declarations before allocating their contents.
const MAX_CENTRAL_DIRECTORY_SIZE: usize = 256 * 1024 * 1024;

/// Errors reading a ZIP (JAR) archive. Crate-private: callers only see
/// `JarClassPath`, which maps these into [`crate::class_path::ClassPathError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ZipError {
    /// No end-of-central-directory record was found. Either this isn't a
    /// ZIP archive, or it uses ZIP64 (which this crate does not support —
    /// see `docs/classloader.md`'s Milestone 2 scope).
    EndOfCentralDirectoryNotFound,
    /// A central directory file header didn't start with the expected
    /// signature, at the given byte offset.
    InvalidCentralDirectoryHeader {
        offset: usize,
    },
    /// A central directory entry's file name was not valid UTF-8, at the
    /// given byte offset.
    InvalidEntryName {
        offset: usize,
    },
    /// A local file header didn't start with the expected signature, at
    /// the given byte offset.
    InvalidLocalFileHeader {
        offset: usize,
    },
    /// The entry uses a compression method this crate does not (yet)
    /// implement.
    UnsupportedCompressionMethod(u16),
    /// A DEFLATE-compressed entry failed to decompress.
    Inflate(InflateError),
    /// An entry's decompressed bytes did not match the CRC-32 declared
    /// for it in the central directory.
    CrcMismatch {
        expected: u32,
        actual: u32,
    },
    /// The central directory's declared `uncompressed_size` for this
    /// entry exceeds [`MAX_ENTRY_UNCOMPRESSED_SIZE`] — rejected before
    /// any decompression was attempted, so a crafted archive can't even
    /// get as far as allocating a large buffer, let alone decompressing
    /// into one.
    EntryTooLarge {
        declared: u32,
        limit: usize,
    },
    /// The compressed input for an entry exceeds the bounded read limit.
    CompressedEntryTooLarge {
        declared: u32,
        limit: usize,
    },
    /// The central directory exceeds the bounded indexing limit.
    CentralDirectoryTooLarge {
        declared: u32,
        limit: usize,
    },
    /// An entry's actual extracted/decompressed length did not match the
    /// `uncompressed_size` declared for it in the central directory —
    /// either corrupt data, or a stream deliberately crafted to produce
    /// more or less than what it claims.
    UncompressedSizeMismatch {
        declared: u32,
        actual: usize,
    },
    /// An operating-system error while reading the file-backed archive.
    FileRead {
        kind: std::io::ErrorKind,
        message: String,
    },
    Read(ZipReadError),
}

impl fmt::Display for ZipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfCentralDirectoryNotFound => write!(
                formatter,
                "end-of-central-directory record not found (not a ZIP archive, or a ZIP64 archive, which is unsupported)"
            ),
            Self::InvalidCentralDirectoryHeader { offset } => write!(
                formatter,
                "invalid central directory file header at offset {offset}"
            ),
            Self::InvalidEntryName { offset } => {
                write!(
                    formatter,
                    "invalid (non-UTF-8) entry name at offset {offset}"
                )
            }
            Self::InvalidLocalFileHeader { offset } => {
                write!(formatter, "invalid local file header at offset {offset}")
            }
            Self::UnsupportedCompressionMethod(method) => {
                write!(formatter, "unsupported ZIP compression method {method}")
            }
            Self::Inflate(error) => write!(formatter, "DEFLATE decompression failed: {error}"),
            Self::CrcMismatch { expected, actual } => write!(
                formatter,
                "CRC-32 mismatch: expected {expected:#010x}, computed {actual:#010x}"
            ),
            Self::EntryTooLarge { declared, limit } => write!(
                formatter,
                "entry's declared uncompressed size ({declared} bytes) exceeds the {limit}-byte limit"
            ),
            Self::CompressedEntryTooLarge { declared, limit } => write!(
                formatter,
                "entry's compressed size ({declared} bytes) exceeds the {limit}-byte read limit"
            ),
            Self::CentralDirectoryTooLarge { declared, limit } => write!(
                formatter,
                "central directory size ({declared} bytes) exceeds the {limit}-byte indexing limit"
            ),
            Self::UncompressedSizeMismatch { declared, actual } => write!(
                formatter,
                "entry's actual size ({actual} bytes) does not match its declared uncompressed size ({declared} bytes)"
            ),
            Self::FileRead { kind, message } => {
                write!(formatter, "ZIP file read failed ({kind:?}): {message}")
            }
            Self::Read(error) => error.fmt(formatter),
        }
    }
}

impl ZipError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::EndOfCentralDirectoryNotFound => "classpath.zip.unsupported_or_invalid_archive",
            Self::InvalidCentralDirectoryHeader { .. } => {
                "classpath.zip.invalid_central_directory_header"
            }
            Self::InvalidEntryName { .. } => "classpath.zip.invalid_entry_name",
            Self::InvalidLocalFileHeader { .. } => "classpath.zip.invalid_local_file_header",
            Self::UnsupportedCompressionMethod(_) => "classpath.zip.unsupported_compression",
            Self::Inflate(_) => "classpath.zip.invalid_deflate_data",
            Self::CrcMismatch { .. } => "classpath.zip.crc_mismatch",
            Self::EntryTooLarge { .. } | Self::CompressedEntryTooLarge { .. } => {
                "classpath.zip.entry_too_large"
            }
            Self::CentralDirectoryTooLarge { .. } => "classpath.zip.directory_too_large",
            Self::UncompressedSizeMismatch { .. } => "classpath.zip.size_mismatch",
            Self::FileRead { .. } => "classpath.io",
            Self::Read(_) => "classpath.zip.read_error",
        }
    }

    pub(crate) fn category(&self) -> crate::error::ClassLoadCategory {
        use crate::error::ClassLoadCategory as C;
        match self {
            Self::UnsupportedCompressionMethod(_)
            | Self::EntryTooLarge { .. }
            | Self::CompressedEntryTooLarge { .. }
            | Self::CentralDirectoryTooLarge { .. } => C::UnsupportedFeatureOrVersion,
            Self::EndOfCentralDirectoryNotFound => C::Conditional,
            Self::FileRead { .. } => C::IoOrEnvironment,
            Self::Read(_) => C::Conditional,
            Self::InvalidCentralDirectoryHeader { .. }
            | Self::InvalidEntryName { .. }
            | Self::InvalidLocalFileHeader { .. }
            | Self::Inflate(_)
            | Self::CrcMismatch { .. }
            | Self::UncompressedSizeMismatch { .. } => C::MalformedArtifact,
        }
    }
}

impl std::error::Error for ZipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EndOfCentralDirectoryNotFound
            | Self::InvalidCentralDirectoryHeader { .. }
            | Self::InvalidEntryName { .. }
            | Self::InvalidLocalFileHeader { .. }
            | Self::UnsupportedCompressionMethod(_)
            | Self::CrcMismatch { .. }
            | Self::EntryTooLarge { .. }
            | Self::CompressedEntryTooLarge { .. }
            | Self::CentralDirectoryTooLarge { .. }
            | Self::FileRead { .. }
            | Self::UncompressedSizeMismatch { .. } => None,
            Self::Inflate(error) => Some(error),
            Self::Read(error) => Some(error),
        }
    }
}

impl From<InflateError> for ZipError {
    fn from(error: InflateError) -> Self {
        Self::Inflate(error)
    }
}

impl From<ZipReadError> for ZipError {
    fn from(error: ZipReadError) -> Self {
        Self::Read(error)
    }
}

/// The fixed fields of a ZIP end-of-central-directory record that this
/// crate needs (APPNOTE.TXT §4.3.16). Multi-disk archives and ZIP64 are
/// not supported: this always reads the single-disk fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EndOfCentralDirectory {
    pub(crate) entry_count: u16,
    pub(crate) central_directory_size: u32,
    pub(crate) central_directory_offset: u32,
}

impl EndOfCentralDirectory {
    /// Locates and decodes the end-of-central-directory record by
    /// scanning backward from the end of `bytes`. The record sits at the
    /// very end of a ZIP file, after an optional comment of up to 65535
    /// bytes, so the search window is bounded to the trailing
    /// `22 + 65535` bytes. A candidate is only accepted once its own
    /// comment-length field is checked against how many bytes actually
    /// follow it — this rules out a signature that happens to appear
    /// inside the comment itself, to the left of the real record.
    pub(crate) fn locate_and_decode(bytes: &[u8]) -> Result<Self, ZipError> {
        let len = bytes.len();
        if len < END_OF_CENTRAL_DIRECTORY_RECORD_SIZE {
            return Err(ZipError::EndOfCentralDirectoryNotFound);
        }

        let search_floor =
            len.saturating_sub(END_OF_CENTRAL_DIRECTORY_RECORD_SIZE + MAX_COMMENT_LENGTH);
        let search_ceiling = len - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;

        for offset in (search_floor..=search_ceiling).rev() {
            let Ok(mut reader) = ZipReader::with_range(bytes, offset, len) else {
                continue;
            };

            let Ok(signature) = reader.read_u32() else {
                continue;
            };
            if signature != END_OF_CENTRAL_DIRECTORY_SIGNATURE {
                continue;
            }

            // These reads cannot fail: the loop bounds guarantee at least
            // `END_OF_CENTRAL_DIRECTORY_RECORD_SIZE` bytes remain from
            // `offset`, and only 18 more bytes are read here.
            reader.read_u16()?; // number of this disk
            reader.read_u16()?; // disk with the start of the central directory
            reader.read_u16()?; // entries in the central directory on this disk
            let entry_count = reader.read_u16()?;
            let central_directory_size = reader.read_u32()?;
            let central_directory_offset = reader.read_u32()?;
            let comment_length = reader.read_u16()?;

            let expected_len =
                offset + END_OF_CENTRAL_DIRECTORY_RECORD_SIZE + usize::from(comment_length);
            if expected_len == len {
                return Ok(Self {
                    entry_count,
                    central_directory_size,
                    central_directory_offset,
                });
            }
        }

        Err(ZipError::EndOfCentralDirectoryNotFound)
    }
}

/// One entry's metadata from a ZIP central directory (APPNOTE.TXT
/// §4.3.12): enough to find and extract that entry's bytes, without yet
/// reading them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CentralDirectoryEntry {
    pub(crate) name: String,
    pub(crate) compression_method: u16,
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u32,
    pub(crate) uncompressed_size: u32,
    pub(crate) local_header_offset: u32,
}

/// Decodes `eocd.entry_count` central directory file headers starting at
/// `eocd.central_directory_offset`, bounded to `eocd.central_directory_size`
/// bytes.
#[cfg(test)]
pub(crate) fn decode_central_directory(
    bytes: &[u8],
    eocd: &EndOfCentralDirectory,
) -> Result<Vec<CentralDirectoryEntry>, ZipError> {
    decode_central_directory_at(bytes, eocd, 0)
}

fn decode_central_directory_at(
    bytes: &[u8],
    eocd: &EndOfCentralDirectory,
    offset_bias: usize,
) -> Result<Vec<CentralDirectoryEntry>, ZipError> {
    let start = eocd.central_directory_offset as usize;
    let end = start
        .checked_add(eocd.central_directory_size as usize)
        .ok_or(ZipError::Read(ZipReadError::InvalidRange {
            start,
            end: usize::MAX,
            len: bytes.len(),
        }))?;
    let mut reader = ZipReader::with_offset_bias(bytes, start, end, offset_bias)?;

    let mut entries = Vec::with_capacity(usize::from(eocd.entry_count));
    for _ in 0..eocd.entry_count {
        let header_offset = reader.position();

        let signature = reader.read_u32()?;
        if signature != CENTRAL_DIRECTORY_FILE_HEADER_SIGNATURE {
            return Err(ZipError::InvalidCentralDirectoryHeader {
                offset: header_offset,
            });
        }

        reader.read_u16()?; // version made by
        reader.read_u16()?; // version needed to extract
        reader.read_u16()?; // general purpose bit flag
        let compression_method = reader.read_u16()?;
        reader.read_u16()?; // last mod file time
        reader.read_u16()?; // last mod file date
        let crc32 = reader.read_u32()?;
        let compressed_size = reader.read_u32()?;
        let uncompressed_size = reader.read_u32()?;
        let file_name_length = reader.read_u16()?;
        let extra_field_length = reader.read_u16()?;
        let file_comment_length = reader.read_u16()?;
        reader.read_u16()?; // disk number start
        reader.read_u16()?; // internal file attributes
        reader.read_u32()?; // external file attributes
        let local_header_offset = reader.read_u32()?;

        let name_offset = reader.position();
        let name_bytes = reader.read_bytes(usize::from(file_name_length))?;
        let name =
            String::from_utf8(name_bytes.to_vec()).map_err(|_| ZipError::InvalidEntryName {
                offset: name_offset,
            })?;

        reader.read_bytes(usize::from(extra_field_length))?;
        reader.read_bytes(usize::from(file_comment_length))?;

        entries.push(CentralDirectoryEntry {
            name,
            compression_method,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });
    }

    Ok(entries)
}

fn invalid_archive_range(start: u64, end: u64, archive_len: u64) -> ZipError {
    let start = usize::try_from(start).unwrap_or(usize::MAX);
    ZipError::Read(ZipReadError::InvalidRange {
        start,
        end: usize::try_from(end).unwrap_or(usize::MAX),
        len: usize::try_from(archive_len).unwrap_or(usize::MAX),
    })
}

#[derive(Debug)]
#[allow(dead_code)] // Kept private for the follow-up that switches JMODs to this backing.
enum ZipStorage {
    Memory(Vec<u8>),
    File {
        file: Mutex<File>,
        archive_start: u64,
        archive_len: u64,
    },
}

impl ZipStorage {
    fn len(&self) -> u64 {
        match self {
            Self::Memory(bytes) => bytes.len() as u64,
            Self::File { archive_len, .. } => *archive_len,
        }
    }

    fn read_range(&self, start: u64, length: usize) -> Result<Vec<u8>, ZipError> {
        self.with_range(start, length, |bytes| Ok(bytes.to_vec()))
    }

    fn with_range<T>(
        &self,
        start: u64,
        length: usize,
        read: impl FnOnce(&[u8]) -> Result<T, ZipError>,
    ) -> Result<T, ZipError> {
        let archive_len = self.len();
        let length_u64 = u64::try_from(length).map_err(|_| {
            ZipError::Read(ZipReadError::InvalidRange {
                start: usize::try_from(start).unwrap_or(usize::MAX),
                end: usize::MAX,
                len: usize::try_from(archive_len).unwrap_or(usize::MAX),
            })
        })?;
        let end = start.checked_add(length_u64).ok_or_else(|| {
            ZipError::Read(ZipReadError::InvalidRange {
                start: usize::try_from(start).unwrap_or(usize::MAX),
                end: usize::MAX,
                len: usize::try_from(archive_len).unwrap_or(usize::MAX),
            })
        })?;
        let start_usize = usize::try_from(start).unwrap_or(usize::MAX);
        let end_usize = usize::try_from(end).unwrap_or(usize::MAX);
        let len_usize = usize::try_from(archive_len).unwrap_or(usize::MAX);
        if start > archive_len {
            return Err(ZipError::Read(ZipReadError::InvalidRange {
                start: start_usize,
                end: end_usize,
                len: len_usize,
            }));
        }
        if end > archive_len {
            return Err(ZipError::Read(ZipReadError::UnexpectedEof {
                offset: start_usize,
                needed: length,
                remaining: usize::try_from(archive_len - start).unwrap_or(usize::MAX),
            }));
        }

        match self {
            Self::Memory(bytes) => read(&bytes[start_usize..end_usize]),
            Self::File {
                file,
                archive_start,
                ..
            } => {
                let physical_offset = archive_start.checked_add(start).ok_or({
                    ZipError::Read(ZipReadError::InvalidRange {
                        start: start_usize,
                        end: end_usize,
                        len: len_usize,
                    })
                })?;
                let mut file = file.lock().map_err(|_| ZipError::FileRead {
                    kind: std::io::ErrorKind::Other,
                    message: "ZIP file read lock was poisoned".to_owned(),
                })?;
                let bytes = file
                    .seek(SeekFrom::Start(physical_offset))
                    .and_then(|_| {
                        let mut bytes = vec![0; length];
                        file.read_exact(&mut bytes)?;
                        Ok(bytes)
                    })
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::UnexpectedEof {
                            ZipError::Read(ZipReadError::UnexpectedEof {
                                offset: start_usize,
                                needed: length,
                                remaining: 0,
                            })
                        } else {
                            ZipError::FileRead {
                                kind: error.kind(),
                                message: error.to_string(),
                            }
                        }
                    })?;
                drop(file);
                read(&bytes)
            }
        }
    }

    #[cfg(test)]
    fn retained_archive_payload_bytes(&self) -> usize {
        match self {
            Self::Memory(bytes) => bytes.len(),
            Self::File { .. } => 0,
        }
    }
}

/// A ZIP archive indexed by its central directory. Its bytes are either
/// retained in memory (JARs and test fixtures) or fetched from a
/// synchronized file handle in bounded ranges.
pub(crate) struct ZipArchive {
    storage: ZipStorage,
    entries: HashMap<String, CentralDirectoryEntry>,
    class_resource_names: Vec<String>,
}

impl ZipArchive {
    pub(crate) fn open(bytes: Vec<u8>) -> Result<Self, ZipError> {
        Self::open_storage(ZipStorage::Memory(bytes))
    }

    #[allow(dead_code)] // Kept as the path-based constructor for other ZIP adapters.
    pub(crate) fn open_file(path: impl AsRef<Path>, archive_start: u64) -> Result<Self, ZipError> {
        let file = File::open(path).map_err(|error| ZipError::FileRead {
            kind: error.kind(),
            message: error.to_string(),
        })?;
        Self::open_file_handle(file, archive_start)
    }

    pub(crate) fn open_file_handle(file: File, archive_start: u64) -> Result<Self, ZipError> {
        let file_len = file
            .metadata()
            .map_err(|error| ZipError::FileRead {
                kind: error.kind(),
                message: error.to_string(),
            })?
            .len();
        if archive_start > file_len {
            return Err(ZipError::Read(ZipReadError::InvalidRange {
                start: usize::try_from(archive_start).unwrap_or(usize::MAX),
                end: usize::try_from(archive_start).unwrap_or(usize::MAX),
                len: usize::try_from(file_len).unwrap_or(usize::MAX),
            }));
        }

        Self::open_storage(ZipStorage::File {
            file: Mutex::new(file),
            archive_start,
            archive_len: file_len - archive_start,
        })
    }

    fn open_storage(storage: ZipStorage) -> Result<Self, ZipError> {
        let archive_len = storage.len();
        let search_window_len = usize::try_from(
            archive_len.min((END_OF_CENTRAL_DIRECTORY_RECORD_SIZE + MAX_COMMENT_LENGTH) as u64),
        )
        .expect("bounded ZIP search window fits usize");
        let search_window =
            storage.read_range(archive_len - search_window_len as u64, search_window_len)?;
        let eocd = EndOfCentralDirectory::locate_and_decode(&search_window)?;
        let central_directory_size = eocd.central_directory_size as usize;
        if central_directory_size > MAX_CENTRAL_DIRECTORY_SIZE {
            return Err(ZipError::CentralDirectoryTooLarge {
                declared: eocd.central_directory_size,
                limit: MAX_CENTRAL_DIRECTORY_SIZE,
            });
        }
        let central_directory_start = u64::from(eocd.central_directory_offset);
        let central_directory_end = central_directory_start
            .checked_add(u64::from(eocd.central_directory_size))
            .ok_or_else(|| invalid_archive_range(central_directory_start, u64::MAX, archive_len))?;
        if central_directory_end > archive_len {
            return Err(invalid_archive_range(
                central_directory_start,
                central_directory_end,
                archive_len,
            ));
        }
        let normalized_eocd = EndOfCentralDirectory {
            central_directory_offset: 0,
            ..eocd
        };
        let decoded_entries = storage.with_range(
            central_directory_start,
            central_directory_size,
            |central_directory| {
                decode_central_directory_at(
                    central_directory,
                    &normalized_eocd,
                    central_directory_start as usize,
                )
            },
        )?;
        let entries: HashMap<_, _> = decoded_entries
            .into_iter()
            .map(|entry| (entry.name.clone(), entry))
            .collect();
        let mut class_resource_names = entries
            .keys()
            .filter(|name| name.ends_with(".class") || name.ends_with(".tasty"))
            .cloned()
            .collect::<Vec<_>>();
        class_resource_names.sort();

        Ok(Self {
            storage,
            entries,
            class_resource_names,
        })
    }

    #[cfg(test)]
    fn retained_index_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| entry.name.len() + std::mem::size_of::<CentralDirectoryEntry>())
            .sum::<usize>()
            + self
                .class_resource_names
                .iter()
                .map(String::len)
                .sum::<usize>()
    }

    #[cfg(test)]
    pub(crate) fn retained_archive_payload_bytes(&self) -> usize {
        self.storage.retained_archive_payload_bytes()
    }

    /// Extracts and decompresses the entry named `name`, verifying its
    /// CRC-32 against the value declared in the central directory.
    /// `Ok(None)` means the archive has no such entry — not an error.
    pub(crate) fn read_entry(&self, name: &str) -> Result<Option<Vec<u8>>, ZipError> {
        let Some(entry) = self.entries.get(name) else {
            return Ok(None);
        };

        let decompressed = self.extract_entry(entry)?;

        let actual_crc32 = crc32::checksum(&decompressed);
        if actual_crc32 != entry.crc32 {
            return Err(ZipError::CrcMismatch {
                expected: entry.crc32,
                actual: actual_crc32,
            });
        }

        Ok(Some(decompressed))
    }

    /// Whether the archive has a direct class or TASTy file below this
    /// directory prefix. File contents are not needed to establish package
    /// evidence, so this only inspects the already-validated central index.
    pub(crate) fn contains_package(&self, prefix: &str) -> bool {
        self.entries.keys().any(|name| {
            let Some(file) = name.strip_prefix(prefix) else {
                return false;
            };
            !file.is_empty()
                && !file.contains('/')
                && (file.ends_with(".class") || file.ends_with(".tasty"))
        })
    }

    /// Returns the indexed logical package paths proved by class or TASTy
    /// resources under `archive_root`. Resource names were selected from the
    /// validated central directory when this archive was opened; no entry is
    /// decompressed for this query.
    pub(crate) fn package_prefixes_under(&self, archive_root: &str) -> HashSet<String> {
        let mut packages = HashSet::new();
        for resource in &self.class_resource_names {
            if let Some(logical_path) = resource.strip_prefix(archive_root) {
                add_resource_package_prefixes(logical_path, &mut packages);
            }
        }
        packages
    }

    /// Returns the logical package paths proved by eligible multi-release
    /// resources at or below `release_version`. The caller decides whether
    /// the archive's manifest enables multi-release lookup.
    pub(crate) fn versioned_package_prefixes(&self, release_version: u16) -> HashSet<String> {
        let mut packages = HashSet::new();
        for resource in &self.class_resource_names {
            let Some(versioned_path) = resource.strip_prefix("META-INF/versions/") else {
                continue;
            };
            let Some((version_name, logical_path)) = versioned_path.split_once('/') else {
                continue;
            };
            let Ok(version) = version_name.parse::<u16>() else {
                continue;
            };
            if version.to_string() != version_name || !(9..=release_version).contains(&version) {
                continue;
            }
            add_resource_package_prefixes(logical_path, &mut packages);
        }
        packages
    }

    /// Reads the local file header at `entry.local_header_offset` to find
    /// where the entry's data starts, then decompresses
    /// `entry.compressed_size` bytes from there. Sizes and the
    /// compression method are taken from the central directory (already
    /// parsed), not re-read from the local header — the local header's
    /// own size fields can legitimately be zero when the general-purpose
    /// "data descriptor follows" bit is set, so the central directory is
    /// the only field that's always authoritative.
    fn extract_entry(&self, entry: &CentralDirectoryEntry) -> Result<Vec<u8>, ZipError> {
        let declared_uncompressed_size = entry.uncompressed_size as usize;
        if declared_uncompressed_size > MAX_ENTRY_UNCOMPRESSED_SIZE {
            return Err(ZipError::EntryTooLarge {
                declared: entry.uncompressed_size,
                limit: MAX_ENTRY_UNCOMPRESSED_SIZE,
            });
        }

        if entry.compressed_size as usize > MAX_ENTRY_UNCOMPRESSED_SIZE {
            return Err(ZipError::CompressedEntryTooLarge {
                declared: entry.compressed_size,
                limit: MAX_ENTRY_UNCOMPRESSED_SIZE,
            });
        }

        let header_offset = u64::from(entry.local_header_offset);
        let fixed_header = self.storage.read_range(header_offset, 30)?;
        let mut reader = ZipReader::with_range(&fixed_header, 0, fixed_header.len())?;

        let signature = reader.read_u32()?;
        if signature != LOCAL_FILE_HEADER_SIGNATURE {
            return Err(ZipError::InvalidLocalFileHeader {
                offset: usize::try_from(header_offset).unwrap_or(usize::MAX),
            });
        }

        reader.read_u16()?; // version needed to extract
        reader.read_u16()?; // general purpose bit flag
        reader.read_u16()?; // compression method (already known)
        reader.read_u16()?; // last mod file time
        reader.read_u16()?; // last mod file date
        reader.read_u32()?; // crc-32 (already known)
        reader.read_u32()?; // compressed size (already known)
        reader.read_u32()?; // uncompressed size (already known)
        let file_name_length = reader.read_u16()?;
        let extra_field_length = reader.read_u16()?;
        let variable_length = usize::from(file_name_length) + usize::from(extra_field_length);
        let variable_start = header_offset
            .checked_add(30)
            .ok_or_else(|| invalid_archive_range(header_offset, u64::MAX, self.storage.len()))?;
        self.storage.read_range(variable_start, variable_length)?;
        let compressed_start = variable_start
            .checked_add(variable_length as u64)
            .ok_or_else(|| invalid_archive_range(variable_start, u64::MAX, self.storage.len()))?;
        let decompressed = match entry.compression_method {
            COMPRESSION_METHOD_STORED => self
                .storage
                .read_range(compressed_start, entry.compressed_size as usize)?,
            COMPRESSION_METHOD_DEFLATE => self.storage.with_range(
                compressed_start,
                entry.compressed_size as usize,
                |compressed| Ok(inflate(compressed, declared_uncompressed_size)?),
            )?,
            other => {
                return self.storage.with_range(
                    compressed_start,
                    entry.compressed_size as usize,
                    |_| Err(ZipError::UnsupportedCompressionMethod(other)),
                );
            }
        };

        if decompressed.len() != declared_uncompressed_size {
            return Err(ZipError::UncompressedSizeMismatch {
                declared: entry.uncompressed_size,
                actual: decompressed.len(),
            });
        }

        Ok(decompressed)
    }
}

fn add_resource_package_prefixes(resource: &str, packages: &mut HashSet<String>) {
    if resource.starts_with('/') || resource.ends_with('/') {
        return;
    }
    if !resource.ends_with(".class") && !resource.ends_with(".tasty") {
        return;
    }
    if resource.contains('\\') || resource.contains(':') || resource.contains('\0') {
        return;
    }

    let segments = resource.split('/').collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        || segments
            .first()
            .is_some_and(|segment| *segment == "META-INF")
    {
        return;
    }

    packages.insert(String::new());
    for length in 1..segments.len() {
        packages.insert(segments[..length].join("/"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEMP_ARCHIVE: AtomicUsize = AtomicUsize::new(0);

    struct TemporaryArchiveFile(PathBuf);

    impl TemporaryArchiveFile {
        fn new(bytes: &[u8]) -> Self {
            let id = NEXT_TEMP_ARCHIVE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "dotty-classloader-zip-{}-{id}.zip",
                std::process::id()
            ));
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TemporaryArchiveFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn open_memory_and_file(bytes: &[u8]) -> (ZipArchive, ZipArchive, TemporaryArchiveFile) {
        let file = TemporaryArchiveFile::new(bytes);
        (
            ZipArchive::open(bytes.to_vec()).unwrap(),
            ZipArchive::open_file(file.path(), 0).unwrap(),
            file,
        )
    }

    fn minimal_eocd(entry_count: u16, cd_size: u32, cd_offset: u32, comment: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // signature
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk with start of cd
        bytes.extend_from_slice(&entry_count.to_le_bytes()); // entries on this disk
        bytes.extend_from_slice(&entry_count.to_le_bytes()); // total entries
        bytes.extend_from_slice(&cd_size.to_le_bytes());
        bytes.extend_from_slice(&cd_offset.to_le_bytes());
        bytes.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);
        bytes
    }

    #[test]
    fn locates_the_eocd_with_no_comment() {
        let bytes = minimal_eocd(3, 100, 50, &[]);

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 3);
        assert_eq!(eocd.central_directory_size, 100);
        assert_eq!(eocd.central_directory_offset, 50);
    }

    #[test]
    fn locates_the_eocd_with_a_comment() {
        let bytes = minimal_eocd(1, 40, 10, b"hello, jar");

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 1);
        assert_eq!(eocd.central_directory_size, 40);
        assert_eq!(eocd.central_directory_offset, 10);
    }

    #[test]
    fn locates_the_eocd_when_preceded_by_unrelated_archive_bytes() {
        let mut bytes = vec![0xAB; 128]; // stand-in for local headers + central directory
        bytes.extend(minimal_eocd(2, 64, 20, &[]));

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 2);
    }

    #[test]
    fn reports_not_found_when_the_signature_is_missing() {
        let bytes = vec![0u8; 22];

        assert_eq!(
            EndOfCentralDirectory::locate_and_decode(&bytes),
            Err(ZipError::EndOfCentralDirectoryNotFound)
        );
    }

    #[test]
    fn reports_not_found_for_a_too_short_buffer() {
        let bytes = vec![0x50, 0x4B, 0x05, 0x06];

        assert_eq!(
            EndOfCentralDirectory::locate_and_decode(&bytes),
            Err(ZipError::EndOfCentralDirectoryNotFound)
        );
    }

    fn minimal_central_directory_header(
        name: &str,
        compression_method: u16,
        crc32: u32,
        compressed_size: u32,
        uncompressed_size: u32,
        local_header_offset: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // signature
        bytes.extend_from_slice(&0u16.to_le_bytes()); // version made by
        bytes.extend_from_slice(&0u16.to_le_bytes()); // version needed to extract
        bytes.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        bytes.extend_from_slice(&compression_method.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file time
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file date
        bytes.extend_from_slice(&crc32.to_le_bytes());
        bytes.extend_from_slice(&compressed_size.to_le_bytes());
        bytes.extend_from_slice(&uncompressed_size.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes()); // file name length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        bytes.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
        bytes.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
        bytes.extend_from_slice(&local_header_offset.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes
    }

    /// Builds a complete, minimal single-entry ZIP archive: a local file
    /// header (mirroring the fields `extract_entry` reads) immediately
    /// followed by `compressed_data`, then a matching central directory
    /// entry and end-of-central-directory record.
    ///
    /// `declared_uncompressed_size` is passed straight through to the
    /// central directory, independent of `compressed_data`'s own length
    /// or what it actually decompresses to — letting a test build an
    /// entry whose declared size doesn't match reality, the same way a
    /// crafted or corrupted archive could.
    fn minimal_zip_with_entry(
        name: &str,
        compression_method: u16,
        crc32: u32,
        compressed_data: &[u8],
        declared_uncompressed_size: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // local file header signature
        bytes.extend_from_slice(&0u16.to_le_bytes()); // version needed to extract
        bytes.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        bytes.extend_from_slice(&compression_method.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file time
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file date
        bytes.extend_from_slice(&crc32.to_le_bytes());
        bytes.extend_from_slice(&(compressed_data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&declared_uncompressed_size.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(compressed_data);

        let central_directory_offset = bytes.len() as u32;
        bytes.extend(minimal_central_directory_header(
            name,
            compression_method,
            crc32,
            compressed_data.len() as u32,
            declared_uncompressed_size,
            0,
        ));
        let central_directory_size = bytes.len() as u32 - central_directory_offset;

        bytes.extend(minimal_eocd(
            1,
            central_directory_size,
            central_directory_offset,
            &[],
        ));
        bytes
    }

    #[test]
    fn file_backing_reads_a_zip_after_a_nonzero_archive_start_offset() {
        let archive = minimal_zip_with_entry(
            "pkg/Value.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"class bytes"),
            b"class bytes",
            11,
        );
        let mut payload = b"JM\x01\x00".to_vec();
        payload.extend_from_slice(&archive);
        let file = TemporaryArchiveFile::new(&payload);
        let memory = ZipArchive::open(archive).unwrap();
        let backed = ZipArchive::open_file(file.path(), 4).unwrap();

        assert_eq!(
            memory.read_entry("pkg/Value.class"),
            backed.read_entry("pkg/Value.class")
        );
        assert_eq!(
            backed.read_entry("pkg/Value.class").unwrap().unwrap(),
            b"class bytes"
        );
    }

    #[test]
    fn file_backing_reads_a_class_from_the_checked_in_jar_fixture() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pool_sample_jar/pool_sample_stored.jar");
        let memory = ZipArchive::open(std::fs::read(&path).unwrap()).unwrap();
        let file_backed = ZipArchive::open_file(&path, 0).unwrap();

        assert_eq!(
            memory.read_entry("PoolSample.class"),
            file_backed.read_entry("PoolSample.class")
        );
        assert!(
            file_backed
                .read_entry("PoolSample.class")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn memory_and_file_backings_share_real_deflate_decoding() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pool_sample_jar/pool_sample_deflate.jar");
        let memory = ZipArchive::open(std::fs::read(&path).unwrap()).unwrap();
        let file_backed = ZipArchive::open_file(&path, 0).unwrap();

        let expected = memory.read_entry("PoolSample.class").unwrap().unwrap();
        assert_eq!(
            file_backed.read_entry("PoolSample.class").unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn malformed_deflate_keeps_the_invalid_deflate_diagnostic_code() {
        let archive_bytes = minimal_zip_with_entry(
            "Broken.class",
            COMPRESSION_METHOD_DEFLATE,
            crate::crc32::checksum(&[]),
            &[0x07], // BFINAL = 1, reserved BTYPE = 3
            0,
        );
        let archive = ZipArchive::open(archive_bytes).unwrap();

        let error = archive
            .read_entry("Broken.class")
            .expect_err("reserved block type must be rejected");

        assert!(matches!(
            error,
            ZipError::Inflate(InflateError::InvalidData)
        ));
        assert_eq!(error.code(), "classpath.zip.invalid_deflate_data");
    }

    #[test]
    fn memory_and_file_backings_match_for_valid_missing_and_package_queries() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // local header
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&COMPRESSION_METHOD_STORED.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&crate::crc32::checksum(b"x").to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&("pkg/nested/A.class".len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(b"pkg/nested/A.class");
        bytes.push(b'x');
        let central_directory_offset = bytes.len() as u32;
        bytes.extend(minimal_central_directory_header(
            "pkg/nested/A.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"x"),
            1,
            1,
            0,
        ));
        let central_directory_size = bytes.len() as u32 - central_directory_offset;
        bytes.extend(minimal_eocd(
            1,
            central_directory_size,
            central_directory_offset,
            &[],
        ));
        let (memory, file_backed, _file) = open_memory_and_file(&bytes);

        assert_eq!(
            memory.read_entry("pkg/nested/A.class"),
            file_backed.read_entry("pkg/nested/A.class")
        );
        assert_eq!(memory.read_entry("absent.class"), Ok(None));
        assert_eq!(
            memory.read_entry("absent.class"),
            file_backed.read_entry("absent.class")
        );
        for prefix in ["", "pkg/", "pkg/nested/"] {
            assert_eq!(
                memory.contains_package(prefix),
                file_backed.contains_package(prefix)
            );
        }
        assert_eq!(
            memory.package_prefixes_under(""),
            file_backed.package_prefixes_under("")
        );
    }

    #[test]
    fn memory_and_file_backings_match_for_versioned_package_evidence() {
        let bytes = crate::jar_class_path::tests::minimal_stored_zip(&[
            ("pkg/Base.class", b"base"),
            ("META-INF/versions/11/pkg/Versioned.class", b"versioned"),
        ]);
        let (memory, file_backed, _file) = open_memory_and_file(&bytes);

        for release in [8, 11, 17, u16::MAX] {
            assert_eq!(
                memory.versioned_package_prefixes(release),
                file_backed.versioned_package_prefixes(release)
            );
        }
        assert_eq!(memory.versioned_package_prefixes(10), HashSet::new());
        assert_eq!(
            memory.versioned_package_prefixes(11),
            HashSet::from([String::new(), "pkg".to_owned()])
        );
    }

    #[test]
    fn memory_and_file_backings_match_for_crc_errors() {
        let mut bytes = minimal_zip_with_entry(
            "Crc.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"x"),
            b"x",
            1,
        );
        bytes[30 + "Crc.class".len()] = b'y';
        let (memory, file_backed, _file) = open_memory_and_file(&bytes);

        assert_eq!(
            memory.read_entry("Crc.class").unwrap_err(),
            file_backed.read_entry("Crc.class").unwrap_err()
        );
    }

    #[test]
    fn memory_and_file_backings_match_for_invalid_central_offsets_and_names() {
        let mut invalid_offset =
            minimal_zip_with_entry("Offset.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = invalid_offset.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        invalid_offset[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        let file = TemporaryArchiveFile::new(&invalid_offset);
        let memory_error = match ZipArchive::open(invalid_offset) {
            Err(error) => error,
            Ok(_) => panic!("invalid central offset unexpectedly opened in memory"),
        };
        let file_error = match ZipArchive::open_file(file.path(), 0) {
            Err(error) => error,
            Ok(_) => panic!("invalid central offset unexpectedly opened from a file"),
        };
        assert_eq!(memory_error, file_error);

        let mut oversized_directory =
            minimal_zip_with_entry("Bounded.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = oversized_directory.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        oversized_directory[eocd + 12..eocd + 16]
            .copy_from_slice(&((MAX_CENTRAL_DIRECTORY_SIZE as u32) + 1).to_le_bytes());
        let file = TemporaryArchiveFile::new(&oversized_directory);
        let memory_error = match ZipArchive::open(oversized_directory) {
            Err(error) => error,
            Ok(_) => panic!("oversized central directory unexpectedly opened in memory"),
        };
        let file_error = match ZipArchive::open_file(file.path(), 0) {
            Err(error) => error,
            Ok(_) => panic!("oversized central directory unexpectedly opened from a file"),
        };
        assert_eq!(memory_error, file_error);
        assert_eq!(memory_error.code(), "classpath.zip.directory_too_large");

        let mut truncated_central =
            minimal_zip_with_entry("One.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = truncated_central.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        truncated_central[eocd + 10..eocd + 12].copy_from_slice(&2u16.to_le_bytes());
        let file = TemporaryArchiveFile::new(&truncated_central);
        let memory_error = match ZipArchive::open(truncated_central) {
            Err(error) => error,
            Ok(_) => panic!("truncated central directory unexpectedly opened in memory"),
        };
        let file_error = match ZipArchive::open_file(file.path(), 0) {
            Err(error) => error,
            Ok(_) => panic!("truncated central directory unexpectedly opened from a file"),
        };
        assert_eq!(memory_error, file_error);

        let mut invalid_name =
            minimal_zip_with_entry("Name.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = invalid_name.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central =
            u32::from_le_bytes(invalid_name[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        invalid_name[central + 46] = 0xFF;
        let file = TemporaryArchiveFile::new(&invalid_name);
        let memory_error = match ZipArchive::open(invalid_name) {
            Err(error) => error,
            Ok(_) => panic!("invalid name unexpectedly opened in memory"),
        };
        let file_error = match ZipArchive::open_file(file.path(), 0) {
            Err(error) => error,
            Ok(_) => panic!("invalid name unexpectedly opened from a file"),
        };
        assert_eq!(memory_error, file_error);

        let mut invalid_local_offset =
            minimal_zip_with_entry("Offset.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = invalid_local_offset.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central = u32::from_le_bytes(
            invalid_local_offset[eocd + 16..eocd + 20]
                .try_into()
                .unwrap(),
        ) as usize;
        invalid_local_offset[central + 42..central + 46].copy_from_slice(&u32::MAX.to_le_bytes());
        let (memory, file_backed, _file) = open_memory_and_file(&invalid_local_offset);
        assert_eq!(
            memory.read_entry("Offset.class").unwrap_err(),
            file_backed.read_entry("Offset.class").unwrap_err()
        );
    }

    #[test]
    fn memory_and_file_backings_match_for_local_header_data_and_size_errors() {
        let mut invalid_header = minimal_zip_with_entry(
            "Header.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"x"),
            b"x",
            1,
        );
        invalid_header[..4].copy_from_slice(&0u32.to_le_bytes());
        let (memory, file_backed, _file) = open_memory_and_file(&invalid_header);
        assert_eq!(
            memory.read_entry("Header.class").unwrap_err(),
            file_backed.read_entry("Header.class").unwrap_err()
        );

        let mut truncated_header = minimal_zip_with_entry(
            "Header.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"x"),
            b"x",
            1,
        );
        truncated_header[26..28].copy_from_slice(&u16::MAX.to_le_bytes());
        let (memory, file_backed, _file) = open_memory_and_file(&truncated_header);
        assert_eq!(
            memory.read_entry("Header.class").unwrap_err(),
            file_backed.read_entry("Header.class").unwrap_err()
        );

        let mut truncated_data =
            minimal_zip_with_entry("Data.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = truncated_data.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central =
            u32::from_le_bytes(truncated_data[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let too_long_data_range = u32::try_from(truncated_data.len()).unwrap() + 1;
        truncated_data[central + 20..central + 24]
            .copy_from_slice(&too_long_data_range.to_le_bytes());
        let (memory, file_backed, _file) = open_memory_and_file(&truncated_data);
        assert!(matches!(
            memory.read_entry("Data.class"),
            Err(ZipError::Read(ZipReadError::UnexpectedEof { .. }))
        ));
        assert_eq!(
            memory.read_entry("Data.class").unwrap_err(),
            file_backed.read_entry("Data.class").unwrap_err()
        );

        let oversized = minimal_zip_with_entry(
            "Large.class",
            COMPRESSION_METHOD_STORED,
            0,
            b"x",
            MAX_ENTRY_UNCOMPRESSED_SIZE as u32 + 1,
        );
        let (memory, file_backed, _file) = open_memory_and_file(&oversized);
        assert_eq!(
            memory.read_entry("Large.class").unwrap_err(),
            file_backed.read_entry("Large.class").unwrap_err()
        );

        let unsupported = minimal_zip_with_entry("Method.class", 99, 0, b"x", 1);
        let (memory, file_backed, _file) = open_memory_and_file(&unsupported);
        assert_eq!(
            memory.read_entry("Method.class").unwrap_err(),
            file_backed.read_entry("Method.class").unwrap_err()
        );

        let mut oversized_compressed =
            minimal_zip_with_entry("Compressed.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = oversized_compressed.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central = u32::from_le_bytes(
            oversized_compressed[eocd + 16..eocd + 20]
                .try_into()
                .unwrap(),
        ) as usize;
        oversized_compressed[central + 20..central + 24].copy_from_slice(&u32::MAX.to_le_bytes());
        let (memory, file_backed, _file) = open_memory_and_file(&oversized_compressed);
        assert_eq!(
            memory.read_entry("Compressed.class").unwrap_err(),
            file_backed.read_entry("Compressed.class").unwrap_err()
        );
    }

    #[test]
    fn file_backing_reports_a_typed_error_if_the_archive_is_truncated_after_open() {
        let bytes = minimal_zip_with_entry(
            "Truncated.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"payload"),
            b"payload",
            7,
        );
        let file = TemporaryArchiveFile::new(&bytes);
        let archive = ZipArchive::open_file(file.path(), 0).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(file.path())
            .unwrap()
            .set_len(32)
            .unwrap();

        let error = archive.read_entry("Truncated.class").unwrap_err();
        assert!(matches!(
            error,
            ZipError::Read(ZipReadError::UnexpectedEof { .. })
        ));
        assert_eq!(error.code(), "classpath.zip.read_error");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::Conditional
        );
    }

    #[test]
    fn file_backing_preserves_classpath_io_classification_for_a_missing_file() {
        let id = NEXT_TEMP_ARCHIVE.fetch_add(1, Ordering::Relaxed);
        let missing = std::env::temp_dir().join(format!(
            "dotty-classloader-zip-missing-{}-{id}.zip",
            std::process::id()
        ));

        let error = match ZipArchive::open_file(&missing, 0) {
            Err(error) => error,
            Ok(_) => panic!("missing ZIP file unexpectedly opened"),
        };

        assert_eq!(error.code(), "classpath.io");
        assert_eq!(
            error.category(),
            crate::error::ClassLoadCategory::IoOrEnvironment
        );
    }

    #[test]
    fn file_backing_retains_no_archive_payload_and_serializes_concurrent_reads() {
        let bytes = minimal_zip_with_entry(
            "Parallel.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"class bytes"),
            b"class bytes",
            11,
        );
        let mut padded = bytes.clone();
        let eocd = padded.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central_directory_offset =
            u32::from_le_bytes(padded[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let padding = vec![0; 4 * 1024 * 1024];
        padded.splice(central_directory_offset..central_directory_offset, padding);
        let padded_eocd = padded.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        padded[padded_eocd + 16..padded_eocd + 20]
            .copy_from_slice(&((central_directory_offset + 4 * 1024 * 1024) as u32).to_le_bytes());
        let file = TemporaryArchiveFile::new(&padded);
        let archive = ZipArchive::open_file(file.path(), 0).unwrap();
        assert_eq!(archive.storage.retained_archive_payload_bytes(), 0);
        assert!(
            std::fs::metadata(file.path()).unwrap().len()
                > u64::from(archive.entries.len() as u32) * 1024 * 1024
        );
        assert!(
            archive.retained_index_bytes() < std::fs::metadata(file.path()).unwrap().len() as usize
        );
        assert!(archive.entries.contains_key("Parallel.class"));

        let archive = std::sync::Arc::new(archive);
        std::thread::scope(|scope| {
            let readers = (0..8)
                .map(|_| {
                    let archive = std::sync::Arc::clone(&archive);
                    scope.spawn(move || archive.read_entry("Parallel.class").unwrap().unwrap())
                })
                .collect::<Vec<_>>();
            for reader in readers {
                assert_eq!(reader.join().unwrap(), b"class bytes");
            }
        });
    }

    #[test]
    fn rejects_a_declared_uncompressed_size_above_the_limit() {
        let compressed_data = b"tiny".as_slice();
        let crc = crate::crc32::checksum(compressed_data);
        let declared = MAX_ENTRY_UNCOMPRESSED_SIZE as u32 + 1;
        let archive_bytes = minimal_zip_with_entry(
            "Bomb.class",
            COMPRESSION_METHOD_STORED,
            crc,
            compressed_data,
            declared,
        );

        let archive = ZipArchive::open(archive_bytes).unwrap();

        assert_eq!(
            archive.read_entry("Bomb.class"),
            Err(ZipError::EntryTooLarge {
                declared,
                limit: MAX_ENTRY_UNCOMPRESSED_SIZE,
            })
        );
    }

    /// A synthetic "declared size lies" regression: a crafted (or
    /// corrupted) archive can declare a small `uncompressed_size` for an
    /// entry that actually holds more data than that — the mismatch must
    /// be rejected, not silently truncated or passed through, regardless
    /// of compression method.
    #[test]
    fn rejects_an_entry_whose_actual_size_does_not_match_its_declared_size() {
        // STORED, so the "compressed" bytes are also the real
        // decompressed output — no DEFLATE encoding needed to prove the
        // declared-size check fires.
        let data = b"ABABAB".as_slice();
        let crc = crate::crc32::checksum(data);
        let archive_bytes =
            minimal_zip_with_entry("Lying.class", COMPRESSION_METHOD_STORED, crc, data, 1);

        let archive = ZipArchive::open(archive_bytes).unwrap();

        assert_eq!(
            archive.read_entry("Lying.class"),
            Err(ZipError::UncompressedSizeMismatch {
                declared: 1,
                actual: 6,
            })
        );
    }

    #[test]
    fn decodes_two_central_directory_headers() {
        let mut central_directory = Vec::new();
        central_directory.extend(minimal_central_directory_header(
            "PoolSample.class",
            0,
            0x1234_5678,
            100,
            100,
            0,
        ));
        central_directory.extend(minimal_central_directory_header(
            "java/lang/Object.class",
            8,
            0x9ABC_DEF0,
            40,
            80,
            120,
        ));

        let eocd = EndOfCentralDirectory {
            entry_count: 2,
            central_directory_size: central_directory.len() as u32,
            central_directory_offset: 0,
        };

        let entries = decode_central_directory(&central_directory, &eocd).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "PoolSample.class");
        assert_eq!(entries[0].compression_method, 0);
        assert_eq!(entries[0].crc32, 0x1234_5678);
        assert_eq!(entries[0].compressed_size, 100);
        assert_eq!(entries[0].uncompressed_size, 100);
        assert_eq!(entries[0].local_header_offset, 0);

        assert_eq!(entries[1].name, "java/lang/Object.class");
        assert_eq!(entries[1].compression_method, 8);
        assert_eq!(entries[1].crc32, 0x9ABC_DEF0);
        assert_eq!(entries[1].compressed_size, 40);
        assert_eq!(entries[1].uncompressed_size, 80);
        assert_eq!(entries[1].local_header_offset, 120);
    }

    #[test]
    fn reports_an_invalid_header_when_the_signature_is_wrong() {
        let mut central_directory =
            minimal_central_directory_header("PoolSample.class", 0, 0, 0, 0, 0);
        central_directory[0] = 0x00; // corrupt the signature

        let eocd = EndOfCentralDirectory {
            entry_count: 1,
            central_directory_size: central_directory.len() as u32,
            central_directory_offset: 0,
        };

        assert_eq!(
            decode_central_directory(&central_directory, &eocd),
            Err(ZipError::InvalidCentralDirectoryHeader { offset: 0 })
        );
    }

    #[test]
    fn rejects_a_central_directory_offset_outside_the_archive() {
        let mut bytes =
            minimal_zip_with_entry("Offset.class", COMPRESSION_METHOD_STORED, 0, b"x", 1);
        let eocd = bytes.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        bytes[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());

        let error = ZipArchive::open(bytes)
            .err()
            .expect("an out-of-range central directory must fail");

        assert!(matches!(
            error,
            ZipError::Read(ZipReadError::InvalidRange { .. })
        ));
        assert_eq!(error.code(), "classpath.zip.read_error");
    }

    #[test]
    fn rejects_an_entry_offset_outside_the_archive_when_reading_it() {
        let mut bytes = minimal_zip_with_entry(
            "Offset.class",
            COMPRESSION_METHOD_STORED,
            crate::crc32::checksum(b"x"),
            b"x",
            1,
        );
        let eocd = bytes.len() - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;
        let central_directory =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        bytes[central_directory + 42..central_directory + 46]
            .copy_from_slice(&u32::MAX.to_le_bytes());

        let archive = ZipArchive::open(bytes).unwrap();
        let error = archive.read_entry("Offset.class").unwrap_err();

        assert!(matches!(
            error,
            ZipError::Read(ZipReadError::InvalidRange { .. })
        ));
        assert_eq!(error.code(), "classpath.zip.read_error");
    }

    #[test]
    fn rejects_an_unsupported_compression_method_when_reading_an_entry() {
        let bytes = minimal_zip_with_entry("Compressed.class", 99, 0, b"x", 1);
        let archive = ZipArchive::open(bytes).unwrap();

        assert_eq!(
            archive.read_entry("Compressed.class"),
            Err(ZipError::UnsupportedCompressionMethod(99))
        );
    }

    fn pool_sample_class_bytes() -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-classfile/tests/fixtures/pool_sample/PoolSample.class"),
        )
        .expect("PoolSample.class fixture should exist")
    }

    fn pool_sample_stored_jar_bytes() -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pool_sample_jar/pool_sample_stored.jar"),
        )
        .expect("pool_sample_stored.jar fixture should exist")
    }

    #[test]
    fn extracts_a_stored_entry_matching_the_original_bytes_exactly() {
        let archive = ZipArchive::open(pool_sample_stored_jar_bytes()).unwrap();

        let extracted = archive
            .read_entry("PoolSample.class")
            .unwrap()
            .expect("entry should be present");

        assert_eq!(extracted, pool_sample_class_bytes());
    }

    #[test]
    fn read_entry_returns_none_for_a_missing_name() {
        let archive = ZipArchive::open(pool_sample_stored_jar_bytes()).unwrap();

        assert_eq!(archive.read_entry("DoesNotExist.class").unwrap(), None);
    }

    #[test]
    fn crc_mismatch_is_reported_instead_of_silently_returning_bad_bytes() {
        let mut jar_bytes = pool_sample_stored_jar_bytes();

        // Corrupt one byte inside the entry's stored data (past the local
        // file header + "PoolSample.class" name, well before the central
        // directory) so the CRC-32 declared for it no longer matches.
        jar_bytes[100] ^= 0xFF;

        let archive = ZipArchive::open(jar_bytes).unwrap();

        let error = archive
            .read_entry("PoolSample.class")
            .expect_err("changed bytes must fail CRC validation");
        assert!(matches!(error, ZipError::CrcMismatch { expected, actual } if expected != actual));
        assert_eq!(error.code(), "classpath.zip.crc_mismatch");
    }
}
