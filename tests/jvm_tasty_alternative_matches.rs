#[test]
fn pinned_semantic_oracle_preserves_alternative_structure_and_returns() {
    let oracle = include_str!("fixtures/jvm_tasty_alternative_matches/oracle.txt");
    for shape in [
        "(-2147483648 | 0 | 2147483647)",
        "(0 | 0)",
        "((0 | 1) | 2)",
        "scala.Long",
        "return",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class AlternativeMatches {{ {methods} }}"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn tasty_alternatives_preserve_nested_structure_and_literal_order() {
    use dotty_tasty::tasty::*;
    let pair = emit(
        "def f(x: Int): Int = x match { case (-2147483648 | 0) | 2147483647 => 10; case _ => 20 }",
    );
    let file = TastyFile::parse_and_validate_scala_3_9(pair.tasty_bytes()).unwrap();
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let index = file.ast_address_index().unwrap();
    let mut alternatives = index.iter_nodes_with_tag(ALTERNATIVE_TAG);
    let root = alternatives.next().unwrap();
    let RawTree::LengthNode(node) =
        RawTree::decode(&mut Reader::new(&payload[root.offset..])).unwrap()
    else {
        panic!()
    };
    let outer = node.decode_alternative().unwrap();
    assert_eq!(outer.alternatives.len(), 2);
    let RawTree::LengthNode(inner) = &outer.alternatives[0] else {
        panic!()
    };
    assert_eq!(inner.tag, ALTERNATIVE_TAG);
    let inner = inner.decode_alternative().unwrap();
    let values: Vec<_> = inner
        .alternatives
        .iter()
        .chain([&outer.alternatives[1]])
        .map(|v| {
            let RawTree::Leaf(v) = v else { panic!() };
            assert_eq!(v.tag, INTCONST_TAG);
            let TermValue::Int(value) = v.value else {
                panic!()
            };
            value
        })
        .collect();
    assert_eq!(values, [i32::MIN, 0, i32::MAX]);
    assert_eq!(alternatives.count(), 1);
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_alternative_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_alternative_matches/AlternativeMatches.scala"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-alternative-match-scala-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/jvm_tasty_alternative_matches/run.py"
        ))
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}

fn jdk_available() -> bool {
    let version = std::process::Command::new("java")
        .arg("-version")
        .output()
        .ok();
    let major = version.as_ref().and_then(|output| {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        text.split('"')
            .nth(1)?
            .split('.')
            .next()?
            .parse::<u32>()
            .ok()
    });
    let available = major.is_some_and(|major| major >= 21);
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        assert!(
            available,
            "mandatory alternative-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-alternative-matchs-{}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join("example")).unwrap();
    std::fs::write(
        directory.join(pair.classfile_path()),
        pair.classfile_bytes(),
    )
    .unwrap();
    std::fs::write(directory.join(pair.tasty_path()), pair.tasty_bytes()).unwrap();
    let client = directory.join(format!("{name}.java"));
    std::fs::write(&client, source).unwrap();
    let compile = std::process::Command::new("javac")
        .args(["--release", "21", "-cp"])
        .arg(&directory)
        .arg("-d")
        .arg(&directory)
        .arg(&client)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new("java")
        .args(["-Xverify:all", "-ea", "-cp"])
        .arg(&directory)
        .arg(name)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "alternative match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_alternative_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!("fixtures/jvm_tasty_alternative_matches/AlternativeMatches.scala"),
        "AlternativeMatches.scala",
        SourceId::from_index(2474),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_alternative_matches/JavaConsumer.java"),
    );
}
