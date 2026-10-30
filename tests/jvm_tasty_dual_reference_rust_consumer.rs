use dotty::jvm_tasty_artifacts::{PairedJvmTastyBatchArtifacts, emit_paired_jvm_tasty_batch};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_core::SourceId;
use dotty_tasty::ast::TERMREFSYMBOL_TAG;
use dotty_tasty::section::StandardSection;
use dotty_tasty::tasty::TastyFile;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const REQUIRE: &str = "DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const PAIR_SOURCE_SHA256: &str = "06266874ed9a7acdfc4d6a1c27bea3967055e474bca5a2f666af5ebfc8669c28";
const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");
const CLIENT: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/RustArtifactConsumer.scala");

fn required() -> bool {
    if std::env::var_os(REQUIRE).is_none() {
        eprintln!("skipping strict Scala consumer; set {REQUIRE}=1 to require it");
        return false;
    }
    true
}

fn run(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"));
    assert!(
        output.status.success(),
        "command failed: {rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn task_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "dotty-issue-1881-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create task-local temporary directory");
    path
}

fn producer(source_id: u32) -> PairedJvmTastyBatchArtifacts {
    emit_paired_jvm_tasty_batch(SOURCE, "Pair.scala", SourceId::from_index(source_id))
        .expect("one authentic Rust frontend session emits the complete Pair batch")
}

fn write_batch(root: &Path, batch: &PairedJvmTastyBatchArtifacts) {
    for class in batch.classes() {
        for (relative, bytes) in [
            (class.classfile_path(), class.classfile_bytes()),
            (class.tasty_path(), class.tasty_bytes()),
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("artifact has a parent")).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }
}

fn sha256(path: &Path) -> String {
    let output = run(Command::new("sha256sum").arg(path));
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum emits a digest")
        .to_owned()
}

fn artifact_hashes(root: &Path) -> Vec<(String, String)> {
    [
        "example/Counter.class",
        "example/Counter.tasty",
        "example/Pair.class",
        "example/Pair.tasty",
    ]
    .into_iter()
    .map(|relative| {
        let path = root.join(relative);
        (relative.to_owned(), sha256(&path))
    })
    .collect()
}

fn uuid_text(bytes: &[u8; 16]) -> String {
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

struct Scala {
    java_home: PathBuf,
    scalac: PathBuf,
    java: PathBuf,
    scala_libraries: String,
}

impl Scala {
    fn from_environment() -> Self {
        let root =
            PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
        let java_home =
            PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
        let scala_libraries = [
            root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"),
            root.join("dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"),
        ];
        let expected_hashes = [
            "8c605e1945ae5a3c57d618d02241f7190ecbed744367473c7c3a9b61940c920c",
            "bc87575e0099514b17f7e84384b4a9a51d7a376720af8974e04a41a202909434",
        ];
        for (path, expected) in scala_libraries.iter().zip(expected_hashes) {
            assert!(
                path.is_file(),
                "pinned Scala runtime exists: {}",
                path.display()
            );
            assert_eq!(
                sha256(path),
                expected,
                "approved Scala runtime hash: {}",
                path.display()
            );
        }

        let revision = run(Command::new("git")
            .args(["-C"])
            .arg(&root)
            .args(["rev-parse", "HEAD"]));
        assert_eq!(
            String::from_utf8_lossy(&revision.stdout).trim(),
            SCALA_REVISION
        );
        let scalac = root.join("dist/target/universal/stage/bin/scalac");
        let version = run(Command::new(&scalac).arg("-version"));
        let version_text = String::from_utf8_lossy(&version.stderr).to_string()
            + &String::from_utf8_lossy(&version.stdout);
        assert!(
            version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
            "{version_text}"
        );
        assert_eq!(
            sha256(&root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-compiler_3/3.9.0-bin-SNAPSHOT/scala3-compiler_3-3.9.0-bin-SNAPSHOT.jar")),
            "b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f",
            "pinned compiler jar hash"
        );
        let java = java_home.join("bin/java");
        let java_version = run(Command::new(&java).arg("-version"));
        let java_text = String::from_utf8_lossy(&java_version.stderr).to_string()
            + &String::from_utf8_lossy(&java_version.stdout);
        assert!(java_text.contains("21.0.13"), "{java_text}");
        assert_eq!(
            sha256(&java),
            "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155",
            "pinned Java executable hash"
        );

        Self {
            java_home,
            scalac,
            java,
            scala_libraries: scala_libraries
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        }
    }

    fn scalac(&self) -> Command {
        let mut command = Command::new(&self.scalac);
        command.env("JAVA_HOME", &self.java_home);
        if let Some(path) = std::env::var_os("PATH") {
            let mut paths = vec![self.java_home.join("bin")];
            paths.extend(std::env::split_paths(&path));
            command.env("PATH", std::env::join_paths(paths).expect("PATH is valid"));
        }
        command
    }

    fn compile(&self, classpath: &str, output: &Path, source: &Path) -> Output {
        run(self
            .scalac()
            .args(["-Werror", "-release", "21", "-classpath"])
            .arg(classpath)
            .arg("-d")
            .arg(output)
            .arg(source))
    }

    fn compile_result(&self, classpath: &str, output: &Path, source: &Path) -> Output {
        self.scalac()
            .args(["-Werror", "-release", "21", "-classpath"])
            .arg(classpath)
            .arg("-d")
            .arg(output)
            .arg(source)
            .output()
            .expect("run pinned Scala reader probe")
    }

    fn classpath(&self, producer: &Path) -> String {
        format!("{}:{}", producer.display(), self.scala_libraries)
    }
}

fn decode_class(bytes: &[u8]) -> ClassFile<'_> {
    let mut reader = Reader::new(bytes);
    let class = ClassFile::decode(&mut reader).expect("decode producer classfile");
    assert!(reader.is_at_end());
    class
}

fn without_method(bytes: &[u8], name: &str, descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let before = class.methods.len();
    class.methods.retain(|method| {
        pool.utf8(method.name_index).unwrap() != name
            || pool.utf8(method.descriptor_index).unwrap() != descriptor
    });
    assert_eq!(
        class.methods.len() + 1,
        before,
        "found one {name}{descriptor}"
    );
    class
        .encode()
        .expect("encode structurally valid mutated classfile")
}

fn wrong_pair_constructor_descriptor(bytes: &[u8]) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let no_argument_void = (1..=u16::MAX)
        .map(dotty_classfile::constant_pool::ConstantPoolIndex)
        .find(|index| class.constant_pool.utf8(*index).ok() == Some("()V"))
        .expect("Pair constant pool contains the Object constructor descriptor");
    let constructor = class
        .methods
        .iter_mut()
        .find(|method| class.constant_pool.utf8(method.name_index).ok() == Some("<init>"))
        .expect("Pair has a primary constructor");
    constructor.descriptor_index = no_argument_void;
    class
        .encode()
        .expect("wrong-arity constructor descriptor remains structurally encodable")
}

fn wrong_method_descriptor(bytes: &[u8], name: &str, expected_descriptor: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let no_argument_void = (1..=u16::MAX)
        .map(dotty_classfile::constant_pool::ConstantPoolIndex)
        .find(|index| class.constant_pool.utf8(*index).ok() == Some("()V"))
        .expect("class constant pool contains the Object constructor descriptor");
    let pool = &class.constant_pool;
    let method = class
        .methods
        .iter_mut()
        .find(|method| {
            pool.utf8(method.name_index).ok() == Some(name)
                && pool.utf8(method.descriptor_index).ok() == Some(expected_descriptor)
        })
        .expect("requested method is in the classfile");
    method.descriptor_index = no_argument_void;
    class
        .encode()
        .expect("altered method descriptor remains structurally encodable")
}

fn without_field(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut class = decode_class(bytes);
    let pool = &class.constant_pool;
    let before = class.fields.len();
    class
        .fields
        .retain(|field| pool.utf8(field.name_index).unwrap() != name);
    assert_eq!(class.fields.len() + 1, before, "found field {name}");
    class
        .encode()
        .expect("encode classfile with one field removed")
}

fn stale_tasty_uuid(class_bytes: &[u8], tasty_bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes)
        .expect("original Rust TASTy sidecar validates");
    let uuid = tasty.header().uuid;
    let mut class = class_bytes.to_vec();
    let link = class
        .windows(uuid.len())
        .position(|window| window == uuid)
        .expect("classfile links the original sidecar UUID");
    class[link] ^= 0x80;
    class
}

fn bad_alias_termref_target(tasty_bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes)
        .expect("original Rust Pair TASTy validates before mutation");
    let address_index = tasty
        .ast_address_index()
        .expect("index Pair TASTy AST addresses");
    let invalid_target = (0..128u32)
        .find(|address| address_index.get_node(*address).is_none())
        .expect("TASTy has an unused one-byte AST address");
    let asts = tasty
        .section(StandardSection::Asts)
        .expect("Pair TASTy has ASTs section");
    let original_target = 12u8;
    let needle = [TERMREFSYMBOL_TAG, 0x80 | original_target];
    let local_offset = asts
        .payload
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("first alias TERMREFsymbol has the expected PARAM address");
    let mut malformed = tasty_bytes.to_vec();
    malformed[asts.offset + local_offset + 1] = 0x80 | invalid_target as u8;
    malformed
}

fn method_lookup_probes(temporary: &Path, scala: &Scala, batch: &PairedJvmTastyBatchArtifacts) {
    let cases = [
        (
            "counter-get",
            "example/Counter.class",
            "get",
            "()I",
            "new Counter(1).get()",
        ),
        (
            "counter-set",
            "example/Counter.class",
            "set",
            "(I)V",
            "new Counter(1).set(2)",
        ),
        (
            "pair-left",
            "example/Pair.class",
            "leftValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).leftValue()",
        ),
        (
            "pair-right",
            "example/Pair.class",
            "rightValue",
            "()I",
            "new Pair(new Counter(1), new Counter(2)).rightValue()",
        ),
        (
            "pair-constructor",
            "example/Pair.class",
            "<init>",
            "(Lexample/Counter;Lexample/Counter;)V",
            "new Pair(new Counter(1), new Counter(2))",
        ),
    ];
    for (name, class_path, member, descriptor, expression) in cases {
        let source_root = temporary.join(format!("lookup-{name}"));
        let classes = source_root.join("classes");
        let output = source_root.join("out");
        fs::create_dir_all(&classes).unwrap();
        fs::create_dir_all(&output).unwrap();
        write_batch(&classes, batch);
        let original = fs::read(classes.join(class_path)).unwrap();
        fs::write(
            classes.join(class_path),
            without_method(&original, member, descriptor),
        )
        .unwrap();
        let source = source_root.join("Probe.scala");
        fs::write(
            &source,
            format!("package example\nobject Probe {{ val selected = {expression} }}\n"),
        )
        .unwrap();

        let tasty_classpath = scala.classpath(&classes);
        let tasty_result = scala.compile_result(&tasty_classpath, &output, &source);
        assert!(
            tasty_result.status.success(),
            "TASTy lookup for {member}{descriptor} should succeed after hiding its classfile declaration:\n{}",
            String::from_utf8_lossy(&tasty_result.stderr)
        );
        eprintln!("TASTY_LOOKUP_ACCEPTED {member}{descriptor}");

        let classfile_only = source_root.join("classfile-only");
        let classfile_only_out = source_root.join("classfile-only-out");
        fs::create_dir_all(&classfile_only).unwrap();
        fs::create_dir_all(&classfile_only_out).unwrap();
        write_batch(&classfile_only, batch);
        fs::remove_file(classfile_only.join(class_path.replace(".class", ".tasty"))).unwrap();
        let classfile_only_result = scala.compile_result(
            &scala.classpath(&classfile_only),
            &classfile_only_out,
            &source,
        );
        assert!(
            !classfile_only_result.status.success(),
            "classfile-only control unexpectedly resolved {member}{descriptor}"
        );
        eprintln!(
            "CLASSFILE_ONLY_FIRST_ERROR {member}{descriptor}: {}",
            first_error(&classfile_only_result)
        );
        fs::write(
            source_root.join(format!("{name}.first-error.txt")),
            first_error(&classfile_only_result),
        )
        .unwrap();
    }
}

fn compile_and_run_variant(
    scala: &Scala,
    source: &str,
    source_name: &str,
    client: &str,
    main_class: &str,
    label: &str,
    source_id: u32,
) {
    let first = emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(source_id))
        .expect("variant has its own fresh Rust producer session");
    let repeated =
        emit_paired_jvm_tasty_batch(source, source_name, SourceId::from_index(source_id + 1))
            .expect("same variant source emits in another fresh Rust session");
    assert_eq!(first.classes().len(), 2);
    assert_eq!(repeated.classes().len(), 2);
    for (left, right) in first.classes().iter().zip(repeated.classes()) {
        assert_eq!(left.classfile_path(), right.classfile_path());
        assert_eq!(left.tasty_path(), right.tasty_path());
        assert_eq!(left.classfile_bytes(), right.classfile_bytes());
        assert_eq!(left.tasty_bytes(), right.tasty_bytes());
        assert_eq!(left.tasty_link_id(), right.tasty_link_id());
    }

    let temporary = task_dir(label);
    let classes = temporary.join("rust-producer");
    let client_output = temporary.join("scala-client");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&client_output).unwrap();
    write_batch(&classes, &first);
    let producer_source = temporary.join(source_name);
    fs::write(&producer_source, source).unwrap();
    let client_source = temporary.join("VariantConsumer.scala");
    fs::write(&client_source, client).unwrap();
    eprintln!(
        "VARIANT_PRODUCER_SOURCE_SHA256 {label} {}",
        sha256(&producer_source)
    );
    eprintln!(
        "VARIANT_CLIENT_SOURCE_SHA256 {label} {}",
        sha256(&client_source)
    );
    for class in first.classes() {
        eprintln!(
            "VARIANT_ARTIFACT_SHA256 {label} {} {}",
            class.classfile_path(),
            sha256(&classes.join(class.classfile_path()))
        );
        eprintln!(
            "VARIANT_ARTIFACT_SHA256 {label} {} {}",
            class.tasty_path(),
            sha256(&classes.join(class.tasty_path()))
        );
        let tasty = TastyFile::parse_and_validate_scala_3_9(class.tasty_bytes()).unwrap();
        eprintln!(
            "VARIANT_ARTIFACT_UUID {label} {} {}",
            class.tasty_path(),
            uuid_text(&tasty.header().uuid)
        );
    }
    scala.compile(&scala.classpath(&classes), &client_output, &client_source);
    let classpath = format!(
        "{}:{}:{}",
        client_output.display(),
        classes.display(),
        scala.scala_libraries
    );
    let output = run(Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(classpath)
        .arg(main_class));
    assert!(String::from_utf8_lossy(&output.stdout).contains("variant=ok"));
}

fn first_error(output: &Output) -> String {
    let text = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    text.lines()
        .find(|line| line.contains("[E") || line.contains("Error:"))
        .unwrap_or_else(|| text.lines().next().unwrap_or("no diagnostic"))
        .to_owned()
}

fn reject_with_first_error(output: &Output, label: &str) -> String {
    assert!(
        !output.status.success(),
        "negative probe {label} unexpectedly succeeded"
    );
    let first = first_error(output);
    assert_ne!(
        first, "no diagnostic",
        "negative probe {label} emitted no diagnostics"
    );
    eprintln!("NEGATIVE_FIRST_ERROR {label}: {first}");
    first
}

fn sha256_output(bytes: &[u8]) -> String {
    let path = task_dir("digest").join("output");
    fs::write(&path, bytes).unwrap();
    sha256(&path)
}

fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

#[test]
fn pinned_scala39_compiles_and_runs_a_separate_client_on_original_rust_pair_artifacts() {
    if !required() {
        return;
    }
    let scala = Scala::from_environment();
    let temporary = task_dir("runtime");
    let producer_dir = temporary.join("rust-producer");
    let consumer_dir = temporary.join("scala-client");
    fs::create_dir_all(&producer_dir).unwrap();
    fs::create_dir_all(&consumer_dir).unwrap();
    let batch = producer(18811);
    assert_eq!(batch.classes().len(), 2);
    assert_eq!(
        batch
            .classes()
            .iter()
            .flat_map(|class| [class.classfile_path(), class.tasty_path()])
            .collect::<Vec<_>>(),
        [
            "example/Counter.class",
            "example/Counter.tasty",
            "example/Pair.class",
            "example/Pair.tasty"
        ]
    );
    write_batch(&producer_dir, &batch);
    let before = artifact_hashes(&producer_dir);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_dual_reference_constructors/RustArtifactConsumer.scala");
    assert_eq!(fs::read(&fixture).unwrap(), CLIENT);
    let consumer_sha = sha256(&fixture);
    let classpath = scala.classpath(&producer_dir);
    let reconstructed = temporary.join("from-tasty-inspection-only");
    let compiled = scala.compile(&classpath, &consumer_dir, &fixture);
    assert!(compiled.status.success());
    assert_eq!(
        artifact_hashes(&producer_dir),
        before,
        "Scala client compilation leaves original Rust artifacts byte-identical"
    );

    let runtime_classpath = format!(
        "{}:{}:{}",
        consumer_dir.display(),
        producer_dir.display(),
        scala.scala_libraries
    );
    assert!(!runtime_classpath.contains(reconstructed.to_str().unwrap()));
    eprintln!("SCALA_CLIENT_CLASSPATH={classpath}");
    eprintln!("JAVA_RUNTIME_CLASSPATH={runtime_classpath}");
    let run_result = run(Command::new(&scala.java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(runtime_classpath)
        .arg("example.RustArtifactConsumer"));
    assert_eq!(
        String::from_utf8_lossy(&run_result.stdout).trim(),
        "scala39-rust-dual-reference-consumer=ok"
    );
    assert_eq!(
        artifact_hashes(&producer_dir),
        before,
        "execution leaves original Rust artifacts byte-identical"
    );

    let pair_tasty = producer_dir.join("example/Pair.tasty");
    let decompile = scala
        .scalac()
        .arg("-decompile")
        .arg(&pair_tasty)
        .output()
        .expect("run official Scala -decompile on original Rust Pair.tasty");
    assert!(
        decompile.status.success(),
        "{}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    for declaration in [
        "first",
        "second",
        "left",
        "right",
        "leftValue",
        "rightValue",
    ] {
        assert!(
            decompiled.contains(declaration),
            "Scala -decompile omitted {declaration}:\n{decompiled}"
        );
    }
    let printed = scala
        .scalac()
        .arg("-print-tasty")
        .arg(&pair_tasty)
        .output()
        .expect("run official Scala -print-tasty on original Rust Pair.tasty");
    assert!(
        printed.status.success(),
        "{}",
        String::from_utf8_lossy(&printed.stderr)
    );
    let tasty_dump = strip_ansi(&String::from_utf8_lossy(&printed.stdout));
    for identity_edge in [
        "version: 28.9.0",
        "12:       PARAM(9) 8 [first]",
        "23:       PARAM(9) 10 [second]",
        "49:         PARAM(5) 8 [first]",
        "56:         PARAM(5) 10 [second]",
        "68:       VALDEF(16) 13 [left]",
        "77:         TERMREFsymbol 12",
        "86:       VALDEF(16) 14 [right]",
        "95:         TERMREFsymbol 23",
        "104:       DEFDEF(24) 15 [leftValue]",
        "119:             SELECT 13 [left]",
        "130:       DEFDEF(22) 21 [rightValue]",
        "143:             SELECT 14 [right]",
    ] {
        assert!(
            tasty_dump.contains(identity_edge),
            "Scala -print-tasty omitted identity edge {identity_edge:?}:\n{tasty_dump}"
        );
    }
    fs::create_dir_all(&reconstructed).unwrap();
    let from_tasty = scala
        .scalac()
        .args(["-from-tasty", "-classpath"])
        .arg(&producer_dir)
        .arg("-d")
        .arg(&reconstructed)
        .arg(producer_dir.join("example/Counter.tasty"))
        .arg(&pair_tasty)
        .output()
        .expect("run official Scala -from-tasty on original Rust sidecars");
    assert!(
        from_tasty.status.success(),
        "{}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = run(Command::new(scala.java_home.join("bin/javap"))
        .args(["-p", "-s", "-classpath"])
        .arg(&reconstructed)
        .arg("example.Pair"));
    let rebuilt_text = String::from_utf8_lossy(&javap.stdout);
    assert!(
        rebuilt_text.contains("(Lexample/Counter;Lexample/Counter;)V"),
        "{rebuilt_text}"
    );
    for field in [
        "private final example.Counter left",
        "private final example.Counter right",
    ] {
        assert!(
            rebuilt_text.contains(field),
            "Scala -from-tasty omitted {field}:\n{rebuilt_text}"
        );
    }
    for method in ["leftValue()", "rightValue()"] {
        assert!(
            rebuilt_text.contains(method),
            "Scala -from-tasty omitted {method}:\n{rebuilt_text}"
        );
    }

    eprintln!("SCALA39_SOURCE_SHA={SCALA_REVISION}");
    eprintln!("SCALA_CLIENT_SHA256={consumer_sha}");
    eprintln!(
        "SCALA_DECOMPILE_SHA256={}",
        sha256_output(&decompile.stdout)
    );
    eprintln!(
        "SCALA_PRINT_TASTY_SHA256={}",
        sha256_output(&printed.stdout)
    );
    eprintln!(
        "SCALA_FROM_TASTY_JAVAP_SHA256={}",
        sha256_output(&javap.stdout)
    );
    for (path, hash) in &before {
        eprintln!("RUST_ARTIFACT_SHA256 {path} {hash}");
    }
    let pair_source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");
    eprintln!("RUST_SOURCE_SHA256={}", sha256(&pair_source));
    assert_eq!(sha256(&pair_source), PAIR_SOURCE_SHA256);
    for class in batch.classes() {
        let sidecar = TastyFile::parse_and_validate_scala_3_9(class.tasty_bytes()).unwrap();
        assert_eq!(class.tasty_link_id().as_bytes(), &sidecar.header().uuid);
        let uuid = uuid_text(&sidecar.header().uuid);
        eprintln!("RUST_ARTIFACT_UUID {} {uuid}", class.classfile_path());
        eprintln!("RUST_ARTIFACT_UUID {} {uuid}", class.tasty_path());
    }
}

#[test]
fn pinned_scala39_uses_rust_tasty_for_each_method_and_primary_constructor_signature() {
    if !required() {
        return;
    }
    let scala = Scala::from_environment();
    let temporary = task_dir("lookup");
    let producer_dir = temporary.join("original");
    fs::create_dir_all(&producer_dir).unwrap();
    let batch = producer(18812);
    write_batch(&producer_dir, &batch);
    method_lookup_probes(&temporary, &scala, &batch);
}

#[test]
fn pinned_scala39_accepts_renamed_and_reordered_rust_batches() {
    if !required() {
        return;
    }
    let scala = Scala::from_environment();
    let reordered = include_str!(
        "fixtures/jvm_tasty_dual_reference_constructors/source-variants/ReorderedClasses.scala"
    );
    compile_and_run_variant(
        &scala,
        reordered,
        "ReorderedClasses.scala",
        r#"
        package example
        object ReorderedConsumer:
          def main(args: Array[String]): Unit =
            val first = new Counter(7)
            val second = new Counter(3)
            val pair = new Pair(first, second)
            assert(pair.leftValue() == 7 && pair.rightValue() == 3)
            first.set(42)
            assert(pair.leftValue() == 42 && pair.rightValue() == 3)
            println("reordered-variant=ok")
        "#,
        "example.ReorderedConsumer",
        "reordered",
        18813,
    );

    const RENAMED: &str = "package renamed\nclass Duo(firstToken: Token, secondToken: Token):\n  private val leftToken: Token = firstToken\n  private val rightToken: Token = secondToken\n  def leftValue(): Int = leftToken.get()\n  def rightValue(): Int = rightToken.get()\nclass Token(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n";
    compile_and_run_variant(
        &scala,
        RENAMED,
        "Renamed.scala",
        r#"
        package renamed
        object RenamedConsumer:
          def main(args: Array[String]): Unit =
            val first = new Token(7)
            val second = new Token(3)
            val pair = new Duo(first, second)
            assert(pair.leftValue() == 7 && pair.rightValue() == 3)
            second.set(-9)
            assert(pair.leftValue() == 7 && pair.rightValue() == -9)
            println("renamed-variant=ok")
        "#,
        "renamed.RenamedConsumer",
        "renamed",
        18814,
    );
}

#[test]
fn pinned_scala39_rejects_corrupt_or_inconsistent_dual_reference_inputs() {
    if !required() {
        return;
    }
    let scala = Scala::from_environment();
    let temporary = task_dir("negative-matrix");
    let batch = producer(18815);
    let original = temporary.join("original");
    fs::create_dir_all(&original).unwrap();
    write_batch(&original, &batch);

    let simple_client = temporary.join("Lookup.scala");
    fs::write(
        &simple_client,
        "package example\nobject Lookup { val selected = new Pair(new Counter(1), new Counter(2)).leftValue() }\n",
    )
    .unwrap();
    let mut hidden_left = batch.classes()[1].classfile_bytes().to_vec();
    hidden_left = without_method(&hidden_left, "leftValue", "()I");

    let missing = temporary.join("missing-sidecar");
    fs::create_dir_all(&missing).unwrap();
    write_batch(&missing, &batch);
    fs::write(missing.join("example/Pair.class"), &hidden_left).unwrap();
    fs::remove_file(missing.join("example/Pair.tasty")).unwrap();
    let missing_out = temporary.join("missing-sidecar-out");
    fs::create_dir_all(&missing_out).unwrap();
    let missing_input =
        scala.compile_result(&scala.classpath(&missing), &missing_out, &simple_client);
    reject_with_first_error(
        &missing_input,
        "missing Pair.tasty with Pair.leftValue hidden",
    );

    let misplaced = temporary.join("misplaced-sidecar");
    fs::create_dir_all(&misplaced).unwrap();
    write_batch(&misplaced, &batch);
    fs::write(misplaced.join("example/Pair.class"), &hidden_left).unwrap();
    fs::rename(
        misplaced.join("example/Pair.tasty"),
        misplaced.join("example/MovedPair.tasty"),
    )
    .unwrap();
    let misplaced_out = temporary.join("misplaced-sidecar-out");
    fs::create_dir_all(&misplaced_out).unwrap();
    let misplaced_input =
        scala.compile_result(&scala.classpath(&misplaced), &misplaced_out, &simple_client);
    reject_with_first_error(
        &misplaced_input,
        "misplaced Pair.tasty with Pair.leftValue hidden",
    );

    let counter_client = temporary.join("CounterLookup.scala");
    fs::write(
        &counter_client,
        "package example\nobject CounterLookup { val selected = new Counter(1).get() }\n",
    )
    .unwrap();
    let hidden_get = without_method(batch.classes()[0].classfile_bytes(), "get", "()I");
    let missing_counter_sidecar = temporary.join("missing-counter-sidecar");
    fs::create_dir_all(&missing_counter_sidecar).unwrap();
    write_batch(&missing_counter_sidecar, &batch);
    fs::write(
        missing_counter_sidecar.join("example/Counter.class"),
        &hidden_get,
    )
    .unwrap();
    fs::remove_file(missing_counter_sidecar.join("example/Counter.tasty")).unwrap();
    let missing_counter_out = temporary.join("missing-counter-sidecar-out");
    fs::create_dir_all(&missing_counter_out).unwrap();
    let missing_counter_result = scala.compile_result(
        &scala.classpath(&missing_counter_sidecar),
        &missing_counter_out,
        &counter_client,
    );
    reject_with_first_error(
        &missing_counter_result,
        "missing Counter.tasty with Counter.get hidden",
    );

    let misplaced_counter_sidecar = temporary.join("misplaced-counter-sidecar");
    fs::create_dir_all(&misplaced_counter_sidecar).unwrap();
    write_batch(&misplaced_counter_sidecar, &batch);
    fs::write(
        misplaced_counter_sidecar.join("example/Counter.class"),
        &hidden_get,
    )
    .unwrap();
    fs::rename(
        misplaced_counter_sidecar.join("example/Counter.tasty"),
        misplaced_counter_sidecar.join("example/MovedCounter.tasty"),
    )
    .unwrap();
    let misplaced_counter_out = temporary.join("misplaced-counter-sidecar-out");
    fs::create_dir_all(&misplaced_counter_out).unwrap();
    let misplaced_counter_result = scala.compile_result(
        &scala.classpath(&misplaced_counter_sidecar),
        &misplaced_counter_out,
        &counter_client,
    );
    reject_with_first_error(
        &misplaced_counter_result,
        "misplaced Counter.tasty with Counter.get hidden",
    );

    let corrupt = temporary.join("corrupt-sidecar");
    fs::create_dir_all(&corrupt).unwrap();
    write_batch(&corrupt, &batch);
    fs::write(corrupt.join("example/Pair.tasty"), [0u8, 1, 2, 3]).unwrap();
    let corrupt_input = scala
        .scalac()
        .arg("-decompile")
        .arg(corrupt.join("example/Pair.tasty"))
        .output()
        .unwrap();
    reject_with_first_error(&corrupt_input, "corrupt Pair.tasty -decompile");

    let corrupt_counter = temporary.join("corrupt-counter-sidecar");
    fs::create_dir_all(&corrupt_counter).unwrap();
    write_batch(&corrupt_counter, &batch);
    fs::write(
        corrupt_counter.join("example/Counter.tasty"),
        [0u8, 1, 2, 3],
    )
    .unwrap();
    let counter_tasty_error = scala
        .scalac()
        .arg("-decompile")
        .arg(corrupt_counter.join("example/Counter.tasty"))
        .output()
        .unwrap();
    reject_with_first_error(&counter_tasty_error, "corrupt Counter.tasty -decompile");

    let bad_termref = temporary.join("bad-alias-termref-address");
    fs::create_dir_all(&bad_termref).unwrap();
    write_batch(&bad_termref, &batch);
    fs::write(
        bad_termref.join("example/Pair.tasty"),
        bad_alias_termref_target(batch.classes()[1].tasty_bytes()),
    )
    .unwrap();
    let bad_termref_error = scala
        .scalac()
        .arg("-decompile")
        .arg(bad_termref.join("example/Pair.tasty"))
        .output()
        .unwrap();
    reject_with_first_error(
        &bad_termref_error,
        "malformed Pair alias TERMREFsymbol address",
    );

    let stale_uuid = temporary.join("stale-uuid");
    fs::create_dir_all(&stale_uuid).unwrap();
    write_batch(&stale_uuid, &batch);
    let pair_class = batch.classes()[1].classfile_bytes();
    let pair_tasty = batch.classes()[1].tasty_bytes();
    fs::write(
        stale_uuid.join("example/Pair.class"),
        stale_tasty_uuid(pair_class, pair_tasty),
    )
    .unwrap();
    let stale_out = temporary.join("stale-uuid-out");
    fs::create_dir_all(&stale_out).unwrap();
    let stale_result =
        scala.compile_result(&scala.classpath(&stale_uuid), &stale_out, &simple_client);
    reject_with_first_error(&stale_result, "Pair classfile has stale TASTY UUID");

    let missing_private_field = temporary.join("missing-private-field");
    fs::create_dir_all(&missing_private_field).unwrap();
    write_batch(&missing_private_field, &batch);
    fs::write(
        missing_private_field.join("example/Pair.class"),
        without_field(pair_class, "left"),
    )
    .unwrap();
    let missing_field_out = temporary.join("missing-private-field-out");
    fs::create_dir_all(&missing_field_out).unwrap();
    let missing_field_result = scala.compile_result(
        &scala.classpath(&missing_private_field),
        &missing_field_out,
        &simple_client,
    );
    assert!(
        missing_field_result.status.success(),
        "Scala source lookup reads the private VALDEF from authentic TASTy:\n{}",
        String::from_utf8_lossy(&missing_field_result.stderr)
    );
    eprintln!(
        "CONTROL_PRIVATE_FIELD_ABSENT_WITH_TASTY: compilation succeeds; private fields are not client-visible lookup probes"
    );

    let bad_descriptor = temporary.join("bad-constructor-descriptor");
    fs::create_dir_all(&bad_descriptor).unwrap();
    write_batch(&bad_descriptor, &batch);
    fs::write(
        bad_descriptor.join("example/Pair.class"),
        wrong_pair_constructor_descriptor(pair_class),
    )
    .unwrap();
    let descriptor_with_tasty_out = temporary.join("bad-descriptor-with-tasty-out");
    fs::create_dir_all(&descriptor_with_tasty_out).unwrap();
    let descriptor_with_tasty = scala.compile_result(
        &scala.classpath(&bad_descriptor),
        &descriptor_with_tasty_out,
        &simple_client,
    );
    assert!(
        descriptor_with_tasty.status.success(),
        "TASTy lookup uses the canonical two-argument constructor signature:\n{}",
        String::from_utf8_lossy(&descriptor_with_tasty.stderr)
    );
    eprintln!(
        "CONTROL_WRONG_CLASSFILE_CONSTRUCTOR_DESCRIPTOR_WITH_TASTY: compiled from original TASTy source signature"
    );
    // Exercise the malformed descriptor with the matched classfile-only
    // fallback control so its actual first failure is recorded.
    fs::remove_file(bad_descriptor.join("example/Pair.tasty")).unwrap();
    let descriptor_out = temporary.join("bad-descriptor-out");
    fs::create_dir_all(&descriptor_out).unwrap();
    let descriptor_result = scala.compile_result(
        &scala.classpath(&bad_descriptor),
        &descriptor_out,
        &simple_client,
    );
    reject_with_first_error(
        &descriptor_result,
        "wrong-arity Pair constructor descriptor",
    );

    let bad_method_signature = temporary.join("bad-method-signature");
    fs::create_dir_all(&bad_method_signature).unwrap();
    write_batch(&bad_method_signature, &batch);
    fs::write(
        bad_method_signature.join("example/Pair.class"),
        wrong_method_descriptor(pair_class, "leftValue", "()I"),
    )
    .unwrap();
    let altered_method_with_tasty_out = temporary.join("altered-method-with-tasty-out");
    fs::create_dir_all(&altered_method_with_tasty_out).unwrap();
    let altered_method_with_tasty = scala.compile_result(
        &scala.classpath(&bad_method_signature),
        &altered_method_with_tasty_out,
        &simple_client,
    );
    assert!(
        altered_method_with_tasty.status.success(),
        "TASTy method signature lookup succeeds independently of altered classfile method descriptor:\n{}",
        String::from_utf8_lossy(&altered_method_with_tasty.stderr)
    );
    eprintln!(
        "CONTROL_ALTERED_CLASSFILE_METHOD_SIGNATURE_WITH_TASTY: compiled from TASTy method signature"
    );
    fs::remove_file(bad_method_signature.join("example/Pair.tasty")).unwrap();
    let bad_method_out = temporary.join("bad-method-signature-out");
    fs::create_dir_all(&bad_method_out).unwrap();
    let bad_method_result = scala.compile_result(
        &scala.classpath(&bad_method_signature),
        &bad_method_out,
        &simple_client,
    );
    reject_with_first_error(
        &bad_method_result,
        "altered leftValue classfile signature without TASTy",
    );

    let no_counter = temporary.join("missing-classfile");
    fs::create_dir_all(&no_counter).unwrap();
    write_batch(&no_counter, &batch);
    fs::remove_file(no_counter.join("example/Counter.class")).unwrap();
    let no_counter_out = temporary.join("missing-classfile-out");
    fs::create_dir_all(&no_counter_out).unwrap();
    let tasty_only_result = scala.compile_result(
        &scala.classpath(&no_counter),
        &no_counter_out,
        &simple_client,
    );
    assert!(
        tasty_only_result.status.success(),
        "Scala 3.9 can compile from the remaining TASTy sidecar:\n{}",
        String::from_utf8_lossy(&tasty_only_result.stderr)
    );
    eprintln!(
        "NEGATIVE_CONTROL_CLASSFILE_ABSENT_WITH_TASTY: accepted from authentic TASTy only (compilation only)"
    );

    fs::remove_file(no_counter.join("example/Counter.tasty")).unwrap();
    let absent_type_out = temporary.join("missing-counter-artifacts-out");
    fs::create_dir_all(&absent_type_out).unwrap();
    let absent_type = scala.compile_result(
        &scala.classpath(&no_counter),
        &absent_type_out,
        &simple_client,
    );
    reject_with_first_error(&absent_type, "missing Counter.class and Counter.tasty");

    let wrong_type_source = temporary.join("WrongSecondType.scala");
    fs::write(
        &wrong_type_source,
        "package example\nobject WrongSecondType { val value = new Pair(new Counter(1), 2) }\n",
    )
    .unwrap();
    let wrong_type_out = temporary.join("wrong-second-type-out");
    fs::create_dir_all(&wrong_type_out).unwrap();
    let wrong_type_result = scala.compile_result(
        &scala.classpath(&original),
        &wrong_type_out,
        &wrong_type_source,
    );
    reject_with_first_error(&wrong_type_result, "wrong second constructor argument type");

    let foreign_root = temporary.join("foreign-owner");
    let foreign_classes = foreign_root.join("classes");
    fs::create_dir_all(&foreign_classes).unwrap();
    let foreign_source = foreign_root.join("ForeignCounter.scala");
    fs::write(
        &foreign_source,
        "package foreign\nclass Counter(val start: Int)\n",
    )
    .unwrap();
    scala.compile(&scala.scala_libraries, &foreign_classes, &foreign_source);
    let wrong_owner_source = foreign_root.join("WrongOwner.scala");
    fs::write(
        &wrong_owner_source,
        "package client\nobject WrongOwner { val value = new example.Pair(new example.Counter(1), new foreign.Counter(2)) }\n",
    )
    .unwrap();
    let wrong_owner_out = foreign_root.join("out");
    fs::create_dir_all(&wrong_owner_out).unwrap();
    let wrong_owner_classpath = format!(
        "{}:{}",
        scala.classpath(&original),
        foreign_classes.display()
    );
    let wrong_owner_result = scala.compile_result(
        &wrong_owner_classpath,
        &wrong_owner_out,
        &wrong_owner_source,
    );
    reject_with_first_error(&wrong_owner_result, "same-spelled foreign Counter owner");
}
