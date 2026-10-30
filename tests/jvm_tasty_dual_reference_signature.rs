use std::fs;
use std::path::PathBuf;
use std::process::Command;

use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::SourceId;
use dotty_core::ast::TreeKind;
use dotty_tasty::ast::{DEFDEF_TAG, LOCAL_TAG, PARAM_TAG, PRIVATE_TAG, VALDEF_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{DefinitionBody, DefinitionTail};
use dotty_tasty_pickler::{ClassStatePicklePlan, PrimaryConstructorInputPlan, SourceMetadata};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_dual_reference_constructors/PairSignatureOnly.scala");
const PAIR_SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");

#[test]
fn rust_signature_only_pair_emits_two_typed_template_and_constructor_params() {
    let outputs = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "PairSignatureOnly.scala",
        SourceId::from_index(1876),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class
                            .tasty_plan
                            .pickle()
                            .expect("validated signature-only class emits TASTy")
                            .bytes()
                            .to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("one source session validates Counter and Pair");
    let pair = outputs
        .iter()
        .find(|(path, _)| path.ends_with("/Pair.tasty"))
        .expect("batch contains Pair.tasty");
    let tasty =
        TastyFile::parse_and_validate_scala_3_9(&pair.1).expect("Rust-produced TASTy 28.9 decodes");
    assert_eq!(
        (
            tasty.header().major_version,
            tasty.header().minor_version,
            tasty.header().experimental_version,
        ),
        (28, 9, 0)
    );
    let index = tasty.ast_address_index().unwrap();
    let parameter_nodes = index.iter_nodes_with_tag(PARAM_TAG).collect::<Vec<_>>();
    let parameter_addresses = parameter_nodes
        .iter()
        .map(|node| node.offset as u32)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        parameter_addresses.len(),
        4,
        "each template/constructor PARAM is distinct"
    );
    let parameters = parameter_nodes
        .iter()
        .map(|node| {
            let parameter = index
                .get(node.offset as u32)
                .unwrap()
                .decode_parameter()
                .unwrap();
            let body = parameter.decode_body().unwrap();
            let name = tasty.names().render(parameter.name()).unwrap();
            let type_names = body
                .type_tree
                .name_refs()
                .into_iter()
                .map(|reference| tasty.names().render(reference).unwrap())
                .collect::<Vec<_>>();
            let modifiers = body
                .tail
                .iter()
                .filter_map(|tail| match tail {
                    DefinitionTail::Modifier(tag) => Some(*tag),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (name.to_owned(), type_names, modifiers)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parameters,
        [
            (
                "first".to_owned(),
                vec!["Counter".to_owned(), "example".to_owned()],
                vec![PRIVATE_TAG, LOCAL_TAG],
            ),
            (
                "second".to_owned(),
                vec!["Counter".to_owned(), "example".to_owned()],
                vec![PRIVATE_TAG, LOCAL_TAG],
            ),
            ("first".to_owned(), vec!["Counter".to_owned()], vec![]),
            ("second".to_owned(), vec!["Counter".to_owned()], vec![]),
        ]
    );
    assert_eq!(index.iter_nodes_with_tag(VALDEF_TAG).count(), 0);
    assert_eq!(index.iter_nodes_with_tag(DEFDEF_TAG).count(), 1);
}

#[test]
fn pinned_scala39_decompiles_and_rebuilds_rust_signature_only_sidecars() {
    if std::env::var_os("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_SIGNATURE").is_none() {
        eprintln!(
            "skipping pinned Scala signature oracle; set DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_SIGNATURE=1 to require it"
        );
        return;
    }
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let version = Command::new(&scalac)
        .arg("-version")
        .output()
        .expect("run pinned Scala compiler");
    assert!(version.status.success());
    let version_text = String::from_utf8_lossy(&version.stderr).to_string()
        + &String::from_utf8_lossy(&version.stdout);
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );
    let java = java_home.join("bin/java");
    let java_version = Command::new(&java)
        .arg("-version")
        .output()
        .expect("run Java 21");
    assert!(java_version.status.success());
    let java_text = String::from_utf8_lossy(&java_version.stderr).to_string()
        + &String::from_utf8_lossy(&java_version.stdout);
    assert!(java_text.contains("21.0.13"), "{java_text}");

    let outputs = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "PairSignatureOnly.scala",
        SourceId::from_index(18761),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class.tasty_plan.pickle().unwrap().bytes().to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("the exact source batch validates and emits original Rust sidecars");
    let temporary = std::env::temp_dir().join(format!(
        "dotty-issue-1876-scala-signature-{}",
        std::process::id()
    ));
    let candidate = temporary.join("candidate");
    let rebuilt = temporary.join("rebuilt");
    fs::remove_dir_all(&temporary).ok();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&rebuilt).unwrap();
    for (path, bytes) in outputs {
        let path = candidate.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let pair_tasty = candidate.join("example/Pair.tasty");
    let decompile = Command::new(&scalac)
        .arg("-decompile")
        .arg(&pair_tasty)
        .output()
        .expect("Scala 3.9 -decompile on genuine Rust Pair.tasty");
    assert!(
        decompile.status.success(),
        "Scala -decompile rejected Rust Pair.tasty: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    assert!(decompiled.contains("first"), "{decompiled}");
    assert!(decompiled.contains("second"), "{decompiled}");
    assert!(decompiled.matches("Counter").count() >= 2, "{decompiled}");

    let from_tasty = Command::new(&scalac)
        .arg("-from-tasty")
        .arg("-classpath")
        .arg(&candidate)
        .arg("-d")
        .arg(&rebuilt)
        .arg(candidate.join("example/Counter.tasty"))
        .arg(&pair_tasty)
        .output()
        .expect("Scala 3.9 -from-tasty on genuine Rust Counter and Pair sidecars");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected Rust sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new(java_home.join("bin/javap"))
        .args(["-p", "-s", "-classpath"])
        .arg(&rebuilt)
        .arg("example.Pair")
        .output()
        .expect("inspect Scala-rebuilt signature-only Pair");
    assert!(
        javap.status.success(),
        "javap failed: {}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_text = String::from_utf8_lossy(&javap.stdout);
    assert!(
        javap_text.contains("(Lexample/Counter;Lexample/Counter;)V"),
        "{javap_text}"
    );
    assert!(
        !javap_text.contains("private "),
        "signature-only Pair must have no fields: {javap_text}"
    );
    fs::remove_dir_all(temporary).unwrap();
}

fn pair_sidecar(source: &str, source_name: &str, class_name: &str) -> Vec<u8> {
    with_paired_jvm_tasty_batch_plan(source, source_name, SourceId::from_index(18762), |batch| {
        let class = batch
            .classes
            .iter()
            .find(|class| class.tasty_path.ends_with(&format!("/{class_name}.tasty")))
            .expect("expected class in same-source batch");
        class.tasty_plan.pickle().unwrap().bytes().to_vec()
    })
    .expect("signature-only batch validates")
}

fn full_pair_sidecars() -> Vec<(String, Vec<u8>)> {
    with_paired_jvm_tasty_batch_plan(
        PAIR_SOURCE,
        "Pair.scala",
        SourceId::from_index(18777),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class.tasty_plan.pickle().unwrap().bytes().to_vec(),
                    )
                })
                .collect()
        },
    )
    .expect("the complete two-alias source batch is validated")
}

#[test]
fn rust_pair_tasty_has_two_ordered_field_aliases_and_read_bodies() {
    let outputs = full_pair_sidecars();
    let pair = outputs
        .iter()
        .find(|(path, _)| path.ends_with("/Pair.tasty"))
        .unwrap();
    let tasty = TastyFile::parse_and_validate_scala_3_9(&pair.1).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let parameters = index
        .iter_nodes_with_tag(PARAM_TAG)
        .map(|node| {
            let parameter = index
                .get(node.offset as u32)
                .unwrap()
                .decode_parameter()
                .unwrap();
            (
                node.offset as u32,
                tasty.names().render(parameter.name()).unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        parameters
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "first", "second"]
    );
    let field_nodes = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .map(|node| {
            (
                node.offset as u32,
                index
                    .get(node.offset as u32)
                    .unwrap()
                    .decode_definition_body()
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(field_nodes.len(), 2);
    for (((_, field), expected_name), expected_parameter) in field_nodes
        .iter()
        .zip(["left", "right"])
        .zip([parameters[0].0, parameters[1].0])
    {
        let DefinitionBody::ValDef {
            name,
            rhs: Some(rhs),
            tail,
            ..
        } = field
        else {
            panic!("Pair contains immutable VALDEF aliases with RHS trees")
        };
        assert_eq!(tasty.names().render(*name).unwrap(), expected_name);
        assert!(tail.contains(&DefinitionTail::Modifier(PRIVATE_TAG)));
        assert!(tail.contains(&DefinitionTail::Modifier(LOCAL_TAG)));
        let reference = rhs.decode_reference().unwrap();
        assert_eq!(reference.tag, dotty_tasty::ast::TERMREFSYMBOL_TAG);
        assert_eq!(reference.reference, expected_parameter);
    }
    let methods = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .filter_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            Some((
                node.offset as u32,
                tasty.names().render(body.name).ok()?.to_owned(),
                body.rhs.is_some(),
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods
            .iter()
            .map(|(_, name, has_body)| (name.as_str(), *has_body))
            .collect::<Vec<_>>(),
        [("<init>", false), ("leftValue", true), ("rightValue", true)]
    );
    for (method_name, expected_field) in [("leftValue", "left"), ("rightValue", "right")] {
        let (address, _, _) = methods
            .iter()
            .find(|(_, name, _)| name == method_name)
            .unwrap();
        let mut pending = vec![*address];
        let mut selected = Vec::new();
        while let Some(parent) = pending.pop() {
            for child in index.children_of(parent) {
                if child.tag == dotty_tasty::ast::SELECTIN_TAG {
                    let select = index
                        .get(child.offset as u32)
                        .unwrap()
                        .decode_select_in()
                        .unwrap();
                    let name = tasty
                        .names()
                        .render_signed_name(select.name)
                        .unwrap()
                        .map(|name| match name {
                            dotty_tasty::name_table::RenderedSignedName::Signed {
                                original,
                                ..
                            }
                            | dotty_tasty::name_table::RenderedSignedName::TargetSigned {
                                original,
                                ..
                            } => original,
                        })
                        .unwrap_or_else(|| tasty.names().render(select.name).unwrap());
                    selected.push(name);
                    if let dotty_tasty::ast::StructuredTree::Select(field_select) =
                        select.qualifier.decode_structured().unwrap()
                    {
                        selected.push(tasty.names().render(field_select.name).unwrap().to_owned());
                    }
                }
                pending.push(child.offset as u32);
            }
        }
        assert!(
            selected.iter().any(|name| name == expected_field),
            "{method_name}: {selected:?}"
        );
        assert!(
            selected.iter().any(|name| name == "get"),
            "{method_name}: {selected:?}"
        );
    }
}

#[test]
fn dual_reference_alias_validation_rejects_swaps_reuse_order_and_extra_fields() {
    let swapped = PAIR_SOURCE
        .replace(
            "private val left: Counter = first",
            "private val left: Counter = second",
        )
        .replace(
            "private val right: Counter = second",
            "private val right: Counter = first",
        );
    let reused = PAIR_SOURCE.replace(
        "private val right: Counter = second",
        "private val right: Counter = first",
    );
    let reversed_fields = PAIR_SOURCE.replace(
        "  private val left: Counter = first\n  private val right: Counter = second",
        "  private val right: Counter = second\n  private val left: Counter = first",
    );
    let third_field = PAIR_SOURCE.replace(
        "  def leftValue(): Int = left.get()",
        "  private val extra: Counter = first\n  def leftValue(): Int = left.get()",
    );
    for (label, source, expected_reason) in [
        ("swapped", swapped, "UnsafeFieldInitializer"),
        ("reused", reused, "UnsafeFieldInitializer"),
        (
            "reversed source order",
            reversed_fields,
            "UnsafeFieldInitializer",
        ),
        ("third alias", third_field, "ClassMembers"),
    ] {
        let error = with_paired_jvm_tasty_batch_plan(
            &source,
            "PairNegative.scala",
            SourceId::from_index(18778),
            |_| (),
        )
        .expect_err("invalid dual-reference alias edges fail closed");
        let rendered = format!("{error:?}");
        eprintln!("{label} alias first rejection: {rendered}");
        assert!(
            rendered.contains("TASTy plan validation"),
            "{label}: {rendered}"
        );
        assert!(rendered.contains(expected_reason), "{label}: {rendered}");
    }
    for (label, source, first_phase, expected_reason) in [
        (
            "mutable",
            PAIR_SOURCE.replace("private val left", "private var left"),
            "method body typing",
            "UnstableSelectionPrefix",
        ),
        (
            "public",
            PAIR_SOURCE.replace("private val left", "val left"),
            "member planning",
            "only unqualified private mutable fields",
        ),
    ] {
        let error = with_paired_jvm_tasty_batch_plan(
            &source,
            "PairNegative.scala",
            SourceId::from_index(18779),
            |_| (),
        )
        .expect_err("unsupported reference field modifiers fail closed");
        let rendered = format!("{error:?}");
        eprintln!("{label} field first rejection: {rendered}");
        assert!(rendered.contains(first_phase), "{label}: {rendered}");
        assert!(rendered.contains(expected_reason), "{label}: {rendered}");
    }
}

#[test]
fn renamed_reordered_full_alias_batch_emits_deterministically() {
    let source = "package renamed.example\n\nclass Envelope(firstInput: Token, secondInput: Token):\n  private val firstAlias: Token = firstInput\n  private val secondAlias: Token = secondInput\n  def firstValue(): Int = firstAlias.get()\n  def secondValue(): Int = secondAlias.get()\n\nclass Token(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n";
    let emit = || {
        with_paired_jvm_tasty_batch_plan(
            source,
            "Envelope.scala",
            SourceId::from_index(187780),
            |batch| {
                batch
                    .classes
                    .iter()
                    .map(|class| {
                        (
                            class.tasty_path.clone(),
                            class.tasty_plan.pickle().unwrap().bytes().to_vec(),
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .unwrap()
    };
    let first = emit();
    let second = emit();
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .any(|(path, _)| path.ends_with("/Envelope.tasty"))
    );
    assert!(first.iter().any(|(path, _)| path.ends_with("/Token.tasty")));
}

#[test]
fn pinned_scala39_decompiles_and_rebuilds_genuine_rust_pair_alias_sidecars() {
    if std::env::var_os("DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_ALIAS").is_none() {
        eprintln!(
            "skipping pinned Scala alias oracle; set DOTTY_REQUIRE_SCALA_TASTY_DUAL_REFERENCE_ALIAS=1 to require it"
        );
        return;
    }
    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let scalac = scala_root.join("dist/target/universal/stage/bin/scalac");
    let revision = Command::new("git")
        .arg("-C")
        .arg(&scala_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read pinned Scala revision");
    assert!(revision.status.success());
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let version = Command::new(&scalac)
        .arg("-version")
        .output()
        .expect("run pinned scalac");
    assert!(version.status.success());
    let version_text = String::from_utf8_lossy(&version.stderr).to_string()
        + &String::from_utf8_lossy(&version.stdout);
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );
    let java = java_home.join("bin/java");
    let java_version = Command::new(&java)
        .arg("-version")
        .output()
        .expect("run Java 21");
    assert!(java_version.status.success());
    let java_text = String::from_utf8_lossy(&java_version.stderr).to_string()
        + &String::from_utf8_lossy(&java_version.stdout);
    assert!(java_text.contains("21.0.13"), "{java_text}");

    let outputs = full_pair_sidecars();
    let temporary = std::env::temp_dir().join(format!(
        "dotty-issue-1877-scala-alias-{}",
        std::process::id()
    ));
    let candidate = temporary.join("candidate");
    let rebuilt = temporary.join("rebuilt");
    fs::remove_dir_all(&temporary).ok();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&rebuilt).unwrap();
    for (path, bytes) in outputs {
        let destination = candidate.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, bytes).unwrap();
    }
    let pair_tasty = candidate.join("example/Pair.tasty");
    let decompile = Command::new(&scalac)
        .arg("-decompile")
        .arg(&pair_tasty)
        .output()
        .expect("Scala 3.9 -decompile genuine Rust Pair.tasty");
    assert!(
        decompile.status.success(),
        "Scala -decompile rejected Rust Pair.tasty: {}",
        String::from_utf8_lossy(&decompile.stderr)
    );
    let decompiled = String::from_utf8_lossy(&decompile.stdout);
    for name in [
        "first",
        "second",
        "left",
        "right",
        "leftValue",
        "rightValue",
        "get",
    ] {
        assert!(
            decompiled.contains(name),
            "missing {name} in decompiled output:\n{decompiled}"
        );
    }
    let from_tasty = Command::new(&scalac)
        .args(["-from-tasty", "-classpath"])
        .arg(&candidate)
        .arg("-d")
        .arg(&rebuilt)
        .arg(candidate.join("example/Counter.tasty"))
        .arg(&pair_tasty)
        .output()
        .expect("Scala 3.9 -from-tasty genuine Rust Counter and Pair sidecars");
    assert!(
        from_tasty.status.success(),
        "Scala -from-tasty rejected Rust sidecars: {}",
        String::from_utf8_lossy(&from_tasty.stderr)
    );
    let javap = Command::new(java_home.join("bin/javap"))
        .args(["-p", "-s", "-classpath"])
        .arg(&rebuilt)
        .arg("example.Pair")
        .output()
        .expect("inspect Scala-rebuilt Pair");
    assert!(
        javap.status.success(),
        "javap failed: {}",
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_text = String::from_utf8_lossy(&javap.stdout);
    assert!(
        javap_text.contains("(Lexample/Counter;Lexample/Counter;)V"),
        "{javap_text}"
    );
    assert!(
        javap_text.contains("private final example.Counter left"),
        "{javap_text}"
    );
    assert!(
        javap_text.contains("private final example.Counter right"),
        "{javap_text}"
    );
    fs::remove_dir_all(temporary).unwrap();
}

#[test]
fn renamed_and_reordered_signature_only_sources_emit_deterministically() {
    let original = SOURCE;
    let reordered = "package example\n\nclass Counter(start: Int)\n\nclass Pair(first: Counter, second: Counter)\n";
    let renamed = "package renamed.example\n\nclass Envelope(first: Token, second: Token)\n\nclass Token(start: Int)\n";
    for (source, source_name, class_name) in [
        (original, "PairSignatureOnly.scala", "Pair"),
        (reordered, "Reordered.scala", "Pair"),
        (renamed, "Renamed.scala", "Envelope"),
    ] {
        assert_eq!(
            pair_sidecar(source, source_name, class_name),
            pair_sidecar(source, source_name, class_name),
            "repeat output for {source_name} is deterministic",
        );
    }
}

#[test]
fn class_state_rejects_corrupt_dual_formal_identity_before_output() {
    let source = SOURCE;
    with_paired_jvm_tasty_batch_plan(
        source,
        "PairSignatureOnly.scala",
        SourceId::from_index(18763),
        |batch| {
            let pair = batch
                .classes
                .iter()
                .find(|class| class.tasty_path.ends_with("/Pair.tasty"))
                .expect("Pair plan exists")
                .tasty_plan
                .plan();
            let constructor = &pair.constructor;
            let TreeKind::DefDef(definition) = &pair.source_ast.get(constructor.source_tree).kind
            else {
                panic!("primary constructor source is a DefDef");
            };
            let trees = &definition.value_param_clauses[0];
            let template_first = pair
                .source_index
                .symbol_at(pair.source.id, trees[0])
                .unwrap();
            let original = constructor.clone();
            let check = |candidate: PrimaryConstructorInputPlan, source_id: SourceId| {
                let corrupted = ClassStatePicklePlan {
                    store: pair.store,
                    definitions: pair.definitions,
                    source_ast: pair.source_ast,
                    typed_ast: pair.typed_ast,
                    source_index: pair.source_index,
                    typed_bindings: pair.typed_bindings,
                    source: SourceMetadata {
                        id: source_id,
                        name: pair.source.name.clone(),
                    },
                    class: pair.class,
                    class_tree: pair.class_tree,
                    constructor: candidate,
                    members: pair.members,
                };
                match corrupted.validate() {
                    Err(error) => format!("{error:?}"),
                    Ok(_) => panic!("corrupt signature must fail before TASTy output"),
                }
            };

            let mut missing = original.clone();
            missing.parameter_clauses[0].pop();
            let missing_error = check(missing, pair.source.id);
            assert!(
                missing_error.contains("PrimaryConstructorInput"),
                "{missing_error}"
            );

            let mut duplicate = original.clone();
            duplicate.parameter_clauses[0][1] = duplicate.parameter_clauses[0][0];
            let duplicate_error = check(duplicate, pair.source.id);
            assert!(
                duplicate_error.contains("SourceBinding")
                    || duplicate_error.contains("DuplicateDeclaration")
                    || duplicate_error.contains("ScopeMembers"),
                "{duplicate_error}"
            );

            let mut swapped = original.clone();
            swapped.parameter_clauses[0].reverse();
            let swapped_error = check(swapped, pair.source.id);
            assert!(swapped_error.contains("ScopeMembers"), "{swapped_error}");

            let mut confused_owner = original.clone();
            confused_owner.parameter_clauses[0][0] = template_first;
            let owner_error = check(confused_owner, pair.source.id);
            assert!(
                owner_error.contains("SourceBinding")
                    || owner_error.contains("Owner")
                    || owner_error.contains("ScopeMembers"),
                "{owner_error}"
            );

            let mut third = original;
            third.parameter_clauses[0].push(template_first);
            let third_error = check(third, pair.source.id);
            assert!(
                third_error.contains("PrimaryConstructorInput"),
                "{third_error}"
            );

            let cross_source_error = check(constructor.clone(), SourceId::from_index(1));
            assert!(
                cross_source_error.contains("SourceBinding")
                    || cross_source_error.contains("SourceOrigin")
                    || cross_source_error.contains("SourceDefinition"),
                "{cross_source_error}"
            );
        },
    )
    .expect("the canonical signature-only batch itself is valid");
}
