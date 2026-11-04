use dotty::jvm_tasty_artifacts::{PairedJvmTastyBatchArtifacts, emit_paired_jvm_tasty_batch};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::ConstantPoolIndex;
use dotty_classfile::reader::Reader;
use dotty_core::SourceId;
use dotty_tasty::ast::{
    BLOCK_TAG, DEFDEF_TAG, SHAREDTYPE_TAG, TERMREFDIRECT_TAG, TYPEREF_TAG, VALDEF_TAG,
};
use dotty_tasty::reader::Reader as TastyReader;
use dotty_tasty::section::StandardSection;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty::term::{RawTree, TermValue};
use dotty_tasty::writer::Writer;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_IMMUTABLE_REFERENCE_LOCAL_PAIR_CONSUMER";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const SOURCES: [&str; 2] = [
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactory.scala"),
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/PairFactoryReversed.scala"),
];
const CLIENTS: [&str; 2] = [
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/Consumer.scala"),
    include_str!("fixtures/jvm_tasty_immutable_reference_local_pair/ConsumerReversed.scala"),
];

fn required() -> bool {
    match std::env::var(REQUIRE).as_deref() {
        Ok("1") => true,
        Ok(value) => panic!("{REQUIRE} must equal 1 when set, got {value:?}"),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => panic!("read {REQUIRE}: {error}"),
    }
}

fn output(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"));
    assert!(
        result.status.success(),
        "{rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn hash(path: &Path) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn plain(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut escape = false;
    for ch in text.chars() {
        if escape {
            if ch == 'm' {
                escape = false;
            }
        } else if ch == '\u{1b}' {
            escape = true;
        } else {
            result.push(ch);
        }
    }
    result
}

fn task_dir() -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("TMPDIR").expect("TMPDIR must be task-local in required mode"),
    );
    fs::create_dir_all(&root).unwrap();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    root.join(format!("dotty-issue-2075-{}-{suffix}", std::process::id()))
}

struct Scala {
    compiler: PathBuf,
    java: PathBuf,
    java_home: PathBuf,
    libraries: String,
}

impl Scala {
    fn required() -> Self {
        let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT required"));
        let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME required"));
        let temp = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR required"));
        let java_options = format!("-XX:-UsePerfData -Djava.io.tmpdir={}", temp.display());
        let maven = PathBuf::from(
            std::env::var_os("COURSIER_MAVEN_ROOT").expect("COURSIER_MAVEN_ROOT required"),
        );
        let libraries = [
            (
                "org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar",
                "9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba",
            ),
            (
                "org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar",
                "b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd",
            ),
        ]
        .map(|(relative, expected)| {
            let path = maven.join(relative);
            assert!(
                path.is_file(),
                "approved Scala runtime exists: {}",
                path.display()
            );
            assert_eq!(
                hash(&path),
                expected,
                "approved runtime hash {}",
                path.display()
            );
            path
        });
        let revision = output(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "HEAD"]),
        );
        assert_eq!(
            String::from_utf8_lossy(&revision.stdout).trim(),
            SCALA_REVISION
        );
        let compiler = root.join("dist/target/universal/stage/bin/scalac");
        let compiler_jar = root.join("dist/target/universal/stage/lib/with_compiler.jar");
        assert!(
            compiler_jar.is_file(),
            "pinned staged Scala compiler jar exists: {}",
            compiler_jar.display()
        );
        let version = output(
            Command::new(&compiler)
                .env("JAVA_HOME", &java_home)
                .env("JAVA_TOOL_OPTIONS", &java_options)
                .arg("-version"),
        );
        assert!(
            format!(
                "{}{}",
                String::from_utf8_lossy(&version.stdout),
                String::from_utf8_lossy(&version.stderr)
            )
            .contains("3.9.0-bin-SNAPSHOT-git-777528f")
        );
        let java = java_home.join("bin/java");
        let jdk = output(
            Command::new(&java)
                .env("JAVA_TOOL_OPTIONS", &java_options)
                .arg("-version"),
        );
        assert!(
            format!(
                "{}{}",
                String::from_utf8_lossy(&jdk.stdout),
                String::from_utf8_lossy(&jdk.stderr)
            )
            .contains("21.0.13")
        );
        assert_eq!(
            hash(&java),
            "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155"
        );
        eprintln!(
            "SCALA_PROVENANCE revision={SCALA_REVISION} compiler={} compiler_sha256={} (receipt only; not pinned) java={} java_sha256={}",
            version
                .stdout
                .iter()
                .chain(version.stderr.iter())
                .copied()
                .map(char::from)
                .collect::<String>()
                .trim(),
            hash(&compiler_jar),
            java.display(),
            hash(&java)
        );
        Self {
            compiler,
            java,
            java_home,
            libraries: libraries
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.compiler);
        let tmp = std::env::var_os("TMPDIR").unwrap();
        command
            .env("JAVA_HOME", &self.java_home)
            .env("TMPDIR", &tmp)
            .env(
                "JAVA_TOOL_OPTIONS",
                format!(
                    "-XX:-UsePerfData -Djava.io.tmpdir={}",
                    PathBuf::from(tmp).display()
                ),
            );
        command
    }

    fn compile(&self, cp: &str, out: &Path, source: &Path) {
        output(
            self.command()
                .args(["-Werror", "-release", "21", "-classpath"])
                .arg(cp)
                .arg("-d")
                .arg(out)
                .arg(source),
        );
    }

    fn compile_result(&self, cp: &str, out: &Path, source: &Path) -> Output {
        self.command()
            .args(["-Werror", "-release", "21", "-classpath"])
            .arg(cp)
            .arg("-d")
            .arg(out)
            .arg(source)
            .output()
            .expect("run pinned Scala lookup probe")
    }

    fn run_from_tasty(&self, cp: &str, out: &Path, tasty: &Path) -> Output {
        self.command()
            .args(["-release", "21", "-from-tasty", "-classpath"])
            .arg(cp)
            .arg("-d")
            .arg(out)
            .arg(tasty)
            .output()
            .expect("run pinned Scala from-TASTy probe")
    }
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("decode Rust classfile");
    assert!(reader.is_at_end());
    class
}

fn without_method(bytes: &[u8], method: &str, descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let before = class.methods.len();
    class.methods.retain(|candidate| {
        pool.utf8(candidate.name_index).unwrap() != method
            || pool.utf8(candidate.descriptor_index).unwrap() != descriptor
    });
    assert_eq!(
        class.methods.len() + 1,
        before,
        "masked {method}{descriptor}"
    );
    class.encode().expect("encode classfile with masked member")
}

fn without_tasty_marker(bytes: &[u8]) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let before = class.attributes.len();
    class.attributes.retain(|attribute| match attribute {
        Attribute::Other(raw) => pool.utf8(raw.name_index).unwrap() != "TASTY",
        _ => true,
    });
    assert_eq!(class.attributes.len() + 1, before, "class has TASTY marker");
    class
        .encode()
        .expect("encode classfile without TASTY marker")
}

fn wrong_method_descriptor(bytes: &[u8], name: &str, descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let replacement = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| class.constant_pool.utf8(*index).ok() == Some("()V"))
        .expect("classfile contains a ()V descriptor");
    let pool = &class.constant_pool;
    let method = class
        .methods
        .iter_mut()
        .find(|method| {
            pool.utf8(method.name_index).ok() == Some(name)
                && pool.utf8(method.descriptor_index).ok() == Some(descriptor)
        })
        .expect("target method exists");
    method.descriptor_index = replacement;
    class
        .encode()
        .expect("encode classfile with invalid ABI descriptor")
}

fn local_address(tasty_bytes: &[u8], method_name: &str) -> u32 {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).expect("parse Rust TASTy");
    let index = tasty.ast_address_index().unwrap();
    let method = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|node| node.decode_defdef_body().ok())
                .and_then(|body| {
                    tasty
                        .names()
                        .render(body.name)
                        .ok()
                        .map(|name| name == method_name)
                })
                .unwrap_or(false)
        })
        .expect("factory method exists")
        .offset as u32;
    let block = index
        .iter_nodes_with_tag(BLOCK_TAG)
        .find(|node| {
            std::iter::successors(Some(*node), |current| {
                index.parent_of(current.offset as u32)
            })
            .any(|parent| parent.offset as u32 == method)
        })
        .expect("factory body is BLOCK")
        .offset as u32;
    index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            index
                .parent_of(node.offset as u32)
                .is_some_and(|parent| parent.offset as u32 == block)
        })
        .expect("BLOCK owns the immutable local VALDEF")
        .offset as u32
}

fn block_address(tasty_bytes: &[u8], method_name: &str) -> u32 {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).expect("parse Rust TASTy");
    let index = tasty.ast_address_index().unwrap();
    let local = local_address(tasty_bytes, method_name);
    index
        .parent_of(local)
        .filter(|parent| parent.tag == BLOCK_TAG)
        .expect("local VALDEF is owned by its method BLOCK")
        .offset as u32
}

fn direct_termref_bytes(address: u32) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.write_nat(address);
    let mut bytes = vec![TERMREFDIRECT_TAG];
    bytes.extend(writer.into_inner());
    bytes
}

fn change_termref_target(tasty_bytes: &[u8], address: u32, replacement: u32) -> Vec<u8> {
    let reference = direct_termref_bytes(address);
    let mut writer = Writer::default();
    writer.write_nat(replacement);
    let replacement = writer.into_inner();
    assert_eq!(
        reference.len() - 1,
        replacement.len(),
        "mutated address retains encoded width"
    );
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let asts = tasty.section(StandardSection::Asts).unwrap();
    let matches = asts
        .payload
        .windows(reference.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == reference).then_some(offset))
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one direct local reference");
    let mut changed = tasty_bytes.to_vec();
    let offset = asts.offset + matches[0] + 1;
    changed[offset..offset + replacement.len()].copy_from_slice(&replacement);
    changed
}

fn change_local_type_ref(
    tasty_bytes: &[u8],
    method_name: &str,
    reference_index: usize,
    replacement_name: &str,
) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).expect("parse Rust TASTy");
    let index = tasty.ast_address_index().unwrap();
    let section = tasty.section(StandardSection::Asts).unwrap();
    let local = local_address(tasty_bytes, method_name);
    let replacement = tasty
        .names()
        .find_utf8(replacement_name)
        .unwrap_or_else(|| panic!("TASTy name table has {replacement_name}"));
    let mut references = index
        .iter_nodes_with_tag(TYPEREF_TAG)
        .filter(|node| {
            std::iter::successors(Some(*node), |current| {
                index.parent_of(current.offset as u32)
            })
            .any(|parent| parent.offset as u32 == local)
        })
        .filter_map(|node| {
            let mut reader =
                TastyReader::with_range(section.payload, node.offset, section.payload.len())
                    .ok()?;
            match RawTree::decode(&mut reader).ok()? {
                RawTree::NatAst {
                    tag: TYPEREF_TAG,
                    value,
                    ..
                } => Some((node.offset, value, tasty.names().render(value).ok()?)),
                _ => None,
            }
        })
        .filter(|(_, _, name)| name.ends_with("Counter"))
        .collect::<Vec<_>>();
    references.sort_by_key(|(offset, _, _)| *offset);
    let (offset, original, _) = *references
        .get(reference_index)
        .unwrap_or_else(|| panic!("local VALDEF has Counter type reference #{reference_index}"));
    let mut original_encoded = Writer::default();
    original_encoded.write_nat(original);
    let mut replacement_encoded = Writer::default();
    replacement_encoded.write_nat(replacement);
    let original_encoded = original_encoded.into_inner();
    let replacement_encoded = replacement_encoded.into_inner();
    assert_eq!(
        original_encoded.len(),
        replacement_encoded.len(),
        "mutated type name retains encoded width"
    );
    let mut changed = tasty_bytes.to_vec();
    let start = section.offset + offset + 1;
    changed[start..start + replacement_encoded.len()].copy_from_slice(&replacement_encoded);
    TastyFile::parse_and_validate_scala_3_9(&changed)
        .expect("type-mutated TASTy remains structurally valid");
    changed
}

fn change_initializer_constructor(tasty_bytes: &[u8], method_name: &str) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).expect("parse Rust TASTy");
    let index = tasty.ast_address_index().unwrap();
    let section = tasty.section(StandardSection::Asts).unwrap();
    let local = local_address(tasty_bytes, method_name);
    let pair_type = index
        .iter_nodes_with_tag(TYPEREF_TAG)
        .find_map(|node| {
            let mut reader =
                TastyReader::with_range(section.payload, node.offset, section.payload.len())
                    .ok()?;
            match RawTree::decode(&mut reader).ok()? {
                RawTree::NatAst {
                    tag: TYPEREF_TAG,
                    value,
                    ..
                } if tasty
                    .names()
                    .render(value)
                    .ok()
                    .is_some_and(|name| name.ends_with("Pair")) =>
                {
                    Some(node.offset as u32)
                }
                _ => None,
            }
        })
        .expect("method TASTy has the Pair result type reference");
    let shared_type = index
        .iter_nodes_with_tag(SHAREDTYPE_TAG)
        .find(|node| {
            std::iter::successors(Some(*node), |current| {
                index.parent_of(current.offset as u32)
            })
            .any(|parent| parent.offset as u32 == local)
        })
        .expect("initializer has a shared type reference");
    let mut reader =
        TastyReader::with_range(section.payload, shared_type.offset, section.payload.len())
            .expect("valid shared-type reference range");
    let RawTree::Leaf(reference) = RawTree::decode(&mut reader).expect("decode shared type") else {
        panic!("SHAREDTYPE is a category-two reference")
    };
    let TermValue::AstRef(original) = reference.value else {
        panic!("SHAREDTYPE carries an AST address")
    };
    let mut original_encoded = Writer::default();
    original_encoded.write_nat(original);
    let mut replacement_encoded = Writer::default();
    replacement_encoded.write_nat(pair_type);
    let original_encoded = original_encoded.into_inner();
    let replacement_encoded = replacement_encoded.into_inner();
    assert_eq!(original_encoded.len(), replacement_encoded.len());
    let mut changed = tasty_bytes.to_vec();
    let start = section.offset + shared_type.offset + 1;
    changed[start..start + replacement_encoded.len()].copy_from_slice(&replacement_encoded);
    TastyFile::parse_and_validate_scala_3_9(&changed)
        .expect("constructor-mutated TASTy remains structurally valid");
    changed
}

fn relink_tasty_uuid(tasty_bytes: &[u8], original: &[u8]) -> Vec<u8> {
    let generated_uuid = TastyFile::parse_and_validate_scala_3_9(tasty_bytes)
        .unwrap()
        .header()
        .uuid;
    let original_uuid = TastyFile::parse_and_validate_scala_3_9(original)
        .unwrap()
        .header()
        .uuid;
    let mut bytes = tasty_bytes.to_vec();
    let matches = bytes
        .windows(generated_uuid.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == generated_uuid).then_some(offset))
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "TASTy header UUID appears once");
    bytes[matches[0]..matches[0] + generated_uuid.len()].copy_from_slice(&original_uuid);
    TastyFile::parse_and_validate_scala_3_9(&bytes).expect("relinked TASTy is structurally valid");
    bytes
}

fn write_batch(dir: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for artifact in batch.classes() {
        for (path, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }
}

fn assert_same_batch(a: &PairedJvmTastyBatchArtifacts, b: &PairedJvmTastyBatchArtifacts) {
    assert_eq!(a.classes().len(), 3);
    assert_eq!(b.classes().len(), 3);
    for (x, y) in a.classes().iter().zip(b.classes()) {
        assert_eq!(x.classfile_path(), y.classfile_path());
        assert_eq!(x.classfile_bytes(), y.classfile_bytes());
        assert_eq!(x.tasty_bytes(), y.tasty_bytes());
        assert_eq!(x.tasty_link_id(), y.tasty_link_id());
    }
}

fn compile_and_run(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
    source: &str,
    client: &str,
    entry: &str,
    source_id: u32,
) {
    let repeated = emit_paired_jvm_tasty_batch(
        source,
        "PairFactory.scala",
        SourceId::from_index(source_id + 1),
    )
    .expect("same source in an independent Rust session");
    assert_same_batch(batch, &repeated);
    let producer = root.join(format!("original-{source_id}"));
    let client_out = root.join(format!("client-{source_id}"));
    fs::create_dir_all(&producer).unwrap();
    fs::create_dir_all(&client_out).unwrap();
    write_batch(&producer, batch);
    let originals = batch
        .classes()
        .iter()
        .flat_map(|a| {
            [
                (a.classfile_path(), a.classfile_bytes()),
                (a.tasty_path(), a.tasty_bytes()),
            ]
        })
        .map(|(p, b)| (p.to_owned(), hash(&producer.join(p)), b.to_vec()))
        .collect::<Vec<_>>();
    for artifact in batch.classes() {
        eprintln!(
            "RUST_ARTIFACT_SHA256 {} {}",
            artifact.classfile_path(),
            hash(&producer.join(artifact.classfile_path()))
        );
        eprintln!(
            "RUST_ARTIFACT_SHA256 {} {}",
            artifact.tasty_path(),
            hash(&producer.join(artifact.tasty_path()))
        );
        eprintln!(
            "RUST_ARTIFACT_LINK {} {:?}",
            artifact.classfile_path(),
            artifact.tasty_link_id()
        );
    }
    let producer_source = root.join(format!("Producer-{source_id}.scala"));
    let client_source = root.join(format!("Consumer-{source_id}.scala"));
    fs::write(&producer_source, source).unwrap();
    fs::write(&client_source, client).unwrap();
    eprintln!(
        "RUST_PRODUCER_SOURCE_SHA256 {} {}",
        producer_source.display(),
        hash(&producer_source)
    );
    eprintln!(
        "SCALA_CLIENT_SHA256 {} {}",
        client_source.display(),
        hash(&client_source)
    );
    scala.compile(
        &format!("{}:{}", producer.display(), scala.libraries),
        &client_out,
        &client_source,
    );
    let cp = format!(
        "{}:{}:{}",
        client_out.display(),
        producer.display(),
        scala.libraries
    );
    let runtime = output(
        Command::new(&scala.java)
            .env(
                "JAVA_TOOL_OPTIONS",
                format!(
                    "-XX:-UsePerfData -Djava.io.tmpdir={}",
                    std::env::var_os("TMPDIR").unwrap().to_string_lossy()
                ),
            )
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(&cp)
            .arg(entry),
    );
    let stdout = String::from_utf8_lossy(&runtime.stdout);
    assert!(
        stdout.contains("scala39-immutable-reference-local-pair"),
        "{stdout}"
    );
    for (path, digest, bytes) in &originals {
        let actual = producer.join(path);
        assert_eq!(
            hash(&actual),
            *digest,
            "original Rust artifact {path} remains unchanged"
        );
        assert_eq!(fs::read(actual).unwrap(), *bytes);
    }
    eprintln!("RUST_ORIGINALS_UNCHANGED after Scala compile/runtime; {stdout}");
}

fn inspect_original_sidecars(scala: &Scala, root: &Path, producer: &Path, reversed: bool) {
    let originals = [
        "Counter.class",
        "Counter.tasty",
        "Pair.class",
        "Pair.tasty",
        "PairFactory.class",
        "PairFactory.tasty",
    ]
    .map(|path| (path, hash(&producer.join("example").join(path))));
    for class in ["Counter", "Pair", "PairFactory"] {
        let tasty = producer.join(format!("example/{class}.tasty"));
        let decompile = output(scala.command().arg("-decompile").arg(&tasty));
        let printed = output(scala.command().arg("-print-tasty").arg(&tasty));
        let text = String::from_utf8_lossy(&printed.stdout);
        assert!(text.contains("version: 28.9.0"), "{class}: {text}");
        let source = plain(&String::from_utf8_lossy(&decompile.stdout));
        if class == "Counter" {
            assert!(
                source.contains("def get") && source.contains("def set"),
                "{source}"
            );
        }
        if class == "Pair" {
            assert!(
                source.contains("def leftValue") && source.contains("def rightValue"),
                "{source}"
            );
        }
        if class == "PairFactory" {
            let dump = source;
            assert!(dump.contains("val fresh: example.Counter"), "{dump}");
            assert!(dump.contains("new example.Counter(seed)"), "{dump}");
            assert!(
                dump.contains("new example.Pair(existing, fresh)")
                    || dump.contains("new example.Pair(fresh, existing)"),
                "{dump}"
            );
            let dump = plain(&text);
            for token in ["BLOCK", "VALDEF", "PARAM(", "TERMREFdirect", "<init>"] {
                assert!(dump.contains(token), "{class} TASTy lacks {token}: {dump}");
            }
            let valdefs = dump
                .lines()
                .filter(|line| line.contains("VALDEF("))
                .collect::<Vec<_>>();
            assert_eq!(valdefs.len(), 1, "PairFactory has one local VALDEF: {dump}");
            let direct_refs = dump
                .lines()
                .filter_map(|line| {
                    line.split_once("TERMREFdirect")
                        .map(|(_, target)| target.trim().to_owned())
                })
                .collect::<Vec<_>>();
            let expected = if reversed {
                vec!["74".to_owned(), "32".to_owned(), "41".to_owned()]
            } else {
                vec!["32".to_owned(), "74".to_owned(), "41".to_owned()]
            };
            assert_eq!(
                direct_refs, expected,
                "formal/local/seed TERMREF addresses: {dump}"
            );
            assert!(
                valdefs[0].contains("74:") && valdefs[0].contains("[fresh]"),
                "fresh declaration is at address 74: {dump}"
            );
        }
    }
    let reconstructed = root.join("from-tasty-inspection-only");
    fs::create_dir_all(&reconstructed).unwrap();
    let sidecars = ["Counter", "Pair", "PairFactory"]
        .map(|name| producer.join(format!("example/{name}.tasty")));
    output(
        scala
            .command()
            .args(["-release", "21", "-from-tasty", "-classpath"])
            .arg(format!("{}:{}", producer.display(), scala.libraries))
            .arg("-d")
            .arg(&reconstructed)
            .args(&sidecars),
    );
    for class in ["Counter", "Pair", "PairFactory"] {
        assert!(
            reconstructed
                .join(format!("example/{class}.class"))
                .is_file()
        );
    }
    for (path, expected) in originals {
        assert_eq!(
            hash(&producer.join("example").join(path)),
            expected,
            "Scala inspection preserves original {path}"
        );
    }
}

fn lookup_probes(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
    method: &str,
    method_desc: &str,
) {
    let cases = [
        (
            "counter-constructor",
            "example/Counter.class",
            "<init>",
            "(I)V",
            "new Counter(1)",
            "[E173]",
        ),
        (
            "counter-get",
            "example/Counter.class",
            "get",
            "()I",
            "new Counter(1).get()",
            "[E008]",
        ),
        (
            "counter-set",
            "example/Counter.class",
            "set",
            "(I)V",
            "new Counter(1).set(2)",
            "[E008]",
        ),
        (
            "pair-left",
            "example/Pair.class",
            "leftValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).leftValue()",
            "[E008]",
        ),
        (
            "pair-right",
            "example/Pair.class",
            "rightValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).rightValue()",
            "[E008]",
        ),
        (
            "pair-constructor",
            "example/Pair.class",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
            "new Pair(new Counter(1), new Counter(2))",
            "[E173]",
        ),
        (
            "factory-method",
            "example/PairFactory.class",
            "",
            "",
            "",
            "[E008]",
        ),
    ];
    for (label, class_path, member, descriptor, expression, error_code) in cases {
        let (member, descriptor, expression) = if label == "factory-method" {
            (
                method,
                method_desc,
                format!("new PairFactory().{method}(new Counter(1), 2)"),
            )
        } else {
            (member, descriptor, expression.to_owned())
        };
        let case = root.join(format!("lookup-{label}"));
        let with_tasty = case.join("tasty-backed");
        let class_only = case.join("classfile-only");
        let accepted = case.join("accepted");
        let rejected = case.join("rejected");
        for dir in [&with_tasty, &class_only, &accepted, &rejected] {
            fs::create_dir_all(dir).unwrap();
        }
        write_batch(&with_tasty, batch);
        write_batch(&class_only, batch);
        let path = with_tasty.join(class_path);
        let masked = without_method(&fs::read(&path).unwrap(), member, descriptor);
        fs::write(&path, masked).unwrap();
        let path = class_only.join(class_path);
        let masked = without_method(&fs::read(&path).unwrap(), member, descriptor);
        fs::write(&path, without_tasty_marker(&masked)).unwrap();
        fs::remove_file(class_only.join(class_path.replace(".class", ".tasty"))).unwrap();
        let source = case.join("Lookup.scala");
        fs::write(
            &source,
            format!("package example\nobject Lookup {{ val selected = {{ {expression} }} }}\n"),
        )
        .unwrap();
        let accepted_result = scala.compile_result(
            &format!("{}:{}", with_tasty.display(), scala.libraries),
            &accepted,
            &source,
        );
        assert!(
            accepted_result.status.success(),
            "TASTy lookup failed for {label}: {}",
            String::from_utf8_lossy(&accepted_result.stderr)
        );
        let rejected_result = scala.compile_result(
            &format!("{}:{}", class_only.display(), scala.libraries),
            &rejected,
            &source,
        );
        assert!(
            !rejected_result.status.success(),
            "classfile-only control resolved {label}"
        );
        let stderr = plain(&String::from_utf8_lossy(&rejected_result.stderr));
        assert!(
            stderr.contains(error_code),
            "unexpected first classfile-only error for {label}: {stderr}"
        );
        eprintln!(
            "TASTY_LOOKUP_ACCEPTED {label}; CLASSFILE_ONLY_FIRST_REJECTION {}",
            stderr
                .lines()
                .find(|line| line.contains(error_code))
                .unwrap_or(&stderr)
        );
    }
}

fn sidecar_negative_probes(scala: &Scala, root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for class in ["Counter", "Pair", "PairFactory"] {
        for corruption in ["missing", "misplaced", "corrupt", "mismatched-uuid"] {
            let label = format!("{class}-{corruption}");
            let case = root.join(format!("negative-{label}"));
            let classes = case.join("classes");
            let out = case.join("out");
            fs::create_dir_all(&classes).unwrap();
            fs::create_dir_all(&out).unwrap();
            write_batch(&classes, batch);
            let class_path = format!("example/{class}.class");
            let tasty_path = format!("example/{class}.tasty");
            let tasty = classes.join(&tasty_path);
            match corruption {
                "missing" => fs::remove_file(&tasty).unwrap(),
                "misplaced" => {
                    fs::rename(&tasty, classes.join(format!("{tasty_path}.moved"))).unwrap()
                }
                "corrupt" => {
                    let mut bytes = fs::read(&tasty).unwrap();
                    bytes.truncate(bytes.len().saturating_sub(3));
                    fs::write(tasty, bytes).unwrap();
                }
                "mismatched-uuid" => {
                    let uuid = TastyFile::parse_and_validate_scala_3_9(&fs::read(&tasty).unwrap())
                        .unwrap()
                        .header()
                        .uuid;
                    let mut bytes = fs::read(classes.join(&class_path)).unwrap();
                    let offset = bytes
                        .windows(uuid.len())
                        .position(|window| window == uuid)
                        .expect("classfile carries sidecar UUID");
                    bytes[offset] ^= 0x80;
                    fs::write(classes.join(&class_path), bytes).unwrap();
                }
                _ => unreachable!(),
            }
            let source = case.join("Consumer.scala");
            fs::write(&source, CLIENTS[0]).unwrap();
            let result = scala.compile_result(
                &format!("{}:{}", classes.display(), scala.libraries),
                &out,
                &source,
            );
            assert!(!result.status.success(), "Scala accepted {label}");
            let stderr = plain(&String::from_utf8_lossy(&result.stderr));
            let expected = match corruption {
                "missing" | "misplaced" => ".tasty` file is missing",
                "corrupt" => "is broken",
                "mismatched-uuid" => "out of sync with its TASTy file",
                _ => unreachable!(),
            };
            assert!(
                stderr.contains(expected),
                "wrong first Scala failure for {label}: {stderr}"
            );
            eprintln!(
                "SCALA_NEGATIVE_FIRST_FAILURE {label}: {}",
                stderr
                    .lines()
                    .find(|line| line.contains(expected))
                    .unwrap_or(&stderr)
            );
        }
    }
}

fn malformed_local_reference_probe(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
) {
    let case = root.join("negative-dangling-fresh-termref");
    let classes = case.join("classes");
    fs::create_dir_all(&classes).unwrap();
    write_batch(&classes, batch);
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    let target = local_address(factory.tasty_bytes(), "create");
    fs::write(
        classes.join(factory.tasty_path()),
        change_termref_target(factory.tasty_bytes(), target, 127),
    )
    .unwrap();
    let client = case.join("Consumer.scala");
    let out = case.join("client-out");
    fs::create_dir_all(&out).unwrap();
    fs::write(&client, CLIENTS[0]).unwrap();
    let lookup = scala.compile_result(
        &format!("{}:{}", classes.display(), scala.libraries),
        &out,
        &client,
    );
    assert!(
        lookup.status.success(),
        "signature lookup forced malformed method body: {}",
        String::from_utf8_lossy(&lookup.stderr)
    );
    let decompiled = scala
        .command()
        .arg("-decompile")
        .arg(classes.join(factory.tasty_path()))
        .output()
        .expect("run Scala reader against dangling local address");
    assert!(
        !decompiled.status.success(),
        "Scala -decompile accepted dangling fresh TERMREF"
    );
    let stderr = plain(&String::from_utf8_lossy(&decompiled.stderr));
    assert!(
        stderr.contains("TreeWithoutOwner"),
        "unexpected local-reference failure: {stderr}"
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE dangling-fresh-termref lazy-signature-lookup=accepted; -decompile/readTasty=TreeWithoutOwner"
    );
}

fn malformed_local_body_probe(scala: &Scala, root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    let local = local_address(factory.tasty_bytes(), "create");
    let block = block_address(factory.tasty_bytes(), "create");
    let cases = [
        (
            "wrong-local-type",
            change_local_type_ref(factory.tasty_bytes(), "create", 0, "Pair"),
            "from-TASTy",
        ),
        (
            "wrong-local-owner",
            change_termref_target(factory.tasty_bytes(), local, block),
            "readTasty",
        ),
        (
            "wrong-initializer-constructor",
            change_initializer_constructor(factory.tasty_bytes(), "create"),
            "from-TASTy",
        ),
    ];
    for (label, changed_tasty, stage) in cases {
        let case = root.join(format!("negative-{label}"));
        let classes = case.join("classes");
        let lookup_out = case.join("lookup-out");
        let reconstructed = case.join("reconstructed");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&lookup_out).unwrap();
        fs::create_dir_all(&reconstructed).unwrap();
        write_batch(&classes, batch);
        fs::write(classes.join(factory.tasty_path()), changed_tasty).unwrap();
        let client = case.join("Consumer.scala");
        fs::write(&client, CLIENTS[0]).unwrap();
        let classpath = format!("{}:{}", classes.display(), scala.libraries);
        let lookup = scala.compile_result(&classpath, &lookup_out, &client);
        assert!(
            lookup.status.success(),
            "{label} unexpectedly affected lazy signature lookup: {}",
            String::from_utf8_lossy(&lookup.stderr)
        );
        let result = if stage == "readTasty" {
            scala
                .command()
                .arg("-decompile")
                .arg(classes.join(factory.tasty_path()))
                .output()
                .expect("run Scala reader against wrong-owner local address")
        } else {
            scala.run_from_tasty(
                &classpath,
                &reconstructed,
                &classes.join(factory.tasty_path()),
            )
        };
        if stage == "readTasty" {
            assert!(
                !result.status.success(),
                "Scala accepted malformed local owner negative {label}"
            );
        } else if result.status.success() {
            let decompiled = scala
                .command()
                .arg("-decompile")
                .arg(classes.join(factory.tasty_path()))
                .output()
                .expect("run Scala decompiler on a type-corrupted local body");
            assert!(
                decompiled.status.success(),
                "from-TASTy accepted {label} but decompile failed: {}",
                String::from_utf8_lossy(&decompiled.stderr)
            );
            let dump = plain(&String::from_utf8_lossy(&decompiled.stdout));
            let observed = if label == "wrong-local-type" {
                dump.lines()
                    .find(|line| line.contains("fresh"))
                    .unwrap_or(&dump)
            } else {
                dump.lines()
                    .find(|line| line.contains("new Pair"))
                    .unwrap_or(&dump)
            };
            assert!(
                observed.contains("Pair"),
                "mutated local body was not visible to official Scala: {label}: {dump}"
            );
            eprintln!(
                "SCALA_NEGATIVE_FIRST_OUTCOME {label} lazy-signature-lookup=accepted; -from-tasty=accepted; -decompile=accepted with mutated body: {observed}"
            );
            continue;
        }
        let stderr = plain(&String::from_utf8_lossy(&result.stderr));
        let first = stderr
            .lines()
            .find(|line| {
                line.contains("TreeWithoutOwner")
                    || line.contains("[E")
                    || line.contains("AssertionError")
            })
            .unwrap_or(&stderr);
        match label {
            "wrong-initializer-constructor" => assert!(
                first.contains("[E007]")
                    || first.contains("[E171]")
                    || first.contains("TreeWithoutOwner")
                    || stderr.contains("Unexpected New") && stderr.contains("GenBCode"),
                "unexpected {stage} failure for {label}: {stderr}"
            ),
            "wrong-local-owner" => assert!(
                first.contains("TreeWithoutOwner"),
                "unexpected {stage} failure for {label}: {stderr}"
            ),
            _ => unreachable!(),
        }
        eprintln!(
            "SCALA_NEGATIVE_FIRST_FAILURE {label} lazy-signature-lookup=accepted; {stage}: {first}"
        );
    }
}

fn factory_signature_negative_probes(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
) {
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .unwrap();
    let original_tasty = batch
        .classes()
        .iter()
        .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    let cases = [
        (
            "wrong-formal-count",
            "package example\nclass PairFactory:\n  def create(existing: Counter): Pair = new Pair(existing, new Counter(2))\n",
        ),
        (
            "wrong-formal-type",
            "package example\nclass PairFactory:\n  def create(existing: String, seed: Int): Pair = new Pair(new Counter(seed), new Counter(seed))\n",
        ),
        (
            "wrong-result-type",
            "package example\nclass PairFactory:\n  def create(existing: Counter, seed: Int): Counter = new Counter(seed)\n",
        ),
    ];
    let client = "package example\nobject Probe:\n  val pair: Pair = new PairFactory().create(new Counter(7), 8)\n";
    for (label, control_source) in cases {
        let case = root.join(format!("negative-factory-{label}"));
        let dependencies = case.join("dependencies");
        let generated = case.join("generated");
        let classes = case.join("classes");
        let out = case.join("consumer-out");
        for dir in [&dependencies, &generated, &classes, &out] {
            fs::create_dir_all(dir).unwrap();
        }
        write_batch(&dependencies, batch);
        fs::remove_file(dependencies.join("example/PairFactory.class")).unwrap();
        fs::remove_file(dependencies.join("example/PairFactory.tasty")).unwrap();
        let source = case.join("FactoryControl.scala");
        fs::write(&source, control_source).unwrap();
        scala.compile(
            &format!("{}:{}", dependencies.display(), scala.libraries),
            &generated,
            &source,
        );
        let generated_tasty = fs::read(generated.join("example/PairFactory.tasty")).unwrap();
        write_batch(&classes, batch);
        fs::write(
            classes.join("example/PairFactory.tasty"),
            relink_tasty_uuid(&generated_tasty, original_tasty.tasty_bytes()),
        )
        .unwrap();
        assert_eq!(
            fs::read(classes.join("example/PairFactory.class")).unwrap(),
            factory.classfile_bytes(),
            "{label} leaves the Rust JVM ABI unchanged"
        );
        let client_path = case.join("Consumer.scala");
        fs::write(&client_path, client).unwrap();
        let result = scala.compile_result(
            &format!("{}:{}", classes.display(), scala.libraries),
            &out,
            &client_path,
        );
        assert!(!result.status.success(), "Scala accepted {label}");
        let stderr = plain(&String::from_utf8_lossy(&result.stderr));
        let diagnostic = stderr
            .lines()
            .find(|line| line.contains("[E"))
            .unwrap_or(&stderr);
        assert!(
            diagnostic.contains("[E007]"),
            "wrong first failure for {label}: {stderr}"
        );
        eprintln!("SCALA_NEGATIVE_FIRST_FAILURE {label}: {diagnostic}");
        assert_eq!(
            fs::read(classes.join("example/PairFactory.class")).unwrap(),
            factory.classfile_bytes(),
            "Scala consumer compilation preserves the original Factory classfile"
        );
    }
}

fn abi_descriptor_negative_probes(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
) {
    let cases = [
        ("counter-get", "example/Counter.class", "get", "()I"),
        ("pair-leftValue", "example/Pair.class", "leftValue", "()I"),
        (
            "factory-create",
            "example/PairFactory.class",
            "create",
            "(Lexample/Counter;I)Lexample/Pair;",
        ),
        (
            "pair-constructor",
            "example/Pair.class",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
        ),
    ];
    for (label, class_path, method, descriptor) in cases {
        let case = root.join(format!("negative-invalid-descriptor-{label}"));
        let classes = case.join("classes");
        let out = case.join("client-out");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&out).unwrap();
        write_batch(&classes, batch);
        let class_file = classes.join(class_path);
        let changed = wrong_method_descriptor(&fs::read(&class_file).unwrap(), method, descriptor);
        fs::write(&class_file, changed).unwrap();
        let client = case.join("Consumer.scala");
        fs::write(&client, CLIENTS[0]).unwrap();
        let compile = scala.compile_result(
            &format!("{}:{}", classes.display(), scala.libraries),
            &out,
            &client,
        );
        if !compile.status.success() {
            let diagnostic = plain(&String::from_utf8_lossy(&compile.stderr));
            eprintln!(
                "SCALA_NEGATIVE_FIRST_FAILURE invalid-descriptor-{label} at compile: {}",
                diagnostic
                    .lines()
                    .find(|line| line.contains("[E"))
                    .unwrap_or(&diagnostic)
            );
            continue;
        }
        let classpath = format!(
            "{}:{}:{}",
            out.display(),
            classes.display(),
            scala.libraries
        );
        let runtime = Command::new(&scala.java)
            .env(
                "JAVA_TOOL_OPTIONS",
                format!(
                    "-XX:-UsePerfData -Djava.io.tmpdir={}",
                    std::env::var_os("TMPDIR").unwrap().to_string_lossy()
                ),
            )
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("example.Consumer")
            .output()
            .expect("run negative ABI client");
        assert!(
            !runtime.status.success(),
            "Java accepted invalid descriptor {label}"
        );
        let stderr = plain(&String::from_utf8_lossy(&runtime.stderr));
        let first = stderr
            .lines()
            .find(|line| line.contains("Error") || line.contains("Exception"))
            .unwrap_or(&stderr);
        assert!(
            first.contains("VerifyError") || first.contains("NoSuchMethodError"),
            "unexpected first JVM ABI failure for {label}: {stderr}"
        );
        eprintln!(
            "SCALA_NEGATIVE_FIRST_FAILURE invalid-descriptor-{label} at Java -Xverify:all: {first}"
        );
    }
}

fn wrong_package_probe(scala: &Scala, root: &Path) {
    let case = root.join("negative-wrong-package");
    let classes = case.join("classes");
    let out = case.join("out");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&out).unwrap();
    let (wrong_package_source, _) = renamed_variant(SOURCES[0], CLIENTS[0]);
    let wrong_package_batch = emit_paired_jvm_tasty_batch(
        &wrong_package_source,
        "PairFactory.scala",
        SourceId::from_index(20790),
    )
    .expect("wrong-package producer is a genuine Rust session");
    assert!(
        wrong_package_batch
            .classes()
            .iter()
            .all(|artifact| artifact.classfile_path().starts_with("renamed/example/"))
    );
    write_batch(&classes, &wrong_package_batch);
    let client = case.join("WrongPackage.scala");
    fs::write(&client, CLIENTS[0]).unwrap();
    let result = scala.compile_result(
        &format!("{}:{}", classes.display(), scala.libraries),
        &out,
        &client,
    );
    assert!(
        !result.status.success(),
        "Scala accepted client references against a producer emitted in a different package"
    );
    let stderr = plain(&String::from_utf8_lossy(&result.stderr));
    let first = stderr
        .lines()
        .find(|line| line.contains("[E"))
        .unwrap_or(&stderr);
    assert!(
        first.contains("[E006]"),
        "wrong package failure stage: {stderr}"
    );
    eprintln!("SCALA_NEGATIVE_FIRST_FAILURE wrong-producer-package: {first}");
}

fn renamed_variant(source: &str, client: &str) -> (String, String) {
    let rename = |text: &str| {
        text.replace("package example", "package renamed.example")
            .replace("Counter", "Ledger")
            .replace("PairFactory", "Factory")
            .replace("Pair", "Duo")
            .replace("create", "makePair")
            .replace("fresh", "localValue")
    };
    (rename(source), rename(client))
}

fn reordered_variant(source: &str) -> String {
    let package_end = source.find("class Counter").unwrap();
    let counter_start = package_end;
    let pair_start = source.find("class Pair(").unwrap();
    let factory_start = source.find("class PairFactory:").unwrap();
    let counter = &source[counter_start..pair_start];
    let pair = &source[pair_start..factory_start];
    let factory = &source[factory_start..];
    format!(
        "{}{}\n{}\n{}",
        &source[..package_end],
        pair,
        factory,
        counter
    )
}

#[test]
fn official_scala_consumes_original_rust_immutable_local_pair_batch() {
    if !required() {
        return;
    }
    let scala = Scala::required();
    let root = task_dir();
    fs::create_dir_all(&root).unwrap();
    for i in 0..2 {
        let batch = emit_paired_jvm_tasty_batch(
            SOURCES[i],
            "PairFactory.scala",
            SourceId::from_index(20750 + i as u32 * 10),
        )
        .expect("authentic Rust paired batch");
        let source_id = 20750 + i as u32 * 10;
        compile_and_run(
            &scala,
            &root,
            &batch,
            SOURCES[i],
            CLIENTS[i],
            "example.Consumer",
            source_id,
        );
        let producer = root.join(format!("original-{source_id}"));
        inspect_original_sidecars(&scala, &root, &producer, i == 1);
        let (method, descriptor) = if i == 0 {
            ("create", "(Lexample/Counter;I)Lexample/Pair;")
        } else {
            ("createReversed", "(Lexample/Counter;I)Lexample/Pair;")
        };
        lookup_probes(&scala, &root, &batch, method, descriptor);
        if i == 0 {
            sidecar_negative_probes(&scala, &root, &batch);
            malformed_local_reference_probe(&scala, &root, &batch);
            malformed_local_body_probe(&scala, &root, &batch);
            factory_signature_negative_probes(&scala, &root, &batch);
            abi_descriptor_negative_probes(&scala, &root, &batch);
            wrong_package_probe(&scala, &root);
        }
    }
    let (renamed_source, renamed_client) = renamed_variant(SOURCES[0], CLIENTS[0]);
    let renamed = emit_paired_jvm_tasty_batch(
        &renamed_source,
        "PairFactory.scala",
        SourceId::from_index(20780),
    )
    .expect("renamed source uses a genuine new Rust session");
    compile_and_run(
        &scala,
        &root,
        &renamed,
        &renamed_source,
        &renamed_client,
        "renamed.example.Consumer",
        20780,
    );
    let reordered_source = reordered_variant(SOURCES[0]);
    let reordered = emit_paired_jvm_tasty_batch(
        &reordered_source,
        "PairFactory.scala",
        SourceId::from_index(20790),
    )
    .expect("reordered declarations use a genuine new Rust session");
    compile_and_run(
        &scala,
        &root,
        &reordered,
        &reordered_source,
        CLIENTS[0],
        "example.Consumer",
        20790,
    );
}

#[test]
fn immutable_local_pair_required_mode_does_not_skip_when_configuration_is_missing() {
    if std::env::var_os(REQUIRE).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    for key in ["SCALA39_ROOT", "JAVA_HOME", "COURSIER_MAVEN_ROOT", "TMPDIR"] {
        assert!(std::env::var_os(key).is_some(), "required mode needs {key}");
    }
    assert!(Scala::required().compiler.is_file());
}
