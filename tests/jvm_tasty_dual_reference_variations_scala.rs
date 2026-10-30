use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURES: &str = "tests/fixtures/jvm_tasty_dual_reference_constructors";
const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";

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

fn compile_failure(command: &mut Command) -> String {
    let rendered = format!("{command:?}");
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"));
    assert!(
        !output.status.success(),
        "expected compiler error for {rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr)
}

#[test]
fn pinned_scala39_classifies_dual_reference_source_and_consumer_variations() {
    if std::env::var_os("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER").is_none() {
        eprintln!(
            "skipping Scala source variation matrix; set DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_CONSUMER=1 to require it"
        );
        return;
    }
    let scala_root =
        PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let revision = run(Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"]));
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        SCALA_REVISION
    );
    let version = run(Command::new(&scalac).arg("-version"));
    let version = String::from_utf8_lossy(&version.stderr).to_string()
        + &String::from_utf8_lossy(&version.stdout);
    assert!(
        version.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version}"
    );

    let task_temp = std::env::temp_dir().join(format!(
        "dotty-dual-reference-variations-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
    let producer = task_temp.join("producer");
    fs::create_dir_all(&producer).unwrap();
    let scala_libs = [
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar"),
        scala_root.join("dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"),
    ];
    let scala_classpath = format!("{}:{}", scala_libs[0].display(), scala_libs[1].display());
    run(Command::new(&scalac)
        .args(["-release", "21", "-d"])
        .arg(&producer)
        .arg(fixture_dir.join("Pair.scala")));

    let valid = [
        "ReorderedClasses.scala",
        "ReorderedParameters.scala",
        "GenericConstructor.scala",
        "DefaultConstructor.scala",
        "ContextualConstructor.scala",
        "ConstructorValAccessors.scala",
        "ConstructorVarAccessors.scala",
        "ThreeReferenceFormals.scala",
        "ThirdDistinctReferenceField.scala",
        "WrongFieldAlias.scala",
        "NoFields.scala",
        "SecondaryConstructor.scala",
    ];
    for name in valid {
        let output = task_temp.join(name.strip_suffix(".scala").unwrap());
        fs::create_dir_all(&output).unwrap();
        run(Command::new(&scalac)
            .args(["-release", "21", "-d"])
            .arg(output)
            .arg(fixture_dir.join("source-variants").join(name)));
    }
    for name in ["WrongSecondType.scala", "DuplicateParameterName.scala"] {
        let output = task_temp.join(name.strip_suffix(".scala").unwrap());
        fs::create_dir_all(&output).unwrap();
        let diagnostics = compile_failure(
            Command::new(&scalac)
                .args(["-release", "21", "-d"])
                .arg(output)
                .arg(fixture_dir.join("source-variants").join(name)),
        );
        assert!(
            diagnostics.contains("error") || diagnostics.contains("Error"),
            "{name}: {diagnostics}"
        );
    }

    for name in ["WrongArity.scala", "WrongSecondType.scala"] {
        let output = task_temp.join(format!("client-{}", name.strip_suffix(".scala").unwrap()));
        fs::create_dir_all(&output).unwrap();
        let diagnostics = compile_failure(
            Command::new(&scalac)
                .args(["-release", "21", "-classpath"])
                .arg(format!("{}:{scala_classpath}", producer.display()))
                .arg("-d")
                .arg(output)
                .arg(fixture_dir.join("consumer-negative").join(name)),
        );
        assert!(
            diagnostics.contains("constructor") || diagnostics.contains("argument"),
            "{name}: {diagnostics}"
        );
    }
    let foreign_output = task_temp.join("foreign");
    fs::create_dir_all(&foreign_output).unwrap();
    run(Command::new(&scalac)
        .args(["-release", "21", "-d"])
        .arg(&foreign_output)
        .arg(fixture_dir.join("consumer-negative/ForeignCounter.scala")));
    let wrong_owner_output = task_temp.join("wrong-owner");
    fs::create_dir_all(&wrong_owner_output).unwrap();
    let diagnostics = compile_failure(
        Command::new(&scalac)
            .args(["-release", "21", "-classpath"])
            .arg(format!(
                "{}:{}:{scala_classpath}",
                producer.display(),
                foreign_output.display()
            ))
            .arg("-d")
            .arg(wrong_owner_output)
            .arg(fixture_dir.join("consumer-negative/WrongForeignOwner.scala")),
    );
    assert!(
        diagnostics.contains("Found:")
            && diagnostics.contains("Required:")
            && diagnostics.contains("example"),
        "{diagnostics}"
    );

    let _ = fs::remove_dir_all(task_temp);
}
