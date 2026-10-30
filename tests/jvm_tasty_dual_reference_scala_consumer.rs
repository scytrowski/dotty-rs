use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURES: &str = "tests/fixtures/jvm_tasty_dual_reference_constructors";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const FULL_SOURCE: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");
const FULL_CONSUMER: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/Consumer.scala");
const SIGNATURE_SOURCE: &[u8] =
    include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/PairSignatureOnly.scala");

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

fn collect_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(current).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            files.push(path.strip_prefix(root).unwrap().to_owned());
        }
    }
    files.sort();
}

#[test]
fn pinned_scala39_emits_the_oracle_and_separate_consumer_checks_arity_and_types() {
    if std::env::var_os("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER").is_none() {
        eprintln!(
            "skipping pinned Scala consumer; set DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER=1 to require it"
        );
        return;
    }
    let scala_root =
        PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let java = java_home.join("bin/java");
    let scala_libs = [
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"),
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"),
    ];
    for path in &scala_libs {
        assert!(
            path.is_file(),
            "pinned Scala runtime jar exists: {}",
            path.display()
        );
    }

    let revision = run(Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"]));
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        SCALA_REVISION
    );
    let version = run(Command::new(&scalac).arg("-version"));
    let version_text = String::from_utf8_lossy(&version.stderr).to_string()
        + &String::from_utf8_lossy(&version.stdout);
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );
    let jdk_version = run(Command::new(&java).arg("-version"));
    let jdk_text = String::from_utf8_lossy(&jdk_version.stderr).to_string()
        + &String::from_utf8_lossy(&jdk_version.stdout);
    assert!(
        jdk_text.contains("21.0.13"),
        "the recorded oracle toolchain is Temurin 21.0.13: {jdk_text}"
    );

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let task_temp = std::env::temp_dir().join(format!(
        "dotty-dual-reference-{}-{nonce}",
        std::process::id()
    ));
    let producer = task_temp.join("producer");
    let consumer = task_temp.join("consumer");
    let signature = task_temp.join("signature-only");
    fs::create_dir_all(&producer).unwrap();
    fs::create_dir_all(&consumer).unwrap();
    fs::create_dir_all(&signature).unwrap();
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
    let source = fixture_dir.join("Pair.scala");
    let consumer_source = fixture_dir.join("Consumer.scala");
    let signature_source = fixture_dir.join("PairSignatureOnly.scala");
    assert_eq!(fs::read(&source).unwrap(), FULL_SOURCE);
    assert_eq!(fs::read(&consumer_source).unwrap(), FULL_CONSUMER);
    assert_eq!(fs::read(&signature_source).unwrap(), SIGNATURE_SOURCE);

    run(Command::new(&scalac)
        .args(["-release", "21", "-d"])
        .arg(&producer)
        .arg(&source));
    let scala_classpath = format!("{}:{}", scala_libs[0].display(), scala_libs[1].display());
    run(Command::new(&scalac)
        .args(["-release", "21", "-classpath"])
        .arg(format!("{}:{scala_classpath}", producer.display()))
        .arg("-d")
        .arg(&consumer)
        .arg(&consumer_source));
    let output = run(Command::new(&java)
        .args(["-Xverify:all", "-ea", "-classpath"])
        .arg(format!(
            "{}:{}:{scala_classpath}",
            consumer.display(),
            producer.display()
        ))
        .arg("example.DualReferenceConsumer"));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "scala39-dual-reference-constructor=ok"
    );

    let mut files = Vec::new();
    collect_files(&producer, &producer, &mut files);
    assert_eq!(
        files,
        [
            PathBuf::from("example/Counter.class"),
            PathBuf::from("example/Counter.tasty"),
            PathBuf::from("example/Pair.class"),
            PathBuf::from("example/Pair.tasty"),
        ]
    );
    for (relative, expected) in [
        (
            "example/Counter.class",
            include_bytes!(
                "fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Counter.class"
            )
            .as_slice(),
        ),
        (
            "example/Counter.tasty",
            include_bytes!(
                "fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Counter.tasty"
            )
            .as_slice(),
        ),
        (
            "example/Pair.class",
            include_bytes!(
                "fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Pair.class"
            )
            .as_slice(),
        ),
        (
            "example/Pair.tasty",
            include_bytes!(
                "fixtures/jvm_tasty_dual_reference_constructors/scala39/example/Pair.tasty"
            )
            .as_slice(),
        ),
    ] {
        assert_eq!(
            fs::read(producer.join(relative)).unwrap(),
            expected,
            "deterministic Scala oracle artifact {relative}"
        );
    }

    run(Command::new(&scalac)
        .args(["-release", "21", "-d"])
        .arg(&signature)
        .arg(&signature_source));
    let mut signature_files = Vec::new();
    collect_files(&signature, &signature, &mut signature_files);
    assert_eq!(
        signature_files,
        [
            PathBuf::from("example/Counter.class"),
            PathBuf::from("example/Counter.tasty"),
            PathBuf::from("example/Pair.class"),
            PathBuf::from("example/Pair.tasty"),
        ]
    );
    for (relative, expected) in [
        (
            "example/Counter.class",
            include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Counter.class").as_slice(),
        ),
        (
            "example/Counter.tasty",
            include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Counter.tasty").as_slice(),
        ),
        (
            "example/Pair.class",
            include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Pair.class").as_slice(),
        ),
        (
            "example/Pair.tasty",
            include_bytes!("fixtures/jvm_tasty_dual_reference_constructors/scala39/signature-only/example/Pair.tasty").as_slice(),
        ),
    ] {
        assert_eq!(fs::read(signature.join(relative)).unwrap(), expected, "signature-only oracle artifact {relative}");
    }

    let _ = fs::remove_dir_all(task_temp);
}
