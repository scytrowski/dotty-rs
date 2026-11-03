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

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_MIXED_ORIGIN_CONSTRUCTOR_CONSUMER";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const FORWARD: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
);
const REVERSE: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala"
);
const CLIENT: &str =
    include_str!("fixtures/jvm_tasty_mixed_origin_constructor_arguments/Consumer.scala");
const REVERSED_CLIENT: &str =
    include_str!("fixtures/jvm_tasty_mixed_origin_constructor_arguments/ConsumerReversed.scala");

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

fn sha256(path: &Path) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum emits a hash")
        .to_owned()
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
    let path = root.join(format!("dotty-issue-2017-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create task-local root");
    path
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
                "published runtime hash {}",
                path.display()
            );
        }
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

    fn command(&self) -> Command {
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

    fn compile(&self, classpath: &str, out: &Path, source: &Path) {
        output(
            self.command()
                .args(["-Werror", "-release", "21", "-classpath"])
                .arg(classpath)
                .arg("-d")
                .arg(out)
                .arg(source),
        );
    }

    fn compile_result(&self, classpath: &str, out: &Path, source: &Path) -> Output {
        self.command()
            .args(["-Werror", "-release", "21", "-classpath"])
            .arg(classpath)
            .arg("-d")
            .arg(out)
            .arg(source)
            .output()
            .expect("run pinned Scala compile probe")
    }
}

fn write_batch(root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for artifact in batch.classes() {
        for (path, bytes) in [
            (artifact.classfile_path(), artifact.classfile_bytes()),
            (artifact.tasty_path(), artifact.tasty_bytes()),
        ] {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }
}

fn hashes(batch: &PairedJvmTastyBatchArtifacts, root: &Path) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for artifact in batch.classes() {
        for path in [artifact.classfile_path(), artifact.tasty_path()] {
            let digest = sha256(&root.join(path));
            let original = if path.ends_with(".class") {
                artifact.classfile_bytes()
            } else {
                artifact.tasty_bytes()
            };
            assert_eq!(
                fs::read(root.join(path)).unwrap(),
                original,
                "original Rust artifact {path}"
            );
            eprintln!("RUST_ARTIFACT_SHA256 {path} {digest}");
            result.push((path.to_owned(), digest));
        }
        eprintln!(
            "RUST_ARTIFACT_LINK {} {:?}",
            artifact.classfile_path(),
            artifact.tasty_link_id()
        );
    }
    result
}

fn assert_unchanged(root: &Path, before: &[(String, String)], stage: &str) {
    for (path, expected) in before {
        assert_eq!(
            sha256(&root.join(path)),
            *expected,
            "original {path} changed {stage}"
        );
    }
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("decode original Rust classfile");
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
        "one {name}{descriptor} method exists; methods={:?}",
        class
            .methods
            .iter()
            .map(|method| (
                pool.utf8(method.name_index).unwrap(),
                pool.utf8(method.descriptor_index).unwrap()
            ))
            .collect::<Vec<_>>()
    );
    class
        .encode()
        .expect("encode classfile with one method masked")
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
    class
        .encode()
        .expect("encode classfile without TASTY attribute")
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
    class.encode().expect("encode stale method descriptor")
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
            Some(ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            }) => {
                class.constant_pool.class_name(*class_index).ok() == Some("example/Counter")
                    && class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some(("<init>", "(I)V"))
            }
            _ => false,
        })
        .expect("Factory classfile has a Counter constructor Methodref as a wrong target");
    let old = pair_constructor.0.to_be_bytes();
    let new = wrong_constructor.0.to_be_bytes();
    let create = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "wrapAndCreate")
        .expect("Factory declares wrapAndCreate");
    let [Attribute::Code(code)] = create.attributes.as_slice() else {
        panic!("Factory method has one Code attribute");
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

fn wrong_pair_getter_fieldref(bytes: &[u8]) -> Vec<u8> {
    let class = decode_class(bytes);
    let left_ref = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| match class.constant_pool.get(*index) {
            Some(ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            }) => {
                class.constant_pool.class_name(*class_index).ok() == Some("example/Pair")
                    && class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some(("left", "Lexample/Counter;"))
            }
            _ => false,
        })
        .expect("Pair bytecode has a left Fieldref");
    let right_ref = (1..=u16::MAX)
        .map(ConstantPoolIndex)
        .find(|index| match class.constant_pool.get(*index) {
            Some(ConstantPoolEntry::Fieldref {
                class_index,
                name_and_type_index,
            }) => {
                class.constant_pool.class_name(*class_index).ok() == Some("example/Pair")
                    && class.constant_pool.name_and_type(*name_and_type_index).ok()
                        == Some(("right", "Lexample/Counter;"))
            }
            _ => false,
        })
        .expect("Pair bytecode has a right Fieldref");
    let left = left_ref.0.to_be_bytes();
    let right = right_ref.0.to_be_bytes();
    let getter = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index).unwrap() == "leftValue")
        .expect("Pair declares leftValue");
    let [Attribute::Code(code)] = getter.attributes.as_slice() else {
        panic!("Pair leftValue has one Code attribute");
    };
    let instruction = code
        .code
        .windows(3)
        .position(|window| window[0] == 0xb4 && window[1..] == left)
        .expect("leftValue reads the left field");
    let code_offset = code.code.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut mutated = bytes.to_vec();
    mutated[code_offset + instruction + 1..code_offset + instruction + 3].copy_from_slice(&right);
    mutated
}

fn stale_tasty_uuid(class_bytes: &[u8], tasty_bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let uuid = tasty.header().uuid;
    let mut mutated = class_bytes.to_vec();
    let offset = mutated
        .windows(uuid.len())
        .position(|window| window == uuid)
        .expect("classfile links to original sidecar UUID");
    mutated[offset] ^= 0x80;
    mutated
}

fn formal_addresses(tasty_bytes: &[u8], method_name: &str) -> (u32, u32) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let (method, body) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()?.as_str() == method_name)
                .then_some((node.offset as u32, body))
        })
        .expect("TASTy contains mixed factory method");
    assert_eq!(body.parameters.len(), 2);
    let addresses = index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter(|candidate| {
            std::iter::successors(Some(*candidate), |node| index.parent_of(node.offset as u32))
                .any(|parent| parent.offset as u32 == method)
        })
        .map(|node| node.offset as u32)
        .collect::<Vec<_>>();
    assert_eq!(addresses.len(), 2);
    (addresses[0], addresses[1])
}

fn direct_termref_bytes(address: u32) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.write_nat(address);
    let mut bytes = vec![TERMREFDIRECT_TAG];
    bytes.extend(writer.into_inner());
    bytes
}

fn change_formal_target(tasty_bytes: &[u8], target: u32, replacement: u32) -> Vec<u8> {
    let reference = direct_termref_bytes(target);
    let mut writer = Writer::default();
    writer.write_nat(replacement);
    let replacement = writer.into_inner();
    assert_eq!(
        replacement.len(),
        reference.len() - 1,
        "replacement AST address uses the original encoded width"
    );
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
    let asts = tasty.section(StandardSection::Asts).unwrap();
    let offset = asts
        .payload
        .windows(reference.len())
        .position(|window| window == reference)
        .expect("TASTy contains selected direct formal reference");
    let mut mutated = tasty_bytes.to_vec();
    mutated[asts.offset + offset + 1..asts.offset + offset + reference.len()]
        .copy_from_slice(&replacement);
    mutated
}

fn rename_tasty_method(tasty_bytes: &[u8], original: &[u8], replacement: &[u8]) -> Vec<u8> {
    assert_eq!(original.len(), replacement.len());
    let count = tasty_bytes
        .windows(original.len())
        .filter(|window| *window == original)
        .count();
    assert_eq!(count, 1, "TASTy method spelling occurs once");
    let offset = tasty_bytes
        .windows(original.len())
        .position(|window| window == original)
        .unwrap();
    let mut bytes = tasty_bytes.to_vec();
    bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
    bytes
}

fn first_error(result: &Output) -> String {
    strip_ansi(&String::from_utf8_lossy(&result.stderr))
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with("Picked up JAVA_TOOL_OPTIONS:"))
        .unwrap_or("no diagnostic")
        .to_owned()
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

fn lookup_probes(
    scala: &Scala,
    root: &Path,
    batch: &PairedJvmTastyBatchArtifacts,
    producer: &Path,
    before: &[(String, String)],
    factory_method: &str,
) {
    let cases = [
        (
            "counter-get",
            "example/Counter.class",
            "get",
            "()I",
            "new Counter(1).get()",
            "[E008] Not Found Error",
        ),
        (
            "counter-set",
            "example/Counter.class",
            "set",
            "(I)V",
            "new Counter(1).set(2)",
            "[E008] Not Found Error",
        ),
        (
            "pair-left",
            "example/Pair.class",
            "leftValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).leftValue()",
            "[E008] Not Found Error",
        ),
        (
            "pair-right",
            "example/Pair.class",
            "rightValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).rightValue()",
            "[E008] Not Found Error",
        ),
        (
            "pair-constructor",
            "example/Pair.class",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
            "new Pair(new Counter(1), new Counter(2))",
            "[E173] Reference Error",
        ),
        (
            "factory-wrap-and-create",
            "example/PairFactory.class",
            "wrapAndCreate",
            "(Lexample/Counter;I)Lexample/Pair;",
            "new PairFactory().wrapAndCreate(new Counter(1), 2)",
            "[E008] Not Found Error",
        ),
    ];
    for (label, class_path, original_member, descriptor, original_expression, missing) in cases {
        let member = if label == "factory-wrap-and-create" {
            factory_method
        } else {
            original_member
        };
        let expression = original_expression.replace("wrapAndCreate", factory_method);
        let case = root.join(format!("lookup-{label}"));
        let sidecar_dir = case.join("original-sidecar");
        let sidecar_out = case.join("original-sidecar-out");
        let classfile_dir = case.join("classfile-only");
        let classfile_out = case.join("classfile-only-out");
        for dir in [&sidecar_dir, &sidecar_out, &classfile_dir, &classfile_out] {
            fs::create_dir_all(dir).unwrap();
        }
        write_batch(&sidecar_dir, batch);
        write_batch(&classfile_dir, batch);
        for dir in [&sidecar_dir, &classfile_dir] {
            let classfile = dir.join(class_path);
            let masked = without_method(&fs::read(&classfile).unwrap(), member, descriptor);
            fs::write(&classfile, masked).unwrap();
        }
        let source = case.join("Lookup.scala");
        fs::write(
            &source,
            format!("package example\nobject Lookup {{ val selected = {{ {expression} }} }}\n"),
        )
        .unwrap();
        let classpath = format!("{}:{}", sidecar_dir.display(), scala.libraries);
        let sidecar_result = scala.compile_result(&classpath, &sidecar_out, &source);
        assert!(
            sidecar_result.status.success(),
            "original TASTy lookup for {member}{descriptor} failed: {}",
            String::from_utf8_lossy(&sidecar_result.stderr)
        );

        let classfile = classfile_dir.join(class_path);
        fs::write(
            &classfile,
            without_tasty_marker(&fs::read(&classfile).unwrap()),
        )
        .unwrap();
        fs::remove_file(classfile_dir.join(class_path.replace(".class", ".tasty"))).unwrap();
        let control = scala.compile_result(
            &format!("{}:{}", classfile_dir.display(), scala.libraries),
            &classfile_out,
            &source,
        );
        assert!(
            !control.status.success(),
            "classfile-only lookup resolved {member}{descriptor}"
        );
        let diagnostic = first_error(&control);
        assert!(
            diagnostic.contains(missing),
            "unexpected classfile-only first diagnostic for {label}: {diagnostic}"
        );
        eprintln!("TASTY_LOOKUP_ACCEPTED {label}; CLASSFILE_ONLY_FIRST_REJECTION {diagnostic}");
        assert_unchanged(producer, before, "during TASTy lookup probes");
    }
    let _ = batch;
}

fn inspect_original_tasty(
    scala: &Scala,
    producer: &Path,
    root: &Path,
    before: &[(String, String)],
    method: &str,
    reversed: bool,
) {
    let from_tasty_dir = root.join("scala-from-tasty-inspection-only");
    fs::create_dir_all(&from_tasty_dir).unwrap();
    let mut factory_decompile = String::new();
    let mut factory_print = String::new();
    let sidecars = ["Counter", "Pair", "PairFactory"]
        .map(|name| producer.join(format!("example/{name}.tasty")));
    for (name, sidecar) in ["Counter", "Pair", "PairFactory"]
        .into_iter()
        .zip(&sidecars)
    {
        let decompile = scala
            .command()
            .arg("-decompile")
            .arg(sidecar)
            .output()
            .expect("run official Scala -decompile");
        assert!(
            decompile.status.success(),
            "Scala -decompile rejected Rust {name}.tasty: {}",
            String::from_utf8_lossy(&decompile.stderr)
        );
        let dump = strip_ansi(&String::from_utf8_lossy(&decompile.stdout));
        match name {
            "Counter" => {
                for member in ["def get", "def set"] {
                    assert!(
                        dump.contains(member),
                        "Counter decompile lacks {member}: {dump}"
                    );
                }
            }
            "Pair" => {
                for member in [
                    "class Pair(first: example.Counter, second: example.Counter)",
                    "def leftValue",
                    "def rightValue",
                ] {
                    assert!(
                        dump.contains(member),
                        "Pair decompile lacks {member}: {dump}"
                    );
                }
            }
            "PairFactory" => {
                assert!(
                    dump.contains(&format!(
                        "def {method}(existing: example.Counter, seed: scala.Int): example.Pair"
                    )),
                    "wrong mixed signature: {dump}"
                );
                let expected_body = if reversed {
                    "new example.Pair(new example.Counter(seed), existing)"
                } else {
                    "new example.Pair(existing, new example.Counter(seed))"
                };
                assert!(
                    dump.contains(expected_body),
                    "wrong direct/nested expression order: {dump}"
                );
                factory_decompile = dump;
            }
            _ => unreachable!(),
        }
        let printed = scala
            .command()
            .arg("-print-tasty")
            .arg(sidecar)
            .output()
            .expect("run official Scala -print-tasty");
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
        if name == "PairFactory" {
            for proof in [
                "PARAM(7)",
                "TERMREFdirect 32",
                "TERMREFdirect 41",
                "SELECTin(8) 18 [<init>[Signed Signature(List(example.Counter, example.Counter),example.Pair)",
                "SELECTin(8) 20 [<init>[Signed Signature(List(scala.Int),example.Counter)",
            ] {
                assert!(dump.contains(proof), "TASTy dump lacks {proof}: {dump}");
            }
            if reversed {
                assert!(dump.find("APPLY(12)") < dump.find("TERMREFdirect 41"));
                assert!(dump.find("TERMREFdirect 41") < dump.find("TERMREFdirect 32"));
            } else {
                assert!(dump.find("TERMREFdirect 32") < dump.find("APPLY(12)"));
                assert!(dump.find("APPLY(12)") < dump.find("TERMREFdirect 41"));
            }
            factory_print = dump;
        }
        eprintln!("SCALA_PRINT_TASTY {name} sha256={}", sha256(sidecar));
    }
    let reconstructed = scala
        .command()
        .args(["-release", "21", "-from-tasty", "-classpath"])
        .arg(format!("{}:{}", producer.display(), scala.libraries))
        .arg("-d")
        .arg(&from_tasty_dir)
        .args(&sidecars)
        .output()
        .expect("run official Scala -from-tasty");
    assert!(
        reconstructed.status.success(),
        "Scala -from-tasty rejected original Rust sidecars: {}",
        String::from_utf8_lossy(&reconstructed.stderr)
    );
    for name in ["Counter", "Pair", "PairFactory"] {
        assert!(
            from_tasty_dir
                .join(format!("example/{name}.class"))
                .is_file(),
            "from-tasty reconstructs {name} in inspection-only directory"
        );
    }
    assert!(factory_decompile.contains("existing") && factory_decompile.contains("seed"));
    assert!(factory_print.contains("TERMREFdirect"));
    assert_unchanged(producer, before, "after official sidecar inspection");
    eprintln!(
        "SCALA_FROM_TASTY_OUTPUT {} (inspection-only)",
        from_tasty_dir.display()
    );
}

fn inspect_renamed_tasty(scala: &Scala, producer: &Path, root: &Path, before: &[(String, String)]) {
    let sidecars = [
        "renamed/example/Ledger.tasty",
        "renamed/example/Duo.tasty",
        "renamed/example/Factory.tasty",
    ]
    .map(|path| producer.join(path));
    for (name, expected) in [
        ("Ledger", "def get"),
        ("Duo", "def leftValue"),
        (
            "Factory",
            "def forge(existing: renamed.example.Ledger, seed: scala.Int): renamed.example.Duo",
        ),
    ] {
        let path = sidecars
            .iter()
            .find(|path| path.file_name().unwrap().to_string_lossy() == format!("{name}.tasty"))
            .unwrap();
        let decompiled = scala
            .command()
            .arg("-decompile")
            .arg(path)
            .output()
            .expect("run Scala decompiler for renamed Rust sidecar");
        assert!(
            decompiled.status.success(),
            "Scala -decompile rejected renamed {name}.tasty: {}",
            String::from_utf8_lossy(&decompiled.stderr)
        );
        assert!(
            strip_ansi(&String::from_utf8_lossy(&decompiled.stdout)).contains(expected),
            "renamed {name} TASTy omits {expected}: {}",
            String::from_utf8_lossy(&decompiled.stdout)
        );
        let printed = scala
            .command()
            .arg("-print-tasty")
            .arg(path)
            .output()
            .expect("run Scala TASTy printer for renamed Rust sidecar");
        assert!(
            printed.status.success(),
            "Scala -print-tasty rejected renamed {name}: {}",
            String::from_utf8_lossy(&printed.stderr)
        );
        assert!(strip_ansi(&String::from_utf8_lossy(&printed.stdout)).contains("version: 28.9.0"));
    }
    let reconstructed = root.join("renamed-from-tasty-inspection-only");
    fs::create_dir_all(&reconstructed).unwrap();
    let result = scala
        .command()
        .args(["-release", "21", "-from-tasty", "-classpath"])
        .arg(format!("{}:{}", producer.display(), scala.libraries))
        .arg("-d")
        .arg(&reconstructed)
        .args(&sidecars)
        .output()
        .expect("run Scala -from-tasty for renamed sidecars");
    assert!(
        result.status.success(),
        "Scala -from-tasty rejected renamed Rust sidecars: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    for name in ["Ledger", "Duo", "Factory"] {
        assert!(
            reconstructed
                .join(format!("renamed/example/{name}.class"))
                .is_file()
        );
    }
    assert_unchanged(producer, before, "after renamed TASTy reader controls");
}

fn assert_repeatable(first: &PairedJvmTastyBatchArtifacts, second: &PairedJvmTastyBatchArtifacts) {
    assert_eq!(first.classes().len(), 3);
    assert_eq!(second.classes().len(), 3);
    for (left, right) in first.classes().iter().zip(second.classes()) {
        assert_eq!(
            left.classfile_bytes(),
            right.classfile_bytes(),
            "stable {} class bytes",
            left.classfile_path()
        );
        assert_eq!(
            left.tasty_bytes(),
            right.tasty_bytes(),
            "stable {} TASTy bytes",
            left.tasty_path()
        );
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
    .expect("fresh Rust session emits same source batch");
    assert_repeatable(batch, &repeated);
    let producer = root.join(format!("producer-{source_id}"));
    let client_out = root.join(format!("client-{source_id}"));
    fs::create_dir_all(&producer).unwrap();
    fs::create_dir_all(&client_out).unwrap();
    write_batch(&producer, batch);
    let original_hashes = hashes(batch, &producer);
    let producer_source = root.join(format!("Producer-{source_id}.scala"));
    fs::write(&producer_source, source).unwrap();
    eprintln!(
        "RUST_PRODUCER_SOURCE_SHA256 {} {}",
        producer_source.display(),
        sha256(&producer_source)
    );
    let source_path = root.join(format!("Consumer-{source_id}.scala"));
    fs::write(&source_path, client).unwrap();
    let client_hash = sha256(&source_path);
    eprintln!(
        "SCALA_CLIENT_SHA256 {} {client_hash}",
        source_path.display()
    );
    scala.compile(
        &format!("{}:{}", producer.display(), scala.libraries),
        &client_out,
        &source_path,
    );
    assert_unchanged(
        &producer,
        &original_hashes,
        "after independent Scala compile",
    );
    let classpath = format!(
        "{}:{}:{}",
        client_out.display(),
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
            .arg(entry),
    );
    let stdout = String::from_utf8_lossy(&java.stdout);
    assert!(
        stdout.contains("formal-identity=true; fresh-counter=true; isolated-mutation=true"),
        "unexpected {entry} output: {stdout}"
    );
    eprintln!("SCALA_RUNTIME_OUTPUT {}", stdout.trim());
    assert_unchanged(&producer, &original_hashes, "after Scala runtime");
    let reversed = source.contains("createAndWrap");
    if source.contains("package renamed.example") {
        inspect_renamed_tasty(scala, &producer, root, &original_hashes);
    } else {
        let factory_method = if reversed {
            "createAndWrap"
        } else {
            "wrapAndCreate"
        };
        inspect_original_tasty(
            scala,
            &producer,
            root,
            &original_hashes,
            factory_method,
            reversed,
        );
        lookup_probes(
            scala,
            root,
            batch,
            &producer,
            &original_hashes,
            factory_method,
        );
    }
}

fn renamed_variant(source: &str, client: &str) -> (String, String) {
    let replace = |text: &str| {
        text.replace("package example", "package renamed.example")
            .replace("example.", "renamed.example.")
            .replace("PairFactory", "Factory")
            .replace("Counter", "Ledger")
            .replace("Pair", "Duo")
            .replace("wrapAndCreate", "forge")
    };
    (replace(source), replace(client))
}

fn reordered_variant(source: &str) -> String {
    let counter = source.find("class Counter").unwrap();
    let pair = source.find("class Pair(").unwrap();
    let factory = source.find("class PairFactory:").unwrap();
    format!(
        "{}{}/{}/{}",
        &source[..counter],
        &source[factory..],
        &source[pair..factory],
        &source[counter..pair]
    )
    .replace("/", "\n")
}

fn sidecar_negative_probes(scala: &Scala, root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for class_name in ["Counter", "Pair", "PairFactory"] {
        let class_path = format!("example/{class_name}.class");
        let tasty_path = format!("example/{class_name}.tasty");
        for corruption in ["missing", "misplaced", "corrupt", "mismatched-uuid"] {
            let label = format!("{class_name}-{corruption}");
            let case = root.join(format!("negative-{label}"));
            let classes = case.join("classes");
            let out = case.join("out");
            fs::create_dir_all(&classes).unwrap();
            fs::create_dir_all(&out).unwrap();
            write_batch(&classes, batch);
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
                    let classfile = classes.join(&class_path);
                    let changed = stale_tasty_uuid(
                        &fs::read(&classfile).unwrap(),
                        &fs::read(&tasty).unwrap(),
                    );
                    fs::write(classfile, changed).unwrap();
                }
                _ => unreachable!(),
            }
            let client = case.join("Consumer.scala");
            fs::write(&client, CLIENT).unwrap();
            let result = scala.compile_result(
                &format!("{}:{}", classes.display(), scala.libraries),
                &out,
                &client,
            );
            assert!(!result.status.success(), "Scala accepted {label}");
            let diagnostic = first_error(&result);
            let stderr = strip_ansi(&String::from_utf8_lossy(&result.stderr));
            match corruption {
                "missing" | "misplaced" => assert!(
                    stderr.contains(".tasty` file is missing"),
                    "wrong loader rejection for {label}: {stderr}"
                ),
                "corrupt" => assert!(
                    stderr.contains(&format!("error while loading {class_name}"))
                        && stderr.contains(".tasty file")
                        && stderr.contains("is broken"),
                    "wrong TASTy reader rejection for {label}: {stderr}"
                ),
                "mismatched-uuid" => assert!(
                    stderr.contains("out of sync with its TASTy file"),
                    "wrong UUID rejection for {label}: {stderr}"
                ),
                _ => unreachable!(),
            }
            eprintln!("SCALA_NEGATIVE_FIRST_FAILURE {label}: {diagnostic}");
        }
    }
}

fn source_negative_probes(scala: &Scala, root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for (label, source, expected) in [
        (
            "wrong-pair-arity",
            "package example\nobject Probe { val pair = new Pair(new Counter(1)) }\n",
            "[E171] Type Error",
        ),
        (
            "wrong-pair-argument-type",
            "package example\nclass Wrong\nobject Probe { val pair = new Pair(new Counter(1), new Wrong()) }\n",
            "[E007] Type Mismatch Error",
        ),
        (
            "wrong-package",
            "package wrong\nobject Probe { val pair = new PairFactory().wrapAndCreate(new Counter(1), 2) }\n",
            "[E006] Not Found Error",
        ),
    ] {
        let case = root.join(format!("negative-{label}"));
        let classes = case.join("classes");
        let out = case.join("out");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&out).unwrap();
        write_batch(&classes, batch);
        let client = case.join("Probe.scala");
        fs::write(&client, source).unwrap();
        let result = scala.compile_result(
            &format!("{}:{}", classes.display(), scala.libraries),
            &out,
            &client,
        );
        assert!(!result.status.success(), "Scala accepted {label}");
        let diagnostic = first_error(&result);
        assert!(
            diagnostic.contains(expected),
            "wrong first Scala rejection for {label}: {diagnostic}"
        );
        eprintln!("SCALA_NEGATIVE_FIRST_FAILURE {label}: {diagnostic}");
    }

    let case = root.join("negative-corrupt-formal-termref");
    let classes = case.join("classes");
    let out = case.join("out");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&out).unwrap();
    write_batch(&classes, batch);
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    let (existing, _) = formal_addresses(factory.tasty_bytes(), "wrapAndCreate");
    fs::write(
        classes.join(factory.tasty_path()),
        change_formal_target(factory.tasty_bytes(), existing, 127),
    )
    .unwrap();
    let client = case.join("Consumer.scala");
    fs::write(&client, CLIENT).unwrap();
    let result = scala.compile_result(
        &format!("{}:{}", classes.display(), scala.libraries),
        &out,
        &client,
    );
    assert!(
        result.status.success(),
        "signature-only Scala lookup unexpectedly forced factory body: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let forced = scala
        .command()
        .arg("-decompile")
        .arg(classes.join(factory.tasty_path()))
        .output()
        .expect("run official Scala reader against corrupted formal reference");
    assert!(
        !forced.status.success(),
        "official reader accepted an invalid original formal TERMREF address"
    );
    let forced_diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&forced.stdout),
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(
        forced_diagnostic.contains("TreeWithoutOwner"),
        "wrong first failure for corrupted formal TERMREF: {forced_diagnostic}"
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE corrupt-formal-termref: official -decompile: TreeWithoutOwner"
    );

    let seed_case = root.join("negative-corrupt-seed-termref");
    let seed_classes = seed_case.join("classes");
    fs::create_dir_all(&seed_classes).unwrap();
    write_batch(&seed_classes, batch);
    let (_, seed_address) = formal_addresses(factory.tasty_bytes(), "wrapAndCreate");
    fs::write(
        seed_classes.join(factory.tasty_path()),
        change_formal_target(factory.tasty_bytes(), seed_address, 127),
    )
    .unwrap();
    let seed_forced = scala
        .command()
        .arg("-decompile")
        .arg(seed_classes.join(factory.tasty_path()))
        .output()
        .expect("run official Scala reader against corrupted seed reference");
    assert!(
        !seed_forced.status.success(),
        "official reader accepted invalid seed TERMREF address"
    );
    let seed_diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&seed_forced.stdout),
        String::from_utf8_lossy(&seed_forced.stderr)
    );
    assert!(
        seed_diagnostic.contains("TreeWithoutOwner"),
        "wrong first failure for corrupted seed TERMREF: {seed_diagnostic}"
    );
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE corrupt-seed-termref: official -decompile: TreeWithoutOwner"
    );

    let case = root.join("negative-renamed-factory-method-in-tasty");
    let classes = case.join("classes");
    let out = case.join("out");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&out).unwrap();
    write_batch(&classes, batch);
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.tasty_path() == "example/PairFactory.tasty")
        .unwrap();
    fs::write(
        classes.join(factory.tasty_path()),
        rename_tasty_method(factory.tasty_bytes(), b"wrapAndCreate", b"dropAndCreate"),
    )
    .unwrap();
    let client = case.join("Consumer.scala");
    fs::write(&client, CLIENT).unwrap();
    let result = scala.compile_result(
        &format!("{}:{}", classes.display(), scala.libraries),
        &out,
        &client,
    );
    assert!(
        !result.status.success(),
        "Scala resolved a renamed-away TASTy method"
    );
    assert!(first_error(&result).contains("[E008] Not Found Error"));
    eprintln!(
        "SCALA_NEGATIVE_FIRST_FAILURE renamed-factory-method: {}",
        first_error(&result)
    );

    let missing_counter_case = root.join("negative-missing-counter-class");
    let missing_counter_classes = missing_counter_case.join("classes");
    let missing_counter_out = missing_counter_case.join("out");
    fs::create_dir_all(&missing_counter_classes).unwrap();
    fs::create_dir_all(&missing_counter_out).unwrap();
    write_batch(&missing_counter_classes, batch);
    fs::remove_file(missing_counter_classes.join("example/Counter.class")).unwrap();
    fs::remove_file(missing_counter_classes.join("example/Counter.tasty")).unwrap();
    let missing_counter_client = missing_counter_case.join("Consumer.scala");
    fs::write(&missing_counter_client, CLIENT).unwrap();
    let missing_counter = scala.compile_result(
        &format!("{}:{}", missing_counter_classes.display(), scala.libraries),
        &missing_counter_out,
        &missing_counter_client,
    );
    assert!(
        !missing_counter.status.success(),
        "Scala accepted the Factory signature with a missing Counter class"
    );
    let missing_counter_error = first_error(&missing_counter);
    assert!(
        missing_counter_error.contains("Counter") || missing_counter_error.contains("Not Found"),
        "wrong missing Counter first failure: {missing_counter_error}"
    );
    eprintln!("SCALA_NEGATIVE_FIRST_FAILURE missing-counter-class: {missing_counter_error}");

    let ctor_case = root.join("negative-missing-counter-constructor");
    let ctor_classes = ctor_case.join("classes");
    let ctor_out = ctor_case.join("out");
    fs::create_dir_all(&ctor_classes).unwrap();
    fs::create_dir_all(&ctor_out).unwrap();
    write_batch(&ctor_classes, batch);
    let counter = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/Counter.class")
        .unwrap();
    fs::write(
        ctor_classes.join("example/Counter.class"),
        without_method(counter.classfile_bytes(), "<init>", "(I)V"),
    )
    .unwrap();
    let ctor_client = ctor_case.join("Consumer.scala");
    fs::write(&ctor_client, CLIENT).unwrap();
    let ctor_compile = scala.compile_result(
        &format!("{}:{}", ctor_classes.display(), scala.libraries),
        &ctor_out,
        &ctor_client,
    );
    assert!(
        ctor_compile.status.success(),
        "TASTy source lookup should retain the Counter constructor signature: {}",
        String::from_utf8_lossy(&ctor_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        ctor_out.display(),
        ctor_classes.display(),
        scala.libraries
    );
    let ctor_runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("Consumer")
        .output()
        .expect("run client against missing Counter constructor");
    assert!(
        !ctor_runtime.status.success(),
        "missing Counter constructor unexpectedly linked"
    );
    let ctor_error = format!(
        "{}{}",
        String::from_utf8_lossy(&ctor_runtime.stdout),
        String::from_utf8_lossy(&ctor_runtime.stderr)
    );
    assert!(
        ctor_error.contains("NoSuchMethodError") || ctor_error.contains("LinkageError"),
        "wrong missing Counter constructor failure: {ctor_error}"
    );
    eprintln!(
        "JVM_NEGATIVE_FIRST_FAILURE missing-counter-constructor: {}",
        first_error(&ctor_runtime)
    );

    let external_case = root.join("negative-same-spelled-external-counter");
    let external_classes = external_case.join("classes");
    let external_out = external_case.join("out");
    fs::create_dir_all(&external_classes).unwrap();
    fs::create_dir_all(&external_out).unwrap();
    write_batch(&external_classes, batch);
    let external_client = external_case.join("Probe.scala");
    fs::write(&external_client, "package foreign:\n  class Counter(val value: Int)\npackage example:\n  object Probe:\n    val pair = new PairFactory().wrapAndCreate(new foreign.Counter(1), 2)\n").unwrap();
    let external = scala.compile_result(
        &format!("{}:{}", external_classes.display(), scala.libraries),
        &external_out,
        &external_client,
    );
    assert!(
        !external.status.success(),
        "Scala accepted a foreign same-spelled Counter as example.Counter"
    );
    let external_error = first_error(&external);
    assert!(
        external_error.contains("Type Mismatch") || external_error.contains("E007"),
        "wrong foreign-owner first failure: {external_error}"
    );
    eprintln!("SCALA_NEGATIVE_FIRST_FAILURE foreign-counter-owner: {external_error}");

    let methodref_case = root.join("negative-wrong-factory-constructor-methodref");
    let methodref_classes = methodref_case.join("classes");
    let methodref_out = methodref_case.join("out");
    fs::create_dir_all(&methodref_classes).unwrap();
    fs::create_dir_all(&methodref_out).unwrap();
    write_batch(&methodref_classes, batch);
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .unwrap();
    fs::write(
        methodref_classes.join("example/PairFactory.class"),
        wrong_factory_constructor_methodref(factory.classfile_bytes()),
    )
    .unwrap();
    let methodref_client = methodref_case.join("Consumer.scala");
    fs::write(&methodref_client, CLIENT).unwrap();
    let methodref_compile = scala.compile_result(
        &format!("{}:{}", methodref_classes.display(), scala.libraries),
        &methodref_out,
        &methodref_client,
    );
    assert!(
        methodref_compile.status.success(),
        "TASTy lookup should accept independent classfile Methodref mutation: {}",
        String::from_utf8_lossy(&methodref_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        methodref_out.display(),
        methodref_classes.display(),
        scala.libraries
    );
    let runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("Consumer")
        .output()
        .expect("run client against wrong classfile constructor Methodref");
    assert!(
        !runtime.status.success(),
        "wrong classfile constructor Methodref unexpectedly ran"
    );
    let runtime_diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&runtime.stdout),
        String::from_utf8_lossy(&runtime.stderr)
    );
    assert!(
        runtime_diagnostic.contains("VerifyError") || runtime_diagnostic.contains("LinkageError"),
        "wrong first JVM failure for classfile Methodref: {runtime_diagnostic}"
    );
    eprintln!(
        "JVM_NEGATIVE_FIRST_FAILURE wrong-pair-constructor-methodref: {}",
        first_error(&runtime)
    );

    let descriptor_case = root.join("negative-wrong-factory-descriptor");
    let descriptor_classes = descriptor_case.join("classes");
    let descriptor_out = descriptor_case.join("out");
    fs::create_dir_all(&descriptor_classes).unwrap();
    fs::create_dir_all(&descriptor_out).unwrap();
    write_batch(&descriptor_classes, batch);
    fs::write(
        descriptor_classes.join("example/PairFactory.class"),
        wrong_method_descriptor(
            factory.classfile_bytes(),
            "wrapAndCreate",
            "(Lexample/Counter;I)Lexample/Pair;",
        ),
    )
    .unwrap();
    let descriptor_client = descriptor_case.join("Consumer.scala");
    fs::write(&descriptor_client, CLIENT).unwrap();
    let descriptor_compile = scala.compile_result(
        &format!("{}:{}", descriptor_classes.display(), scala.libraries),
        &descriptor_out,
        &descriptor_client,
    );
    assert!(
        descriptor_compile.status.success(),
        "TASTy lookup did not retain the declared Factory signature: {}",
        String::from_utf8_lossy(&descriptor_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        descriptor_out.display(),
        descriptor_classes.display(),
        scala.libraries
    );
    let runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("Consumer")
        .output()
        .expect("run client against wrong Factory classfile descriptor");
    assert!(
        !runtime.status.success(),
        "wrong Factory classfile descriptor unexpectedly linked"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&runtime.stdout),
        String::from_utf8_lossy(&runtime.stderr)
    );
    assert!(
        diagnostic.contains("NoSuchMethodError")
            || diagnostic.contains("LinkageError")
            || diagnostic.contains("VerifyError"),
        "wrong first JVM failure for Factory descriptor: {diagnostic}"
    );
    eprintln!(
        "JVM_NEGATIVE_FIRST_FAILURE wrong-factory-descriptor: {}",
        first_error(&runtime)
    );

    let fieldref_case = root.join("negative-wrong-pair-getter-fieldref");
    let fieldref_classes = fieldref_case.join("classes");
    let fieldref_out = fieldref_case.join("out");
    fs::create_dir_all(&fieldref_classes).unwrap();
    fs::create_dir_all(&fieldref_out).unwrap();
    write_batch(&fieldref_classes, batch);
    let pair = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/Pair.class")
        .unwrap();
    fs::write(
        fieldref_classes.join("example/Pair.class"),
        wrong_pair_getter_fieldref(pair.classfile_bytes()),
    )
    .unwrap();
    let fieldref_client = fieldref_case.join("Consumer.scala");
    fs::write(&fieldref_client, CLIENT).unwrap();
    let fieldref_compile = scala.compile_result(
        &format!("{}:{}", fieldref_classes.display(), scala.libraries),
        &fieldref_out,
        &fieldref_client,
    );
    assert!(
        fieldref_compile.status.success(),
        "TASTy source lookup should accept a JVM-valid same-typed Fieldref swap: {}",
        String::from_utf8_lossy(&fieldref_compile.stderr)
    );
    let classpath = format!(
        "{}:{}:{}",
        fieldref_out.display(),
        fieldref_classes.display(),
        scala.libraries
    );
    let runtime = Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg("Consumer")
        .output()
        .expect("run client against same-typed but wrong Pair Fieldref");
    assert!(
        !runtime.status.success(),
        "wrong Pair Fieldref escaped the original-object identity assertions"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&runtime.stdout),
        String::from_utf8_lossy(&runtime.stderr)
    );
    assert!(
        diagnostic.contains("AssertionError"),
        "wrong first JVM-valid Fieldref behavior was not caught by identity checks: {diagnostic}"
    );
    eprintln!(
        "JVM_NEGATIVE_SEMANTIC_FAILURE wrong-pair-getter-fieldref: identity assertion catches left/right swap"
    );
}

#[test]
fn official_scala_consumes_original_rust_mixed_origin_constructor_batch() {
    if !required() {
        return;
    }
    assert_eq!(fs::read_to_string("tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala").unwrap(), FORWARD);
    assert_eq!(fs::read_to_string("tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala").unwrap(), REVERSE);
    assert_eq!(
        fs::read_to_string(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/Consumer.scala"
        )
        .unwrap(),
        CLIENT
    );
    assert_eq!(
        fs::read_to_string(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/ConsumerReversed.scala"
        )
        .unwrap(),
        REVERSED_CLIENT
    );
    eprintln!(
        "SOURCE_SHA256 forward={} reverse={} client={} reversed_client={} scala_revision={SCALA_REVISION}",
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
        )),
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala"
        )),
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/Consumer.scala"
        )),
        sha256(Path::new(
            "tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/ConsumerReversed.scala"
        ))
    );
    let scala = Scala::from_environment();
    let root = task_dir();
    for (source, client, entry, source_id) in [
        (FORWARD, CLIENT, "Consumer", 20170),
        (REVERSE, REVERSED_CLIENT, "ConsumerReversed", 20172),
    ] {
        let batch = emit_paired_jvm_tasty_batch(
            source,
            "PairFactory.scala",
            SourceId::from_index(source_id),
        )
        .expect("one original Rust frontend session emits all six artifacts");
        compile_and_run(&scala, &root, &batch, source, client, entry, source_id);
    }
    let forward_batch =
        emit_paired_jvm_tasty_batch(FORWARD, "PairFactory.scala", SourceId::from_index(20174))
            .expect("negative controls use authentic original Rust batch");
    sidecar_negative_probes(&scala, &root, &forward_batch);
    source_negative_probes(&scala, &root, &forward_batch);

    let (renamed_source, renamed_client) = renamed_variant(FORWARD, CLIENT);
    let renamed_batch = emit_paired_jvm_tasty_batch(
        &renamed_source,
        "PairFactory.scala",
        SourceId::from_index(20176),
    )
    .expect("renamed variant is generated by a fresh authentic Rust frontend session");
    compile_and_run(
        &scala,
        &root,
        &renamed_batch,
        &renamed_source,
        &renamed_client,
        "Consumer",
        20176,
    );

    let reordered_source = reordered_variant(FORWARD);
    let reordered_batch = emit_paired_jvm_tasty_batch(
        &reordered_source,
        "PairFactory.scala",
        SourceId::from_index(20178),
    )
    .expect("reordered declarations are generated by a fresh authentic Rust frontend session");
    compile_and_run(
        &scala,
        &root,
        &reordered_batch,
        &reordered_source,
        CLIENT,
        "Consumer",
        20178,
    );
}

#[test]
fn mixed_origin_factory_classfile_has_expected_rust_descriptor_for_lookup_masking() {
    let batch =
        emit_paired_jvm_tasty_batch(FORWARD, "PairFactory.scala", SourceId::from_index(20179))
            .unwrap();
    let factory = batch
        .classes()
        .iter()
        .find(|artifact| artifact.classfile_path() == "example/PairFactory.class")
        .unwrap();
    let class = decode_class(factory.classfile_bytes());
    let methods = class
        .methods
        .iter()
        .map(|method| {
            (
                class.constant_pool.utf8(method.name_index).unwrap(),
                class.constant_pool.utf8(method.descriptor_index).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        methods.contains(&("wrapAndCreate", "(Lexample/Counter;I)Lexample/Pair;")),
        "{methods:?}"
    );
    without_method(
        factory.classfile_bytes(),
        "wrapAndCreate",
        "(Lexample/Counter;I)Lexample/Pair;",
    );
}
