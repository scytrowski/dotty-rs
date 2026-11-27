#[test]
fn pinned_scala_oracle_keeps_original_exhaustive_boolean_cases() {
    let oracle = include_str!("fixtures/jvm_tasty_boolean_exhaustive_matches/oracle.txt");
    for shape in [
        "scala.Boolean",
        "case true",
        "case false",
        "case (true | false)",
        "case n @ true",
        "guard_effect",
        "return",
        "scala.Long",
        "scala.Float",
        "scala.Double",
    ] {
        assert!(oracle.contains(shape), "missing {shape}");
    }
    assert!(!oracle.contains("case _"));
}

use dotty::core::SourceId;
use dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_class;

fn emit(methods: &str) -> dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts {
    emit_paired_jvm_tasty_class(
        &format!("package example; class BooleanExhaustiveMatches {{ {methods} }}"),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2478),
    )
    .unwrap_or_else(|e| panic!("{methods}: {e:?}"))
}

#[test]
fn pinned_scala_consumes_and_recompiles_original_rust_boolean_exhaustive_match_tasty() {
    if std::env::var("DOTTY_REQUIRE_SCALA_TASTY_CONSUMER").as_deref() != Ok("1") {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!(
            "fixtures/jvm_tasty_boolean_exhaustive_matches/BooleanExhaustiveMatches.scala"
        ),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-exhaustive-match-scala-{}",
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
            "/tests/fixtures/jvm_tasty_boolean_exhaustive_matches/run.py"
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
            "mandatory boolean-exhaustive-match runtime gate requires JDK 21+"
        );
    }
    available
}

fn run_java(pair: &dotty::jvm_tasty_artifacts::PairedJvmTastyArtifacts, name: &str, source: &str) {
    let directory = std::env::temp_dir().join(format!(
        "dotty-boolean-exhaustive-matchs-{}-{name}",
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
        "exhaustive boolean match oracle ok"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn original_rust_boolean_exhaustive_matches_execute_on_jdk21() {
    if !jdk_available() {
        return;
    }
    let pair = emit_paired_jvm_tasty_class(
        include_str!(
            "fixtures/jvm_tasty_boolean_exhaustive_matches/BooleanExhaustiveMatches.scala"
        ),
        "BooleanExhaustiveMatches.scala",
        SourceId::from_index(2477),
    )
    .unwrap();
    run_java(
        &pair,
        "JavaConsumer",
        include_str!("fixtures/jvm_tasty_boolean_exhaustive_matches/JavaConsumer.java"),
    );
}

#[test]
fn both_boolean_orders_literal_alternatives_and_binders_emit_complete_pairs() {
    for method in [
        "def f(x: Boolean): Int = x match { case true => 1; case false => 2 }",
        "def f(x: Boolean): Int = x match { case false => 2; case true => 1 }",
        "def f(x: Boolean): Int = x match { case true | false => 1 }",
        "def f(x: Boolean): Int = x match { case ((true | true) | false) => 1 }",
        "def f(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n }",
    ] {
        emit(method);
    }
}

#[test]
fn exhaustive_boolean_case_binders_keep_distinct_direct_targets() {
    use dotty_tasty::tasty::*;
    let output =
        emit("def f(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n }");
    let file = TastyFile::parse_and_validate_scala_3_9(output.tasty_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let binds: Vec<_> = index
        .iter_nodes_with_tag(BIND_TAG)
        .map(|n| n.offset as u32)
        .collect();
    assert_eq!(binds.len(), 2);
    assert_ne!(binds[0], binds[1]);
    let refs: Vec<_> = index
        .iter_nodes_with_tag(TERMREFDIRECT_TAG)
        .filter_map(|n| {
            let RawTree::Leaf(SimpleTerm {
                value: TermValue::AstRef(target),
                ..
            }) = RawTree::decode(&mut Reader::new(
                &file.section(StandardSection::Asts).unwrap().payload[n.offset..],
            ))
            .unwrap()
            else {
                return None;
            };
            binds.contains(&target).then_some(target)
        })
        .collect();
    assert_eq!(refs, binds);
}
