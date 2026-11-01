use dotty::jvm_tasty_artifacts::{PairedJvmTastyBatchArtifacts, emit_paired_jvm_tasty_batch};
use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::reader::Reader;
use dotty_core::SourceId;
use dotty_tasty::ast::{DEFDEF_TAG, PARAM_TAG, TERMREFDIRECT_TAG};
use dotty_tasty::section::StandardSection;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty::writer::Writer;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSTRUCTOR_CONSUMER";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");
const CLIENT: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/RustArtifactConsumer.scala");

fn required() -> bool {
    match std::env::var(REQUIRE).as_deref() {
        Ok("1") => true,
        Ok(value) => panic!("{REQUIRE} must equal 1 when set, got {value:?}"),
        Err(std::env::VarError::NotPresent) => {
            eprintln!("skipping strict Scala consumer; set {REQUIRE}=1 to require it");
            false
        }
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
        "command failed: {rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

fn task_dir() -> PathBuf {
    let root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).expect("create temporary root");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = root.join(format!("dotty-issue-1919-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create task-local root");
    path
}

fn sha256(path: &Path) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum emits a hash")
        .to_owned()
}

struct Scala {
    java_home: PathBuf,
    scalac: PathBuf,
    java: PathBuf,
    libraries: String,
}

impl Scala {
    fn from_environment() -> Self {
        let root =
            PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
        let java_home =
            PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
        let maven = PathBuf::from(
            std::env::var_os("COURSIER_MAVEN_ROOT").expect("COURSIER_MAVEN_ROOT is required"),
        );
        let libraries = [
            (
                maven.join("org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar"),
                "9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba",
            ),
            (
                maven.join("org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar"),
                "b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd",
            ),
        ];
        for (path, expected) in &libraries {
            assert!(
                path.is_file(),
                "pinned published Scala runtime exists: {}",
                path.display()
            );
            assert_eq!(
                sha256(path),
                *expected,
                "published Scala runtime hash {}",
                path.display()
            );
        }
        let revision = output(
            Command::new("git")
                .args(["-C"])
                .arg(&root)
                .args(["rev-parse", "HEAD"]),
        );
        assert_eq!(
            String::from_utf8_lossy(&revision.stdout).trim(),
            SCALA_REVISION
        );
        let scalac = root.join("dist/target/universal/stage/bin/scalac");
        let version = output(Command::new(&scalac).arg("-version"));
        let version_text = format!(
            "{}{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        assert!(
            version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
            "{version_text}"
        );
        let compiler_jar = root.join("dist/target/universal/stage/lib/with_compiler.jar");
        assert!(
            compiler_jar.is_file(),
            "staged pinned compiler jar exists: {}",
            compiler_jar.display()
        );
        let java = java_home.join("bin/java");
        let java_version = output(Command::new(&java).arg("-version"));
        let java_text = format!(
            "{}{}",
            String::from_utf8_lossy(&java_version.stdout),
            String::from_utf8_lossy(&java_version.stderr)
        );
        assert!(java_text.contains("21.0.13"), "{java_text}");
        assert_eq!(
            sha256(&java),
            "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155"
        );
        eprintln!(
            "SCALA_PROVENANCE revision={SCALA_REVISION} compiler={} compiler_sha256={} java={} java_sha256={}",
            version_text.trim(),
            sha256(&compiler_jar),
            java.display(),
            sha256(&java)
        );
        Self {
            java_home,
            scalac,
            java,
            libraries: libraries
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        }
    }

    fn scalac(&self) -> Command {
        let mut command = Command::new(&self.scalac);
        command.env("JAVA_HOME", &self.java_home);
        command.env(
            "JAVA_TOOL_OPTIONS",
            format!("-XX:-UsePerfData -Djava.io.tmpdir={}", task_dir().display()),
        );
        if let Some(path) = std::env::var_os("PATH") {
            let mut paths = vec![self.java_home.join("bin")];
            paths.extend(std::env::split_paths(&path));
            command.env("PATH", std::env::join_paths(paths).expect("PATH is valid"));
        }
        command
    }

    fn compile(&self, classpath: &str, out: &Path, source: &Path) -> Output {
        output(
            self.scalac()
                .args(["-Werror", "-release", "21", "-classpath"])
                .arg(classpath)
                .arg("-d")
                .arg(out)
                .arg(source),
        )
    }

    fn compile_result(&self, classpath: &str, out: &Path, source: &Path) -> Output {
        self.scalac()
            .args(["-Werror", "-release", "21", "-classpath"])
            .arg(classpath)
            .arg("-d")
            .arg(out)
            .arg(source)
            .output()
            .expect("run pinned scalac probe")
    }
}

fn write_batch(root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for class in batch.classes() {
        for (path, bytes) in [
            (class.classfile_path(), class.classfile_bytes()),
            (class.tasty_path(), class.tasty_bytes()),
        ] {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }
}

fn artifacts(batch: &PairedJvmTastyBatchArtifacts, root: &Path) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for class in batch.classes() {
        for path in [class.classfile_path(), class.tasty_path()] {
            let digest = sha256(&root.join(path));
            let source = if path.ends_with(".class") {
                class.classfile_bytes()
            } else {
                class.tasty_bytes()
            };
            assert_eq!(
                fs::read(root.join(path)).unwrap(),
                source,
                "original Rust artifact {path}"
            );
            eprintln!("RUST_ARTIFACT_SHA256 {path} {digest}");
            result.push((path.to_owned(), digest));
        }
        eprintln!(
            "RUST_ARTIFACT_LINK {} {:?}",
            class.classfile_path(),
            class.tasty_link_id()
        );
    }
    result
}

fn assert_unchanged(root: &Path, hashes: &[(String, String)], stage: &str) {
    for (path, hash) in hashes {
        assert_eq!(
            sha256(&root.join(path)),
            *hash,
            "Rust artifact {path} changed {stage}"
        );
    }
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("decode classfile");
    assert!(reader.is_at_end());
    class
}

fn without_method(bytes: &[u8], name: &str, descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let count = class.methods.len();
    class.methods.retain(|method| {
        pool.utf8(method.name_index).unwrap() != name
            || pool.utf8(method.descriptor_index).unwrap() != descriptor
    });
    assert_eq!(
        class.methods.len() + 1,
        count,
        "one method {name}{descriptor} exists"
    );
    class
        .encode()
        .expect("encode class with selected method masked")
}

fn wrong_method_descriptor(bytes: &[u8], name: &str, descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let replacement = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| class.constant_pool.utf8(*index).ok() == Some("()V"))
        .expect("classfile has a no-argument void descriptor");
    let pool = &class.constant_pool;
    let method = class
        .methods
        .iter_mut()
        .find(|method| {
            pool.utf8(method.name_index).ok() == Some(name)
                && pool.utf8(method.descriptor_index).ok() == Some(descriptor)
        })
        .expect("requested method exists");
    method.descriptor_index = replacement;
    class
        .encode()
        .expect("encode class with stale method descriptor")
}

fn without_tasty_marker(bytes: &[u8]) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let count = class.attributes.len();
    class.attributes.retain(|attribute| match attribute {
        Attribute::Other(raw) => pool.utf8(raw.name_index).unwrap() != "TASTY",
        _ => true,
    });
    assert_eq!(
        class.attributes.len() + 1,
        count,
        "one TASTY class attribute exists"
    );
    class.encode().expect("encode class without TASTY link")
}

fn stale_tasty_uuid(class_bytes: &[u8], tasty_bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let uuid = tasty.header().uuid;
    let mut mutated = class_bytes.to_vec();
    let offset = mutated
        .windows(uuid.len())
        .position(|window| window == uuid)
        .expect("classfile links the selected sidecar UUID");
    mutated[offset] ^= 0x80;
    mutated
}

fn create_formal_addresses(tasty_bytes: &[u8]) -> (u32, u32) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let (create, body) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()?.as_str() == "create")
                .then_some((node.offset as u32, body))
        })
        .expect("PairFactory TASTy declares create");
    assert_eq!(body.parameters.len(), 2);
    let parameters = index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter(|candidate| {
            let mut parent = Some(*candidate);
            while let Some(node) = parent {
                if node.offset as u32 == create {
                    return true;
                }
                parent = index.parent_of(node.offset as u32);
            }
            false
        })
        .map(|node| node.offset as u32)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    (parameters[0], parameters[1])
}

fn direct_termref_bytes(address: u32) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.write_nat(address);
    let mut bytes = vec![TERMREFDIRECT_TAG];
    bytes.extend(writer.into_inner());
    bytes
}

fn swap_factory_formal_targets(tasty_bytes: &[u8]) -> Vec<u8> {
    let (first, second) = create_formal_addresses(tasty_bytes);
    let first_ref = direct_termref_bytes(first);
    let second_ref = direct_termref_bytes(second);
    assert_eq!(first_ref.len(), second_ref.len());
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let asts = tasty.section(StandardSection::Asts).unwrap();
    let first_offset = asts
        .payload
        .windows(first_ref.len())
        .position(|window| window == first_ref)
        .expect("TASTy has first direct source-formal reference");
    let second_offset = asts
        .payload
        .windows(second_ref.len())
        .position(|window| window == second_ref)
        .expect("TASTy has second direct source-formal reference");
    assert_ne!(first_offset, second_offset);
    let mut swapped = tasty_bytes.to_vec();
    let first_absolute = asts.offset + first_offset + 1;
    let second_absolute = asts.offset + second_offset + 1;
    let length = first_ref.len() - 1;
    let scratch = swapped[first_absolute..first_absolute + length].to_vec();
    let second = swapped[second_absolute..second_absolute + length].to_vec();
    swapped[first_absolute..first_absolute + length].copy_from_slice(&second);
    swapped[second_absolute..second_absolute + length].copy_from_slice(&scratch);
    swapped
}

fn corrupt_factory_formal_target(tasty_bytes: &[u8]) -> Vec<u8> {
    let (first, _) = create_formal_addresses(tasty_bytes);
    let reference = direct_termref_bytes(first);
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let invalid = (0..128u32)
        .find(|address| index.get_node(*address).is_none())
        .expect("Factory TASTy has an unused one-byte AST address");
    let mut writer = Writer::default();
    writer.write_nat(invalid);
    let encoded = writer.into_inner();
    assert_eq!(encoded.len(), reference.len() - 1);
    let asts = tasty.section(StandardSection::Asts).unwrap();
    let offset = asts
        .payload
        .windows(reference.len())
        .position(|window| window == reference)
        .expect("Factory AST has the first direct formal reference");
    let mut corrupted = tasty_bytes.to_vec();
    corrupted[asts.offset + offset + 1..asts.offset + offset + reference.len()]
        .copy_from_slice(&encoded);
    corrupted
}

fn rename_factory_tasty_method(tasty_bytes: &[u8]) -> Vec<u8> {
    let original = b"create";
    let replacement = b"delete";
    assert_eq!(original.len(), replacement.len());
    let mut renamed = tasty_bytes.to_vec();
    let occurrences = renamed
        .windows(original.len())
        .filter(|window| *window == original)
        .count();
    assert_eq!(
        occurrences, 1,
        "Factory method name occurs once in its TASTy name table"
    );
    let offset = renamed
        .windows(original.len())
        .position(|window| window == original)
        .unwrap();
    renamed[offset..offset + replacement.len()].copy_from_slice(replacement);
    renamed
}

fn wrong_factory_constructor_methodref(bytes: &[u8]) -> Vec<u8> {
    let class = decode_class(bytes);
    let pair_constructor = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| match class.constant_pool.get(*index) {
            Some(ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            }) => {
                class.constant_pool.class_name(*class_index).ok() == Some("example/Pair")
                    && class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some(("<init>", "(Lexample/Counter;Lexample/Counter;)V"))
            }
            _ => false,
        })
        .expect("Factory bytecode has the expected Pair constructor Methodref");
    let wrong_constructor = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| match class.constant_pool.get(*index) {
            Some(ConstantPoolEntry::Methodref { class_index, name_and_type_index }) => {
                class.constant_pool.class_name(*class_index).ok() == Some("example/Counter")
                    && class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some(("<init>", "(I)V"))
            }
            _ => false,
        })
        .expect("Factory classfile has a Counter constructor Methodref with a wrong owner and descriptor");
    let old = pair_constructor.0.to_be_bytes();
    let new = wrong_constructor.0.to_be_bytes();
    let create = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "create")
        .expect("Factory declares create");
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("Factory create has one Code attribute");
    };
    let instruction = code
        .code
        .windows(3)
        .position(|window| window[0] == 0xb7 && window[1..] == old)
        .expect("Factory bytecode invokes the Pair constructor");
    let code_offset = code.code.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut mutated = bytes.to_vec();
    mutated[code_offset + instruction + 1..code_offset + instruction + 3].copy_from_slice(&new);
    mutated
}

fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(ch);
        }
    }
    plain
}

fn first_error(result: &Output) -> String {
    strip_ansi(&String::from_utf8_lossy(&result.stderr))
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with("Picked up JAVA_TOOL_OPTIONS:"))
        .unwrap_or("no diagnostic")
        .to_owned()
}

fn method_lookup_probes(
    root: &Path,
    scala: &Scala,
    batch: &PairedJvmTastyBatchArtifacts,
    producer: &Path,
    original_hashes: &[(String, String)],
) {
    let cases = [
        (
            "counter-get",
            "example/Counter.class",
            "get",
            "()I",
            "new Counter(1).get()",
            "value get is not a member",
        ),
        (
            "counter-set",
            "example/Counter.class",
            "set",
            "(I)V",
            "new Counter(1).set(2)",
            "value set is not a member",
        ),
        (
            "pair-left",
            "example/Pair.class",
            "leftValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).leftValue()",
            "value leftValue is not a member",
        ),
        (
            "pair-right",
            "example/Pair.class",
            "rightValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).rightValue()",
            "value rightValue is not a member",
        ),
        (
            "pair-constructor",
            "example/Pair.class",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
            "new Pair(new Counter(1), new Counter(2))",
            "constructor",
        ),
        (
            "factory-result",
            "example/PairFactory.class",
            "create",
            "(II)Lexample/Pair;",
            "val pair: Pair = new PairFactory().create(1, 2); pair.leftValue()",
            "value create is not a member",
        ),
    ];
    for (label, class_path, member, descriptor, expression, missing) in cases {
        let base = root.join(label);
        let classes = base.join("with-tasty");
        let out = base.join("with-tasty-out");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&out).unwrap();
        write_batch(&classes, batch);
        let original = fs::read(classes.join(class_path)).unwrap();
        fs::write(
            classes.join(class_path),
            without_method(&original, member, descriptor),
        )
        .unwrap();
        let source = base.join("Probe.scala");
        fs::write(
            &source,
            format!("package example\nobject Probe {{ val selected = {{ {expression} }} }}\n"),
        )
        .unwrap();
        let tasty = scala.compile_result(
            &format!("{}:{}", classes.display(), scala.libraries),
            &out,
            &source,
        );
        assert!(
            tasty.status.success(),
            "TASTy lookup for {member}{descriptor}: {}",
            String::from_utf8_lossy(&tasty.stderr)
        );
        assert_unchanged(producer, original_hashes, "during lookup probe");

        let controls = base.join("classfile-only");
        let control_out = base.join("classfile-only-out");
        fs::create_dir_all(&controls).unwrap();
        fs::create_dir_all(&control_out).unwrap();
        write_batch(&controls, batch);
        fs::write(
            controls.join(class_path),
            without_tasty_marker(&without_method(
                &fs::read(controls.join(class_path)).unwrap(),
                member,
                descriptor,
            )),
        )
        .unwrap();
        fs::remove_file(controls.join(class_path.replace(".class", ".tasty"))).unwrap();
        let control = scala.compile_result(
            &format!("{}:{}", controls.display(), scala.libraries),
            &control_out,
            &source,
        );
        assert!(
            !control.status.success(),
            "classfile-only control resolved {member}{descriptor}"
        );
        let diagnostic = strip_ansi(&String::from_utf8_lossy(&control.stderr));
        assert!(
            diagnostic.contains(missing),
            "wrong classfile-only failure for {member}{descriptor}: {diagnostic}"
        );
        eprintln!(
            "TASTY_LOOKUP_ACCEPTED {member}{descriptor}; CLASSFILE_ONLY_FIRST_REJECTION {}",
            first_error(&control)
        );
    }
}

fn inspect_original_tasty(
    scala: &Scala,
    producer: &Path,
    root: &Path,
    hashes: &[(String, String)],
) {
    let reconstructed = root.join("scala-from-tasty-inspection-only");
    fs::create_dir_all(&reconstructed).unwrap();
    let sidecars = ["Counter", "Pair", "PairFactory"]
        .map(|name| producer.join(format!("example/{name}.tasty")));
    let mut factory_dump = String::new();
    for (name, sidecar) in ["Counter", "Pair", "PairFactory"]
        .into_iter()
        .zip(&sidecars)
    {
        let decompiled = scala
            .scalac()
            .arg("-decompile")
            .arg(sidecar)
            .output()
            .expect("run pinned Scala decompiler");
        assert!(
            decompiled.status.success(),
            "Scala -decompile rejected Rust {name}.tasty: {}",
            String::from_utf8_lossy(&decompiled.stderr)
        );
        let decompiled = strip_ansi(&String::from_utf8_lossy(&decompiled.stdout));
        match name {
            "Counter" => {
                for member in ["def get", "def set"] {
                    assert!(
                        decompiled.contains(member),
                        "Counter decompile omitted {member}: {decompiled}"
                    );
                }
            }
            "Pair" => {
                for member in [
                    "class Pair(first: example.Counter, second: example.Counter)",
                    "private[this] val left: example.Counter = Pair.this.first",
                    "private[this] val right: example.Counter = Pair.this.second",
                    "def leftValue",
                    "def rightValue",
                ] {
                    assert!(
                        decompiled.contains(member),
                        "Pair decompile did not preserve the ordered reference constructor and private immutable aliases ({member}): {decompiled}"
                    );
                }
            }
            "PairFactory" => {
                assert!(
                    decompiled
                        .contains("def create(first: scala.Int, second: scala.Int): example.Pair"),
                    "{decompiled}"
                );
                let body = decompiled
                    .rsplit_once(" = ")
                    .expect("Factory decompile has a method body")
                    .1;
                let first = body
                    .find("new example.Counter(first)")
                    .expect("first nested Counter use points to first formal");
                let second = body
                    .find("new example.Counter(second)")
                    .expect("second nested Counter use points to second formal");
                assert!(
                    first < second,
                    "nested arguments remain source ordered: {decompiled}"
                );
                assert!(
                    body.contains(
                        "new example.Pair(new example.Counter(first), new example.Counter(second))"
                    ),
                    "{decompiled}"
                );
                factory_dump = body.to_owned();
            }
            _ => unreachable!(),
        }
        let printed = scala
            .scalac()
            .arg("-print-tasty")
            .arg(sidecar)
            .output()
            .expect("run pinned Scala -print-tasty");
        assert!(
            printed.status.success(),
            "Scala -print-tasty rejected Rust {name}.tasty: {}",
            String::from_utf8_lossy(&printed.stderr)
        );
        let dump = strip_ansi(&String::from_utf8_lossy(&printed.stdout));
        assert!(
            dump.contains("version: 28.9.0"),
            "wrong TASTy version for {name}: {dump}"
        );
        eprintln!(
            "SCALA_PRINT_TASTY {name} bytes={} sha256={}",
            dump.len(),
            sha256(sidecar)
        );
    }
    assert!(factory_dump.contains("first") && factory_dump.contains("second"));
    let from_tasty = scala
        .scalac()
        .args(["-release", "21", "-from-tasty", "-classpath"])
        .arg(format!("{}:{}", producer.display(), scala.libraries))
        .arg("-d")
        .arg(&reconstructed)
        .args(&sidecars)
        .output()
        .expect("run pinned Scala -from-tasty on original Rust sidecars");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected Rust sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    for name in ["Counter", "Pair", "PairFactory"] {
        assert!(
            reconstructed
                .join(format!("example/{name}.class"))
                .is_file(),
            "Scala -from-tasty reconstructed {name} in the inspection-only directory"
        );
    }
    assert_unchanged(
        producer,
        hashes,
        "after decompile, print-tasty and from-tasty",
    );
    eprintln!(
        "SCALA_FROM_TASTY_OUTPUT {} (inspection-only; excluded from runtime classpath)",
        reconstructed.display()
    );
}

fn negative_reader_probes(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
    producer: &Path,
    original_hashes: &[(String, String)],
) {
    for class_name in ["Counter", "Pair", "PairFactory"] {
        let class_path = format!("example/{class_name}.class");
        let tasty_path = format!("example/{class_name}.tasty");
        for corruption in ["missing", "corrupt", "misplaced", "mismatched-uuid"] {
            let label = format!("{class_name}-{corruption}");
            let base = root.join(&label);
            let classes = base.join("classes");
            let out = base.join("out");
            fs::create_dir_all(&classes).unwrap();
            fs::create_dir_all(&out).unwrap();
            write_batch(&classes, batch);
            let tasty = classes.join(&tasty_path);
            match corruption {
                "missing" => fs::remove_file(&tasty).unwrap(),
                "corrupt" => {
                    let mut bytes = fs::read(&tasty).unwrap();
                    bytes.truncate(bytes.len().saturating_sub(3));
                    fs::write(&tasty, bytes).unwrap();
                }
                "misplaced" => {
                    fs::rename(&tasty, classes.join(format!("{tasty_path}.moved"))).unwrap()
                }
                "mismatched-uuid" => {
                    let class_file = classes.join(&class_path);
                    let class_bytes = fs::read(&class_file).unwrap();
                    fs::write(
                        &class_file,
                        stale_tasty_uuid(&class_bytes, &fs::read(&tasty).unwrap()),
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let client = base.join("Consumer.scala");
            fs::write(&client, CLIENT).unwrap();
            let compilation = scala.compile_result(
                &format!("{}:{}", classes.display(), scala.libraries),
                &out,
                &client,
            );
            assert!(!compilation.status.success(), "Scala accepted {label}");
            let diagnostic = first_error(&compilation);
            let stderr = strip_ansi(&String::from_utf8_lossy(&compilation.stderr));
            let expected_first_failure = match corruption {
                "missing" | "misplaced" => {
                    format!(
                        "Loading Scala 3 binary from {}. It should have been loaded from `.tasty` file. This `.tasty` file is missing.",
                        classes.join(&class_path).display()
                    )
                }
                "corrupt" => {
                    format!("error while loading {class_name},")
                }
                "mismatched-uuid" => format!(
                    "{} is out of sync with its TASTy file. Loaded TASTy file. Try cleaning the project to fix this issue",
                    classes.join(&class_path).display()
                ),
                _ => unreachable!(),
            };
            assert!(
                diagnostic.contains(&expected_first_failure),
                "wrong first rejecting stage for {label}; expected {expected_first_failure:?}, got {diagnostic:?}\nfull stderr:\n{stderr}"
            );
            if corruption == "corrupt" {
                assert!(
                    stderr.contains(&format!("{class_name}.tasty file"))
                        && stderr.contains(
                            "is broken, reading aborted with class java.lang.ArrayIndexOutOfBoundsException"
                        ),
                    "corrupt {class_name} sidecar did not fail in Scala's TASTy reader:\n{stderr}"
                );
            }
            if corruption == "mismatched-uuid" {
                assert!(
                    stderr.contains("No warnings can be incurred under -Werror"),
                    "UUID warning was not promoted to the expected compile failure for {label}:\n{stderr}"
                );
            }
            eprintln!("SCALA_NEGATIVE_FIRST_FAILURE {label}: {diagnostic}");
        }
    }

    for (label, body, expected) in [
        (
            "wrong-pair-constructor-arity",
            "val pair = new Pair(new Counter(1))",
            "[E171] Type Error",
        ),
        (
            "wrong-second-counter-argument-type",
            "val pair = new Pair(new Counter(1), new Wrong())",
            "[E007] Type Mismatch Error",
        ),
    ] {
        let base = root.join(label);
        let classes = base.join("classes");
        let out = base.join("out");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&out).unwrap();
        write_batch(&classes, batch);
        let source = base.join("Probe.scala");
        fs::write(
            &source,
            format!("package example\nclass Wrong\nobject Probe {{ {body} }}\n"),
        )
        .unwrap();
        let compilation = scala.compile_result(
            &format!("{}:{}", classes.display(), scala.libraries),
            &out,
            &source,
        );
        assert!(!compilation.status.success(), "Scala accepted {label}");
        let diagnostic = first_error(&compilation);
        assert!(
            diagnostic.contains(expected),
            "unexpected first typechecking error for {label}: {diagnostic}"
        );
        eprintln!("SCALA_NEGATIVE_FIRST_FAILURE {label}: {diagnostic}");
    }

    let unknown_root = root.join("altered-method-name");
    let unknown_classes = unknown_root.join("classes");
    let unknown_out = unknown_root.join("out");
    fs::create_dir_all(&unknown_classes).unwrap();
    fs::create_dir_all(&unknown_out).unwrap();
    write_batch(&unknown_classes, batch);
    let unknown_source = unknown_root.join("Probe.scala");
    fs::write(
        &unknown_source,
        "package example\nobject Probe { val selected = new PairFactory().missing(1, 2) }\n",
    )
    .unwrap();
    let unknown = scala.compile_result(
        &format!("{}:{}", unknown_classes.display(), scala.libraries),
        &unknown_out,
        &unknown_source,
    );
    assert!(
        !unknown.status.success(),
        "Scala accepted an absent source method name"
    );
    assert!(first_error(&unknown).contains("[E008] Not Found Error"));
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE altered-method-name: {}",
        first_error(&unknown)
    );

    let renamed_member_root = root.join("renamed-tasty-method");
    let renamed_member_classes = renamed_member_root.join("classes");
    let renamed_member_out = renamed_member_root.join("out");
    fs::create_dir_all(&renamed_member_classes).unwrap();
    fs::create_dir_all(&renamed_member_out).unwrap();
    write_batch(&renamed_member_classes, batch);
    let factory = batch
        .classes()
        .iter()
        .find(|class| class.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    fs::write(
        renamed_member_classes.join(factory.tasty_path()),
        rename_factory_tasty_method(factory.tasty_bytes()),
    )
    .unwrap();
    let renamed_member_client = renamed_member_root.join("Consumer.scala");
    fs::write(&renamed_member_client, CLIENT).unwrap();
    let renamed_member = scala.compile_result(
        &format!("{}:{}", renamed_member_classes.display(), scala.libraries),
        &renamed_member_out,
        &renamed_member_client,
    );
    assert!(
        !renamed_member.status.success(),
        "Scala resolved create after the TASTy member was renamed"
    );
    assert!(first_error(&renamed_member).contains("[E008] Not Found Error"));
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE TASTy create renamed to delete: {}",
        first_error(&renamed_member)
    );

    let descriptor_root = root.join("wrong-pair-constructor-descriptor");
    let descriptor_classes = descriptor_root.join("classes");
    let descriptor_consumer = descriptor_root.join("consumer");
    fs::create_dir_all(&descriptor_classes).unwrap();
    fs::create_dir_all(&descriptor_consumer).unwrap();
    write_batch(&descriptor_classes, batch);
    let pair = batch
        .classes()
        .iter()
        .find(|class| class.classfile_path() == "example/Pair.class")
        .unwrap();
    fs::write(
        descriptor_classes.join("example/Pair.class"),
        wrong_method_descriptor(
            pair.classfile_bytes(),
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
        ),
    )
    .unwrap();
    let descriptor_client = descriptor_root.join("Consumer.scala");
    fs::write(&descriptor_client, CLIENT).unwrap();
    let descriptor_compile = scala.compile_result(
        &format!("{}:{}", descriptor_classes.display(), scala.libraries),
        &descriptor_consumer,
        &descriptor_client,
    );
    assert!(
        descriptor_compile.status.success(),
        "Pair constructor source signature still resolves from its TASTy sidecar: {}",
        String::from_utf8_lossy(&descriptor_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        descriptor_consumer.display(),
        descriptor_classes.display(),
        scala.libraries
    );
    let descriptor_runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.RustArtifactConsumer")
        .output()
        .expect("run stale Pair constructor descriptor control");
    assert!(!descriptor_runtime.status.success());
    let descriptor_error = format!(
        "{}{}",
        String::from_utf8_lossy(&descriptor_runtime.stdout),
        String::from_utf8_lossy(&descriptor_runtime.stderr)
    );
    assert!(
        descriptor_error.contains("VerifyError") || descriptor_error.contains("LinkageError"),
        "stale Pair constructor descriptor fails at JVM verification or linkage: {descriptor_error}"
    );
    eprintln!(
        "JVM_WRONG_PAIR_CONSTRUCTOR_DESCRIPTOR_FIRST_FAILURE {}",
        first_error(&descriptor_runtime)
    );

    let base = root.join("missing-counter-classfile");
    let classes = base.join("classes");
    let consumer = base.join("consumer");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&consumer).unwrap();
    write_batch(&classes, batch);
    fs::remove_file(classes.join("example/Counter.class")).unwrap();
    let client = base.join("Consumer.scala");
    fs::write(&client, CLIENT).unwrap();
    let compilation = scala.compile_result(
        &format!("{}:{}", classes.display(), scala.libraries),
        &consumer,
        &client,
    );
    assert!(
        compilation.status.success(),
        "genuine Counter TASTy still supplies Scala source lookup when the classfile is absent: {}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        consumer.display(),
        classes.display(),
        scala.libraries
    );
    let runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.RustArtifactConsumer")
        .output()
        .expect("run against missing-classfile control");
    assert!(
        !runtime.status.success(),
        "runtime unexpectedly found missing original Counter classfile"
    );
    let runtime_diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&runtime.stdout),
        String::from_utf8_lossy(&runtime.stderr)
    );
    assert!(
        runtime_diagnostic.contains("NoClassDefFoundError")
            || runtime_diagnostic.contains("ClassNotFoundException"),
        "wrong missing classfile failure: {runtime_diagnostic}"
    );
    eprintln!(
        "SCALA_LOOKUP_ACCEPTS_MISSING_CLASSFILE; JVM_RUNTIME_FIRST_FAILURE {}",
        first_error(&runtime)
    );

    let factory = batch
        .classes()
        .iter()
        .find(|class| class.classfile_path() == "example/PairFactory.class")
        .unwrap();
    let swapped_root = root.join("swapped-wire-formals");
    let swapped_classes = swapped_root.join("classes");
    let swapped_out = swapped_root.join("consumer");
    fs::create_dir_all(&swapped_classes).unwrap();
    fs::create_dir_all(&swapped_out).unwrap();
    write_batch(&swapped_classes, batch);
    fs::write(
        swapped_classes.join("example/PairFactory.tasty"),
        swap_factory_formal_targets(factory.tasty_bytes()),
    )
    .unwrap();
    let swapped_client = swapped_root.join("Consumer.scala");
    fs::write(&swapped_client, CLIENT).unwrap();
    let swapped_compile = scala.compile_result(
        &format!("{}:{}", swapped_classes.display(), scala.libraries),
        &swapped_out,
        &swapped_client,
    );
    assert!(
        swapped_compile.status.success(),
        "Scala accepts a type-correct wire mutation without validating its argument identity: {}",
        String::from_utf8_lossy(&swapped_compile.stderr)
    );
    let swapped_decompile = scala
        .scalac()
        .arg("-decompile")
        .arg(swapped_classes.join("example/PairFactory.tasty"))
        .output()
        .expect("decompile swapped source-formal wire copy");
    assert!(swapped_decompile.status.success());
    let swapped_text = strip_ansi(&String::from_utf8_lossy(&swapped_decompile.stdout));
    assert!(
        swapped_text
            .contains("new example.Pair(new example.Counter(second), new example.Counter(first))"),
        "Scala exposes the swapped original-formal targets: {swapped_text}"
    );
    eprintln!("SCALA_ACCEPTED_TYPE_CORRECT_SWAPPED_FORMAL_WIRE; decompile shows second/first");

    let corrupt_root = root.join("corrupt-nested-formal-address");
    let corrupt_classes = corrupt_root.join("classes");
    let corrupt_out = corrupt_root.join("out");
    fs::create_dir_all(&corrupt_classes).unwrap();
    fs::create_dir_all(&corrupt_out).unwrap();
    write_batch(&corrupt_classes, batch);
    fs::write(
        corrupt_classes.join("example/PairFactory.tasty"),
        corrupt_factory_formal_target(factory.tasty_bytes()),
    )
    .unwrap();
    let corrupt_client = corrupt_root.join("Consumer.scala");
    fs::write(&corrupt_client, CLIENT).unwrap();
    let corrupt_compile = scala.compile_result(
        &format!("{}:{}", corrupt_classes.display(), scala.libraries),
        &corrupt_out,
        &corrupt_client,
    );
    assert!(
        corrupt_compile.status.success(),
        "consumer signature lookup is lazy and does not force the malformed Factory body: {}",
        String::from_utf8_lossy(&corrupt_compile.stderr)
    );
    let corrupt_read = scala
        .scalac()
        .arg("-decompile")
        .arg(corrupt_classes.join("example/PairFactory.tasty"))
        .output()
        .expect("force official Scala readTasty of corrupted nested reference");
    assert!(
        !corrupt_read.status.success(),
        "Scala -decompile accepted an absent nested AST reference"
    );
    let corrupt_diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&corrupt_read.stdout),
        String::from_utf8_lossy(&corrupt_read.stderr)
    );
    assert!(
        corrupt_diagnostic.contains("TreeWithoutOwner"),
        "unexpected corrupted TASTy first forced error: {corrupt_diagnostic}"
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE corrupt-nested-tasty-address: official readTasty -decompile: TreeWithoutOwner"
    );

    let methodref_root = root.join("wrong-factory-pair-methodref");
    let methodref_classes = methodref_root.join("classes");
    let methodref_consumer = methodref_root.join("consumer");
    fs::create_dir_all(&methodref_classes).unwrap();
    fs::create_dir_all(&methodref_consumer).unwrap();
    write_batch(&methodref_classes, batch);
    fs::write(
        methodref_classes.join("example/PairFactory.class"),
        wrong_factory_constructor_methodref(factory.classfile_bytes()),
    )
    .unwrap();
    let methodref_client = methodref_root.join("Consumer.scala");
    fs::write(&methodref_client, CLIENT).unwrap();
    let methodref_compile = scala.compile_result(
        &format!("{}:{}", methodref_classes.display(), scala.libraries),
        &methodref_consumer,
        &methodref_client,
    );
    assert!(
        methodref_compile.status.success(),
        "TASTy source lookup accepts the separate classfile Methodref mutation: {}",
        String::from_utf8_lossy(&methodref_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        methodref_consumer.display(),
        methodref_classes.display(),
        scala.libraries
    );
    let methodref_runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("example.RustArtifactConsumer")
        .output()
        .expect("run Factory with a stale Pair constructor Methodref");
    assert!(
        !methodref_runtime.status.success(),
        "wrong Factory Pair constructor Methodref unexpectedly ran"
    );
    let methodref_error = format!(
        "{}{}",
        String::from_utf8_lossy(&methodref_runtime.stdout),
        String::from_utf8_lossy(&methodref_runtime.stderr)
    );
    assert!(
        methodref_error.contains("VerifyError") || methodref_error.contains("LinkageError"),
        "wrong Factory Methodref fails at the JVM verifier/linker: {methodref_error}"
    );
    eprintln!(
        "JVM_WRONG_FACTORY_PAIR_METHODREF_FIRST_FAILURE {}",
        first_error(&methodref_runtime)
    );
    assert_unchanged(
        producer,
        original_hashes,
        "after all Scala/JVM negative controls",
    );
}

fn renamed_variant(source: &str) -> String {
    source
        .replace("package example", "package renamed.example")
        .replace("PairFactory", "PairMaker")
        .replace("Counter", "Gauge")
        .replace("Pair", "Duo")
        .replace("create", "build")
}

fn reordered_variant(source: &str) -> String {
    let counter = source.find("class Counter").unwrap();
    let pair = source.find("class Pair(").unwrap();
    let factory = source.find("class PairFactory").unwrap();
    format!(
        "{}{}\n{}\n{}",
        &source[..counter],
        &source[factory..],
        &source[pair..factory],
        &source[counter..pair]
    )
}

fn compile_and_run_variant(
    scala: &Scala,
    root: &Path,
    label: &str,
    source: &str,
    client: &str,
    main_class: &str,
    source_id: u32,
) {
    let batch = emit_paired_jvm_tasty_batch(
        source,
        "PairFactoryVariant.scala",
        SourceId::from_index(source_id),
    )
    .expect("each source variant is emitted in one authentic Rust session");
    let repeated = emit_paired_jvm_tasty_batch(
        source,
        "PairFactoryVariant.scala",
        SourceId::from_index(source_id + 1),
    )
    .expect("a second session emits the same source variant");
    assert_eq!(batch.classes().len(), repeated.classes().len());
    for (first, second) in batch.classes().iter().zip(repeated.classes()) {
        assert_eq!(first.classfile_path(), second.classfile_path());
        assert_eq!(first.classfile_bytes(), second.classfile_bytes());
        assert_eq!(first.tasty_path(), second.tasty_path());
        assert_eq!(first.tasty_bytes(), second.tasty_bytes());
    }
    let base = root.join(label);
    let producer = base.join("rust-original");
    let consumer = base.join("scala-consumer");
    fs::create_dir_all(&producer).unwrap();
    fs::create_dir_all(&consumer).unwrap();
    fs::write(base.join("Producer.scala"), source).unwrap();
    write_batch(&producer, &batch);
    let hashes = artifacts(&batch, &producer);
    let source_path = base.join("Consumer.scala");
    fs::write(&source_path, client).unwrap();
    scala.compile(
        &format!("{}:{}", producer.display(), scala.libraries),
        &consumer,
        &source_path,
    );
    let classpath = format!(
        "{}:{}:{}",
        consumer.display(),
        producer.display(),
        scala.libraries
    );
    let runtime = output(
        Command::new(&scala.java)
            .env(
                "JAVA_TOOL_OPTIONS",
                format!("-XX:-UsePerfData -Djava.io.tmpdir={}", task_dir().display()),
            )
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg(main_class),
    );
    assert!(
        String::from_utf8_lossy(&runtime.stdout).contains("independent=true"),
        "{label}: {}",
        String::from_utf8_lossy(&runtime.stdout)
    );
    assert_unchanged(
        &producer,
        &hashes,
        "after source-variant Scala compile/runtime",
    );
    eprintln!(
        "SCALA_VARIANT_ACCEPTED {label} producer_source_sha256={} consumer_source_sha256={}",
        sha256(&base.join("Producer.scala")),
        sha256(&source_path),
    );
}

#[test]
fn official_scala_compiles_and_runs_against_original_rust_multi_argument_batch() {
    if !required() {
        return;
    }
    let scala = Scala::from_environment();
    let producer_source_hash = sha256(Path::new(
        "tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala",
    ));
    let consumer_source_hash = sha256(Path::new(
        "tests/fixtures/jvm_tasty_multi_argument_constructor_calls/RustArtifactConsumer.scala",
    ));
    eprintln!(
        "SOURCE_SHA256 producer={producer_source_hash} consumer={consumer_source_hash} source_revision={SCALA_REVISION}"
    );
    assert_eq!(
        fs::read_to_string(Path::new(
            "tests/fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala"
        ))
        .unwrap(),
        SOURCE
    );
    assert_eq!(
        fs::read_to_string(Path::new(
            "tests/fixtures/jvm_tasty_multi_argument_constructor_calls/RustArtifactConsumer.scala"
        ))
        .unwrap(),
        CLIENT
    );

    let batch =
        emit_paired_jvm_tasty_batch(SOURCE, "PairFactory.scala", SourceId::from_index(19190))
            .expect("one authentic Rust session emits the original linked batch");
    assert_eq!(batch.classes().len(), 3);
    let repeated =
        emit_paired_jvm_tasty_batch(SOURCE, "PairFactory.scala", SourceId::from_index(19191))
            .expect("a second fresh session emits the same original source");
    assert_eq!(batch.classes().len(), repeated.classes().len());
    for (first, second) in batch.classes().iter().zip(repeated.classes()) {
        assert_eq!(
            first.classfile_bytes(),
            second.classfile_bytes(),
            "fresh classfile {} is deterministic",
            first.classfile_path()
        );
        assert_eq!(
            first.tasty_bytes(),
            second.tasty_bytes(),
            "fresh TASTy {} is deterministic",
            first.tasty_path()
        );
    }
    let root = task_dir();
    let producer = root.join("original-rust");
    let consumer = root.join("scala-consumer");
    fs::create_dir_all(&producer).unwrap();
    fs::create_dir_all(&consumer).unwrap();
    write_batch(&producer, &batch);
    let hashes = artifacts(&batch, &producer);
    let source = Path::new(
        "tests/fixtures/jvm_tasty_multi_argument_constructor_calls/RustArtifactConsumer.scala",
    );
    let _ = scala.compile(
        &format!("{}:{}", producer.display(), scala.libraries),
        &consumer,
        source,
    );
    assert_unchanged(&producer, &hashes, "after Scala consumer compilation");
    let classpath = format!(
        "{}:{}:{}",
        consumer.display(),
        producer.display(),
        scala.libraries
    );
    eprintln!("RUNTIME_CLASSPATH {classpath}");
    let java = output(
        Command::new(&scala.java)
            .env(
                "JAVA_TOOL_OPTIONS",
                format!("-XX:-UsePerfData -Djava.io.tmpdir={}", task_dir().display()),
            )
            .args(["-Xverify:all", "-ea", "-classpath"])
            .arg(classpath)
            .arg("example.RustArtifactConsumer"),
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        "rust-artifact-scala-consumer=7/3,3/7,-9/42; independent=true"
    );
    assert_unchanged(&producer, &hashes, "after Java runtime");

    inspect_original_tasty(&scala, &producer, &root, &hashes);

    for class in batch.classes() {
        assert_eq!(
            fs::read(producer.join(class.classfile_path())).unwrap(),
            class.classfile_bytes()
        );
        assert_eq!(
            fs::read(producer.join(class.tasty_path())).unwrap(),
            class.tasty_bytes()
        );
        let decoded_class = decode_class(class.classfile_bytes());
        assert_eq!(
            decoded_class.version.major, 65,
            "Rust classfile major version"
        );
        let tasty = TastyFile::parse_and_validate_scala_3_9(class.tasty_bytes())
            .expect("genuine Rust sidecar is TASTy 28.9");
        assert_eq!(
            decoded_class.tasty_link_id().unwrap().unwrap().as_bytes(),
            &tasty.header().uuid,
            "each original Rust classfile links its own TASTy UUID"
        );
    }
    method_lookup_probes(&root, &scala, &batch, &producer, &hashes);
    negative_reader_probes(&scala, &root, &batch, &producer, &hashes);

    compile_and_run_variant(
        &scala,
        &root,
        "renamed-source",
        &renamed_variant(SOURCE),
        &renamed_variant(CLIENT),
        "renamed.example.RustArtifactConsumer",
        19192,
    );
    compile_and_run_variant(
        &scala,
        &root,
        "reordered-declarations",
        &reordered_variant(SOURCE),
        CLIENT,
        "example.RustArtifactConsumer",
        19193,
    );
}
