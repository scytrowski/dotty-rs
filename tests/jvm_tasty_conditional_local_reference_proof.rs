use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, ImmutableLocalConditionalReferenceSelectionProofInput,
    ImmutableLocalInitializerPairProofInput,
    validate_immutable_local_conditional_reference_selection_proof,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_core::ast::{AstArena, If, TreeKind, Typed};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolId,
    SymbolInfo, TermRefTarget, TreeId, Type, TypeRefTarget, Untyped,
};
use dotty_jvm::JvmCompilationUnit;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{
    APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, DefinitionBody, IF_TAG, PARAM_TAG, SELECTIN_TAG,
    SHAREDTYPE_TAG, TERMREFDIRECT_TAG, TERMREFPKG_TAG, TYPEREF_TAG, VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::reader::Reader as TastyReader;
use dotty_tasty::tasty::{RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree, TermValue};
use dotty_tasty::writer::Writer as TastyWriter;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, ConditionalLocalReferenceSelectionBinding,
    FormalLocalConstructorCallArgumentBinding, FormalLocalConstructorCallArgumentOrigin,
    FormalLocalConstructorCallPairBinding, LimitKind, LocalCounterInitializerBinding,
    LocalReceiverUnitCallBinding, LocalValueBinding, NestedConstructorCallBinding, PickleError,
    PickleLimits, PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings,
};
use dotty_typer::{SourceTyper, TypedLocalBinding};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/conditional_reference_artifacts.rs"]
mod conditional_reference_artifacts;
#[path = "support/conditional_rust_consumer.rs"]
mod conditional_rust_consumer;
#[path = "support/scala39_toolchain.rs"]
mod scala39_toolchain;

const REQUIRE_SCALA: &str = "DOTTY_REQUIRE_SCALA_TASTY_CONDITIONAL_LOCAL_REFERENCE_CONSUMER";
const SCALA_SUPPORT_SOURCE: &str = r#"package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()
"#;
const JAVA_CLASSFILE_SUPPORT: &str = r#"package example;

public final class PairFactory {
  public Pair create(int firstSeed, int secondSeed, boolean chooseFirst, int nextValue) {
    return null;
  }
}
"#;

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_conditional_reference_selection/PairFactory.scala");

#[derive(Clone, Copy, Debug)]
enum IndexMutation {
    None,
    MissingCondition,
    SwappedBranches,
    ConditionAsIntFormal,
    BranchAsIntFormal,
    WrongSetterReceiver,
    WrongPairArgument,
    WrongSelectedBinding,
    SwapTypedBranchTypes,
    SelectedTypedValDefInt,
    SetterReceiverKindIf,
    SetterArgumentKindIf,
    PairFirstArgumentKindIf,
    PairSecondArgumentKindIf,
    ConditionKindIf,
    ThenBranchKindIf,
    ElseBranchKindIf,
    FirstLocalValDefInt,
    SecondLocalValDefInt,
    BlockTypeInt,
    FirstLocalAnnotationKindIf,
    SecondLocalAnnotationKindIf,
    FirstCounterNewTypeKindIf,
    SecondCounterNewTypeKindIf,
    SelectedAnnotationKindIf,
    PairNewTypeKindIf,
    PostReceiptConditionKindIf,
}

#[derive(Debug)]
struct ReceiptIds {
    source: SourceId,
    method: dotty_core::SymbolId,
    condition_formal: dotty_core::SymbolId,
    targets: [dotty_core::SymbolId; 2],
    selected: dotty_core::SymbolId,
    scope: dotty_core::ScopeId,
    tasty: Vec<u8>,
}

struct ConditionalPicklerBindings<'a> {
    index: &'a SourceTypedIndex,
    source: SourceId,
    method: SymbolId,
    classes: Vec<SymbolId>,
    methods: Vec<SymbolId>,
    locals: Vec<LocalValueBinding>,
    initializers: Vec<LocalCounterInitializerBinding>,
    selection: ConditionalLocalReferenceSelectionBinding,
    setter: LocalReceiverUnitCallBinding,
    pair: FormalLocalConstructorCallPairBinding,
}

impl SourceTypedBindings for ConditionalPicklerBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.index.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.classes
    }
    fn planned_method_symbols(&self) -> &[SymbolId] {
        &self.methods
    }
    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        if source == self.source && method == self.method {
            self.locals.clone()
        } else {
            Vec::new()
        }
    }
    fn local_counter_initializers(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Vec<LocalCounterInitializerBinding> {
        if source == self.source && method == self.method {
            self.initializers.clone()
        } else {
            Vec::new()
        }
    }
    fn conditional_local_reference_selection(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<ConditionalLocalReferenceSelectionBinding> {
        (source == self.source && method == self.method).then_some(self.selection)
    }
    fn local_receiver_unit_call(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<LocalReceiverUnitCallBinding> {
        (source == self.source && method == self.method).then_some(self.setter)
    }
    fn formal_local_constructor_call_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<FormalLocalConstructorCallPairBinding> {
        (source == self.source && method == self.method).then_some(self.pair)
    }
}

fn raw_tree_at<'a>(asts: &'a [u8], address: u32) -> RawTree<'a> {
    let mut reader = TastyReader::with_range(asts, address as usize, asts.len()).unwrap();
    RawTree::decode(&mut reader).unwrap()
}

fn single_ast_reference(tree: &RawTree<'_>, expected_kind: AstRefKind) -> u32 {
    let references = tree.ast_refs();
    let [reference] = references.as_slice() else {
        panic!("tree contains exactly one AST reference: {references:?}")
    };
    assert_eq!(reference.kind, expected_kind);
    reference.address
}

fn assert_counter_type_tree(tree: &RawTree<'_>, tasty: &TastyFile<'_>) {
    fn inspect(
        tree: &RawTree<'_>,
        tasty: &TastyFile<'_>,
        counter_type: &mut bool,
        example_package: &mut bool,
    ) {
        match tree {
            RawTree::Leaf(term) if term.tag == TERMREFPKG_TAG => {
                let TermValue::NameRef(name) = term.value else {
                    panic!("TERMREFpkg carries a name reference")
                };
                *example_package |= tasty.names().render(name).unwrap() == "example";
            }
            RawTree::Ast { child, .. } => inspect(child, tasty, counter_type, example_package),
            RawTree::NatAst {
                tag, value, child, ..
            } => {
                if *tag == TYPEREF_TAG {
                    *counter_type |= tasty.names().render(*value).unwrap() == "Counter";
                }
                inspect(child, tasty, counter_type, example_package);
            }
            RawTree::Leaf(_) | RawTree::LengthNode(_) => {}
        }
    }

    let (mut counter_type, mut example_package) = (false, false);
    inspect(tree, tasty, &mut counter_type, &mut example_package);
    assert!(
        counter_type,
        "declaration type resolves to Counter: {tree:?}"
    );
    assert!(
        example_package,
        "declaration type is qualified by example: {tree:?}"
    );
}

fn assert_signed_select(
    tasty: &TastyFile<'_>,
    selection: &dotty_tasty::ast::SelectInNode<'_>,
    original: &str,
    parameters: &[&str],
    result: &str,
) {
    let rendered = tasty
        .names()
        .render_signed_name(selection.name)
        .unwrap()
        .expect("method selection has a signed name");
    let RenderedSignedName::Signed {
        original: actual_original,
        signature,
    } = rendered
    else {
        panic!("method selection is a plain SIGNED name")
    };
    assert_eq!(actual_original, original);
    assert_eq!(signature.result, result);
    assert_eq!(
        signature.parameters,
        parameters
            .iter()
            .map(|name| RenderedParamSig::TermParameter((*name).to_owned()))
            .collect::<Vec<_>>()
    );
}

fn assert_rust_candidate_if_shape(
    bytes: &[u8],
    local_names: [&str; 2],
    selected_name: &str,
    branch_names: [&str; 2],
    pair_argument_names: [&str; 2],
) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let (create_address, create) = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let address = node.offset as u32;
            let body = index.get(address)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()? == "create").then_some((address, body))
        })
        .expect("Rust candidate contains PairFactory.create");
    let class = index
        .iter_nodes_with_tag(dotty_tasty::ast::TYPEDEF_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_definition().ok())
                .is_some_and(|definition| {
                    tasty.names().render(definition.name()).ok().as_deref() == Some("PairFactory")
                })
        })
        .expect("candidate class definition is PairFactory");
    assert!(
        std::iter::successors(Some(index.get_node(create_address).unwrap()), |node| {
            index.parent_of(node.offset as u32)
        })
        .any(|owner| owner.offset == class.offset),
        "create method belongs to PairFactory"
    );
    let RawTree::LengthNode(_) = create.rhs.unwrap() else {
        panic!("create body is BLOCK")
    };
    let block_address = index
        .iter_nodes_with_tag(BLOCK_TAG)
        .find(|node| {
            std::iter::successors(Some(*node), |current| {
                index.parent_of(current.offset as u32)
            })
            .any(|parent| parent.offset as u32 == create_address)
        })
        .expect("create body contains BLOCK")
        .offset as u32;
    let block = index.get(block_address).unwrap().decode_block().unwrap();
    assert_eq!(block.stats.len(), 4);
    let direct_children = index.children_of(block_address).collect::<Vec<_>>();
    assert_eq!(
        direct_children
            .iter()
            .filter(|node| node.tag == VALDEF_TAG)
            .count(),
        3,
        "the source BLOCK owns all three local declarations"
    );
    let local_addresses = direct_children
        .iter()
        .filter(|node| node.tag == VALDEF_TAG)
        .map(|node| node.offset as u32)
        .collect::<Vec<_>>();
    let local_address_by_name = local_addresses
        .iter()
        .zip(local_names)
        .map(|(address, name)| (name, *address))
        .collect::<std::collections::HashMap<_, _>>();
    let local_definitions = local_addresses
        .iter()
        .map(|address| {
            let DefinitionBody::ValDef {
                name,
                type_tree,
                rhs,
                ..
            } = index
                .get(*address)
                .unwrap()
                .decode_definition_body()
                .unwrap()
            else {
                panic!("BLOCK declaration is a VALDEF")
            };
            (
                tasty.names().render(name).unwrap().to_owned(),
                type_tree,
                rhs,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        local_definitions
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect::<Vec<_>>(),
        [local_names[0], local_names[1], selected_name],
        "TASTy retains source declaration order and renamed identities"
    );
    for (address, (_, type_tree, _)) in local_addresses.iter().zip(&local_definitions) {
        assert_counter_type_tree(type_tree, &tasty);
        assert_eq!(
            index.parent_of(*address).unwrap().offset as u32,
            block_address,
            "local declaration is owned by the create BLOCK"
        );
    }

    let parameters = index
        .children_of(create_address)
        .filter(|node| node.tag == PARAM_TAG)
        .filter_map(|node| {
            let parameter = index.get(node.offset as u32)?.decode_parameter().ok()?;
            Some((
                tasty.names().render(parameter.name()).ok()?.to_owned(),
                node.offset as u32,
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let condition_formal = *parameters
        .get("chooseFirst")
        .unwrap_or_else(|| panic!("Boolean formal exists in {parameters:?}"));
    let next_value_formal = *parameters
        .get("nextValue")
        .unwrap_or_else(|| panic!("setter formal exists in {parameters:?}"));
    let selected_address = local_addresses[2];
    let selected = index
        .get(selected_address)
        .unwrap()
        .decode_definition_body()
        .unwrap();
    let DefinitionBody::ValDef {
        rhs: Some(selected),
        ..
    } = selected
    else {
        panic!("selected local has an IF initializer")
    };
    let RawTree::LengthNode(if_node) = selected else {
        panic!("selected initializer is bare IF, without TYPED")
    };
    assert_eq!(if_node.tag, IF_TAG);
    let if_node = if_node.decode_if().unwrap();
    assert_eq!(raw_tag(&if_node.condition), TERMREFDIRECT_TAG);
    assert_eq!(raw_tag(&if_node.then_branch), SHAREDTYPE_TAG);
    assert_eq!(raw_tag(&if_node.else_branch), SHAREDTYPE_TAG);
    assert_eq!(
        single_ast_reference(&if_node.condition, AstRefKind::TermRefDirect),
        condition_formal,
        "IF condition points to the canonical Boolean formal"
    );
    let first_branch_target = single_ast_reference(&if_node.then_branch, AstRefKind::SharedType);
    let second_branch_target = single_ast_reference(&if_node.else_branch, AstRefKind::SharedType);
    let asts = tasty
        .section(dotty_tasty::section::StandardSection::Asts)
        .unwrap();
    for (branch_target, local_name, label) in [
        (first_branch_target, branch_names[0], "then"),
        (second_branch_target, branch_names[1], "else"),
    ] {
        let expected_local = local_address_by_name[local_name];
        let direct = raw_tree_at(asts.payload, branch_target);
        assert_eq!(
            raw_tag(&direct),
            TERMREFDIRECT_TAG,
            "{label} SHAREDtype resolves to TERMREFdirect"
        );
        assert_eq!(
            single_ast_reference(&direct, AstRefKind::TermRefDirect),
            expected_local,
            "{label} branch references its original local VALDEF"
        );
    }

    let apply_addresses = direct_children
        .iter()
        .filter(|node| node.tag == APPLY_TAG)
        .map(|node| node.offset as u32)
        .collect::<Vec<_>>();
    let mut setter = None;
    let mut pair_constructor = None;
    for address in apply_addresses {
        let apply = index.get(address).unwrap().decode_apply().unwrap();
        let RawTree::LengthNode(selection_node) = &apply.function else {
            continue;
        };
        if selection_node.tag != SELECTIN_TAG {
            continue;
        }
        let selection = selection_node.decode_select_in().unwrap();
        let signed = tasty.names().render_signed_name(selection.name).unwrap();
        match signed {
            Some(RenderedSignedName::Signed { original, .. }) if original == "set" => {
                setter = Some((selection, apply));
            }
            Some(RenderedSignedName::Signed { original, .. }) if original == "<init>" => {
                pair_constructor = Some((selection, apply));
            }
            _ => {}
        }
    }
    let (setter_selection, setter_apply) = setter.expect("BLOCK contains selected.set call");
    assert_signed_select(
        &tasty,
        &setter_selection,
        "set",
        &["scala.Int"],
        "scala.Unit",
    );
    assert_eq!(
        single_ast_reference(&setter_selection.qualifier, AstRefKind::TermRefDirect),
        selected_address,
        "setter receiver is the selected VALDEF"
    );
    assert_eq!(setter_apply.arguments.len(), 1);
    assert_eq!(
        single_ast_reference(&setter_apply.arguments[0], AstRefKind::TermRefDirect),
        next_value_formal,
        "setter receives the original nextValue formal"
    );
    let (pair_selection, pair_apply) =
        pair_constructor.expect("BLOCK result calls Pair constructor");
    assert_signed_select(
        &tasty,
        &pair_selection,
        "<init>",
        &["example.Counter", "example.Counter"],
        "example.Pair",
    );
    assert_eq!(pair_apply.arguments.len(), 2);
    for (argument, local_name) in pair_apply.arguments.iter().zip(pair_argument_names) {
        assert_eq!(
            single_ast_reference(argument, AstRefKind::TermRefDirect),
            local_address_by_name[local_name],
            "final Pair argument preserves source order for {local_name}"
        );
    }
}

fn corrupt_shared_branch_address(bytes: &[u8]) -> Vec<u8> {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let asts = tasty
        .section(dotty_tasty::section::StandardSection::Asts)
        .unwrap();
    let selected = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find(|node| {
            index
                .get(node.offset as u32)
                .and_then(|raw| raw.decode_definition_body().ok())
                .is_some_and(|body| {
                    matches!(body, DefinitionBody::ValDef { name, .. } if tasty.names().render(name).ok().as_deref() == Some("selected"))
                })
        })
        .expect("selected VALDEF exists")
        .offset as u32;
    let if_node = index
        .children_of(selected)
        .find(|node| node.tag == IF_TAG)
        .expect("selected VALDEF owns IF");
    let raw_if = index.get(if_node.offset as u32).unwrap();
    let selected_if = raw_if.decode_if().unwrap();
    let reference_tree = &selected_if.then_branch;
    assert_eq!(raw_tag(reference_tree), SHAREDTYPE_TAG);
    let old_address = single_ast_reference(reference_tree, AstRefKind::SharedType);
    let invalid_address = old_address + 1;
    assert!(
        index.get_node(invalid_address).is_none(),
        "adjacent payload byte is not a visible AST node"
    );

    let mut old_nat = TastyWriter::default();
    old_nat.write_nat(old_address);
    let mut invalid_nat = TastyWriter::default();
    invalid_nat.write_nat(invalid_address);
    assert_eq!(
        old_nat.as_slice().len(),
        invalid_nat.as_slice().len(),
        "corrupt target retains its encoded width"
    );
    let reference_offset = reference_tree.nodes()[0].offset;
    let if_payload_offset = raw_if.payload.as_ptr() as usize - asts.payload.as_ptr() as usize;
    let asts_file_offset = asts.payload.as_ptr() as usize - bytes.as_ptr() as usize;
    assert_eq!(asts_file_offset, asts.offset);
    let tasty_offset = asts_file_offset + if_payload_offset + reference_offset + 1;
    assert_eq!(bytes[tasty_offset - 1], SHAREDTYPE_TAG);
    let mut corrupted = bytes.to_vec();
    corrupted[tasty_offset..tasty_offset + invalid_nat.as_slice().len()]
        .copy_from_slice(invalid_nat.as_slice());
    let corrupted_file = TastyFile::parse_and_validate_scala_3_9(&corrupted)
        .expect("shared-address mutation preserves the TASTy wire tree");
    let corrupted_index = corrupted_file.ast_address_index().unwrap();
    let corrupted_if = corrupted_index
        .get(if_node.offset as u32)
        .unwrap()
        .decode_if()
        .unwrap();
    assert_eq!(
        single_ast_reference(&corrupted_if.then_branch, AstRefKind::SharedType),
        invalid_address,
        "decoded nested IF exposes the corrupted SHAREDtype address"
    );
    assert!(
        corrupted_index.get_node(invalid_address).is_none(),
        "corrupted SHAREDtype no longer resolves to a visible node start"
    );
    corrupted
}

fn raw_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

fn process_output(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    command
        .output()
        .unwrap_or_else(|error| panic!("run {rendered}: {error}"))
}

fn successful_command(command: &mut Command) -> Output {
    let rendered = format!("{command:?}");
    let output = process_output(command);
    assert!(
        output.status.success(),
        "command failed: {rendered}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn verify_rust_candidate_with_pinned_scala(candidate: &[u8], selected_name: &str) {
    match std::env::var(REQUIRE_SCALA).as_deref() {
        Ok("1") => {}
        Ok(value) => panic!("{REQUIRE_SCALA} must equal 1 when set, got {value:?}"),
        Err(std::env::VarError::NotPresent) => {
            eprintln!("skipping pinned Scala reader; set {REQUIRE_SCALA}=1 to require it");
            return;
        }
        Err(error) => panic!("read {REQUIRE_SCALA}: {error}"),
    }

    let scratch = std::env::temp_dir();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    let root = scratch.join(format!(
        "dotty-conditional-reference-reader-{}-{nonce}",
        std::process::id()
    ));
    let support_source_dir = root.join("support-source");
    let dependencies = root.join("dependencies");
    let classfile_support = root.join("classfile-support");
    let candidate_dir = root.join("rust-candidate");
    let reconstructed = root.join("from-tasty-output");
    for directory in [
        &support_source_dir,
        &dependencies,
        &classfile_support,
        &candidate_dir,
        &reconstructed,
    ] {
        fs::create_dir_all(directory).expect("create a fresh Scala reader directory");
        assert_eq!(
            fs::read_dir(directory)
                .expect("read fresh directory")
                .count(),
            0,
            "Scala reader output directory starts empty: {}",
            directory.display()
        );
    }
    let support_source = support_source_dir.join("CounterPair.scala");
    fs::write(&support_source, SCALA_SUPPORT_SOURCE).expect("write Scala dependency source");
    let candidate_path = candidate_dir.join("PairFactory.tasty");
    fs::write(&candidate_path, candidate).expect("write actual Rust-produced candidate TASTy");

    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
    let temporary_java_options = format!("-XX:-UsePerfData -Djava.io.tmpdir={}", scratch.display());
    let pinned = scala39_toolchain::from_environment(process_output, |command| {
        command
            .env("JAVA_HOME", &java_home)
            .env("JAVA_TOOL_OPTIONS", &temporary_java_options);
    });
    eprintln!(
        "CONDITIONAL_REFERENCE_SCALA_GATE revision={} java={}",
        scala39_toolchain::SCALA_REVISION,
        pinned.java.display()
    );
    let mut compile_support =
        scala39_toolchain::scalac(&pinned.java_home, &pinned.scalac, scratch.clone());
    successful_command(
        compile_support
            .args(["-Werror", "-release", "21", "-d"])
            .arg(&dependencies)
            .arg(&support_source),
    );

    let java_stub_source = support_source_dir.join("PairFactory.java");
    fs::write(&java_stub_source, JAVA_CLASSFILE_SUPPORT).expect("write Java classfile support");
    let mut compile_classfile_support = Command::new(java_home.join("bin/javac"));
    successful_command(
        compile_classfile_support
            .args(["--release", "21", "-classpath"])
            .arg(&dependencies)
            .arg("-d")
            .arg(&classfile_support)
            .arg(&java_stub_source),
    );

    assert!(!dependencies.join("example/PairFactory.class").exists());
    assert!(!dependencies.join("example/PairFactory.tasty").exists());
    assert!(
        classfile_support
            .join("example/PairFactory.class")
            .is_file()
    );
    assert!(!classfile_support.join("example/PairFactory.tasty").exists());
    assert_eq!(
        fs::read_dir(&candidate_dir)
            .expect("inspect candidate inputs")
            .map(|entry| entry.expect("read candidate entry").file_name())
            .collect::<Vec<_>>(),
        [std::ffi::OsString::from("PairFactory.tasty")],
        "Scala reader consumes the Rust candidate as its only PairFactory input"
    );

    let classpath = format!(
        "{}:{}:{}",
        classfile_support.display(),
        dependencies.display(),
        pinned.libraries
    );
    let mut print_tasty =
        scala39_toolchain::scalac(&pinned.java_home, &pinned.scalac, scratch.clone());
    let printed = successful_command(
        print_tasty
            .args(["-color:never", "-print-tasty"])
            .arg(&candidate_path),
    );
    let printed = String::from_utf8_lossy(&printed.stdout);
    assert!(
        printed.contains(&format!("[{selected_name}]")),
        "Scala reads the selected local declaration from the Rust candidate:\n{printed}"
    );
    let lines = printed.lines().collect::<Vec<_>>();
    let if_line = lines
        .iter()
        .position(|line| line.contains("IF("))
        .unwrap_or_else(|| panic!("Scala print-tasty contains the Rust candidate IF:\n{printed}"));
    assert!(
        lines
            .get(if_line + 1)
            .is_some_and(|line| line.contains("TERMREFdirect"))
            && lines
                .get(if_line + 2)
                .is_some_and(|line| line.contains("SHAREDtype"))
            && lines
                .get(if_line + 3)
                .is_some_and(|line| line.contains("SHAREDtype")),
        "Scala reads the Rust candidate condition and both shared local branches:\n{printed}"
    );
    assert!(
        printed.contains("IF("),
        "Scala reads the Rust candidate IF:\n{printed}"
    );

    let mut from_tasty =
        scala39_toolchain::scalac(&pinned.java_home, &pinned.scalac, scratch.clone());
    successful_command(
        from_tasty
            .args(["-release", "21", "-from-tasty", "-classpath"])
            .arg(&classpath)
            .arg("-d")
            .arg(&reconstructed)
            .arg(&candidate_path),
    );
    let output_entries = fs::read_dir(&reconstructed)
        .expect("inspect fresh from-tasty output")
        .count();
    assert_eq!(
        output_entries, 0,
        "Scala 3.9 -from-tasty accepts the explicit candidate but does not republish it when the classfile-only support symbol is already on the classpath"
    );
    eprintln!(
        "SCALA_FROM_TASTY candidate_input=Rust PairFactory.tasty output_entries={output_entries}"
    );

    assert_eq!(
        fs::read(&candidate_path).expect("candidate remains present"),
        candidate,
        "Scala reader did not replace the Rust-produced candidate"
    );
}

fn authenticate(
    source: &str,
    source_number: u32,
    mutation: IndexMutation,
) -> Result<ReceiptIds, CrossClassConstructorCallProofError> {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        conditional_reference_artifacts::PRODUCER_SOURCE_NAME,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source names without diagnostics: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("package root")
    };
    let mut classes = Vec::new();
    let mut factory_method = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            continue;
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
            panic!("class template")
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_def) = &parsed.ast.get(template.constructor).kind else {
            panic!("constructor")
        };
        let constructor_formals = constructor_def
            .value_param_clauses
            .iter()
            .flatten()
            .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
            .collect::<Vec<_>>();
        classes.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(def) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(def.name.as_name().text()) == "create" {
                factory_method = Some((member, def.clone()));
            }
        }
    }
    let (method_tree, method_def) = factory_method.expect("create method");
    let method = source_index.symbol_at(source_id, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, parameters) in &classes {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for parameter in parameters {
            typer.complete_symbol(*parameter).unwrap();
        }
    }
    typer.complete_symbol(method).unwrap();
    let formals = method_def.value_param_clauses[0]
        .iter()
        .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
        .collect::<Vec<_>>();
    for formal in &formals {
        typer.complete_symbol(*formal).unwrap();
    }
    let rhs = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("factory block")
    };
    let mut local_bindings = Vec::<TypedLocalBinding>::new();
    for declaration in &block.stats[..3] {
        local_bindings.push(
            typer
                .typed_local_binding_at(source_id, *declaration)
                .unwrap(),
        );
    }
    let mut source_typed_index = typer.source_typed_index().clone();
    let mut typed_ast = AstArena::<Typed>::new();
    for (_, node) in typer.typed_ast().iter() {
        typed_ast.alloc(node.clone());
    }
    let TreeKind::ValDef(selected_def) = &parsed.ast.get(block.stats[2]).kind else {
        panic!("selected declaration")
    };
    let TreeKind::If(if_node) = &parsed.ast.get(selected_def.rhs.unwrap()).kind else {
        panic!("selection IF")
    };
    let typed_if = source_typed_index
        .get(source_id, selected_def.rhs.unwrap())
        .unwrap();
    match mutation {
        IndexMutation::None => {}
        IndexMutation::MissingCondition => {
            source_typed_index.remove(source_id, if_node.cond);
        }
        IndexMutation::SwappedBranches => {
            let then_typed = source_typed_index
                .remove(source_id, if_node.then_branch)
                .unwrap();
            let else_typed = source_typed_index
                .remove(source_id, if_node.else_branch)
                .unwrap();
            source_typed_index
                .insert(source_id, if_node.then_branch, else_typed)
                .unwrap();
            source_typed_index
                .insert(source_id, if_node.else_branch, then_typed)
                .unwrap();
        }
        IndexMutation::ConditionAsIntFormal => {
            let int_use = match &parsed.ast.get(block.stats[3]).kind {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            };
            let int_typed = source_typed_index.get(source_id, int_use).unwrap();
            source_typed_index.remove(source_id, if_node.cond);
            source_typed_index
                .insert(source_id, if_node.cond, int_typed)
                .unwrap();
        }
        IndexMutation::BranchAsIntFormal => {
            let int_use = match &parsed.ast.get(block.stats[3]).kind {
                TreeKind::Apply(application) => application.args[0],
                _ => unreachable!(),
            };
            let int_typed = source_typed_index.get(source_id, int_use).unwrap();
            source_typed_index.remove(source_id, if_node.then_branch);
            source_typed_index
                .insert(source_id, if_node.then_branch, int_typed)
                .unwrap();
        }
        IndexMutation::WrongSetterReceiver => {
            let TreeKind::Apply(setter) = &parsed.ast.get(block.stats[3]).kind else {
                unreachable!()
            };
            let TreeKind::Select(select) = &parsed.ast.get(setter.function).kind else {
                unreachable!()
            };
            let first_typed = source_typed_index.get(source_id, block.stats[0]).unwrap();
            source_typed_index.remove(source_id, select.qualifier);
            source_typed_index
                .insert(source_id, select.qualifier, first_typed)
                .unwrap();
        }
        IndexMutation::WrongPairArgument => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block.expr).kind else {
                unreachable!()
            };
            let selected_typed = source_typed_index.get(source_id, block.stats[2]).unwrap();
            source_typed_index.remove(source_id, pair.args[0]);
            source_typed_index
                .insert(source_id, pair.args[0], selected_typed)
                .unwrap();
        }
        IndexMutation::WrongSelectedBinding => {}
        IndexMutation::SwapTypedBranchTypes => {
            let typed_then = source_typed_index
                .get(source_id, if_node.then_branch)
                .unwrap();
            let typed_else = source_typed_index
                .get(source_id, if_node.else_branch)
                .unwrap();
            let then_ty = typed_ast.get(typed_then).ty;
            let else_ty = typed_ast.get(typed_else).ty;
            typed_ast.get_mut(typed_then).ty = else_ty;
            typed_ast.get_mut(typed_else).ty = then_ty;
        }
        IndexMutation::SelectedTypedValDefInt => {
            let selected_typed = source_typed_index.get(source_id, block.stats[2]).unwrap();
            typed_ast.get_mut(selected_typed).ty = definitions.int;
        }
        IndexMutation::FirstLocalValDefInt | IndexMutation::SecondLocalValDefInt => {
            let local_index = usize::from(matches!(mutation, IndexMutation::SecondLocalValDefInt));
            let local_typed = source_typed_index
                .get(source_id, block.stats[local_index])
                .unwrap();
            typed_ast.get_mut(local_typed).ty = definitions.int;
        }
        IndexMutation::BlockTypeInt => {
            let typed_block = source_typed_index.get(source_id, rhs).unwrap();
            typed_ast.get_mut(typed_block).ty = definitions.int;
        }
        IndexMutation::SetterReceiverKindIf
        | IndexMutation::SetterArgumentKindIf
        | IndexMutation::PairFirstArgumentKindIf
        | IndexMutation::PairSecondArgumentKindIf
        | IndexMutation::ConditionKindIf
        | IndexMutation::ThenBranchKindIf
        | IndexMutation::ElseBranchKindIf
        | IndexMutation::FirstLocalAnnotationKindIf
        | IndexMutation::SecondLocalAnnotationKindIf
        | IndexMutation::FirstCounterNewTypeKindIf
        | IndexMutation::SecondCounterNewTypeKindIf
        | IndexMutation::SelectedAnnotationKindIf
        | IndexMutation::PairNewTypeKindIf => {
            let replacement = typed_ast.get(typed_if).kind.clone();
            let target_source = match mutation {
                IndexMutation::SetterReceiverKindIf => {
                    let TreeKind::Apply(setter) = &parsed.ast.get(block.stats[3]).kind else {
                        unreachable!()
                    };
                    let TreeKind::Select(select) = &parsed.ast.get(setter.function).kind else {
                        unreachable!()
                    };
                    select.qualifier
                }
                IndexMutation::SetterArgumentKindIf => {
                    let TreeKind::Apply(setter) = &parsed.ast.get(block.stats[3]).kind else {
                        unreachable!()
                    };
                    setter.args[0]
                }
                IndexMutation::PairFirstArgumentKindIf
                | IndexMutation::PairSecondArgumentKindIf => {
                    let TreeKind::Apply(pair) = &parsed.ast.get(block.expr).kind else {
                        unreachable!()
                    };
                    pair.args
                        [usize::from(matches!(mutation, IndexMutation::PairSecondArgumentKindIf))]
                }
                IndexMutation::ConditionKindIf => if_node.cond,
                IndexMutation::ThenBranchKindIf => if_node.then_branch,
                IndexMutation::ElseBranchKindIf => if_node.else_branch,
                IndexMutation::FirstLocalAnnotationKindIf
                | IndexMutation::SecondLocalAnnotationKindIf => {
                    let index = usize::from(matches!(
                        mutation,
                        IndexMutation::SecondLocalAnnotationKindIf
                    ));
                    let TreeKind::ValDef(definition) = &parsed.ast.get(block.stats[index]).kind
                    else {
                        unreachable!()
                    };
                    definition.tpt
                }
                IndexMutation::FirstCounterNewTypeKindIf
                | IndexMutation::SecondCounterNewTypeKindIf => {
                    let index = usize::from(matches!(
                        mutation,
                        IndexMutation::SecondCounterNewTypeKindIf
                    ));
                    let TreeKind::ValDef(definition) = &parsed.ast.get(block.stats[index]).kind
                    else {
                        unreachable!()
                    };
                    let TreeKind::Apply(application) =
                        &parsed.ast.get(definition.rhs.unwrap()).kind
                    else {
                        unreachable!()
                    };
                    let TreeKind::Select(selection) = &parsed.ast.get(application.function).kind
                    else {
                        unreachable!()
                    };
                    let TreeKind::New(new) = &parsed.ast.get(selection.qualifier).kind else {
                        unreachable!()
                    };
                    new.tpt
                }
                IndexMutation::SelectedAnnotationKindIf => selected_def.tpt,
                IndexMutation::PairNewTypeKindIf => {
                    let TreeKind::Apply(application) = &parsed.ast.get(block.expr).kind else {
                        unreachable!()
                    };
                    let TreeKind::Select(selection) = &parsed.ast.get(application.function).kind
                    else {
                        unreachable!()
                    };
                    let TreeKind::New(new) = &parsed.ast.get(selection.qualifier).kind else {
                        unreachable!()
                    };
                    new.tpt
                }
                IndexMutation::FirstLocalValDefInt
                | IndexMutation::SecondLocalValDefInt
                | IndexMutation::BlockTypeInt => unreachable!(),
                _ => unreachable!(),
            };
            let target_typed = source_typed_index.get(source_id, target_source).unwrap();
            typed_ast.get_mut(target_typed).kind = replacement;
        }
        IndexMutation::PostReceiptConditionKindIf => {}
    }
    let (
        binding,
        proof_source,
        proof_method,
        proof_condition_formal,
        proof_targets,
        proof_selected,
        proof_scope,
        proof_source_setter,
        proof_typed_setter,
        proof_source_setter_receiver,
        proof_typed_setter_receiver,
        proof_source_setter_argument,
        proof_typed_setter_argument,
        proof_setter_method,
    ) = {
        let jvm_unit = JvmCompilationUnit {
            store: typer.store(),
            typed_ast: &typed_ast,
            classes: Vec::new(),
        };
        let proof = validate_immutable_local_conditional_reference_selection_proof(
            ImmutableLocalConditionalReferenceSelectionProofInput {
                initializers: ImmutableLocalInitializerPairProofInput {
                    source_id,
                    method_tree,
                    store: typer.store(),
                    definitions: &definitions,
                    source_ast: &parsed.ast,
                    typed_ast: &typed_ast,
                    source_index: &source_index,
                    source_typed_index: &source_typed_index,
                    local_bindings: [Some(local_bindings[0]), Some(local_bindings[1])],
                    jvm_unit: &jvm_unit,
                },
                selected_binding: Some(
                    if matches!(mutation, IndexMutation::WrongSelectedBinding) {
                        local_bindings[0]
                    } else {
                        local_bindings[2]
                    },
                ),
            },
        )?;
        (
            proof.binding(),
            proof.source_id(),
            proof.method(),
            proof.condition_formal(),
            proof.targets(),
            proof.selected(),
            proof.scope(),
            proof.source_setter(),
            proof.typed_setter(),
            proof.source_setter_receiver(),
            proof.typed_setter_receiver(),
            proof.source_setter_argument(),
            proof.typed_setter_argument(),
            proof.setter_method(),
        )
    };
    assert_eq!(binding.source_id, source_id);
    assert_eq!(binding.method, method);
    assert_eq!(binding.selected, proof_selected);
    assert_eq!(binding.scope, local_bindings[2].scope());
    assert_eq!(binding.condition_formal, formals[2]);
    assert_ne!(binding.local_type, definitions.boolean);
    if matches!(mutation, IndexMutation::PostReceiptConditionKindIf) {
        let typed_if_kind = typed_ast.get(binding.typed_if).kind.clone();
        let TreeKind::If(nested_if) = typed_if_kind else {
            panic!("receipt-selected typed IF retains its source shape")
        };
        let nested_condition = typed_ast.alloc(typed_ast.get(nested_if.cond).clone());
        let nested_then = typed_ast.alloc(typed_ast.get(nested_if.then_branch).clone());
        let nested_else = typed_ast.alloc(typed_ast.get(nested_if.else_branch).clone());
        typed_ast.get_mut(binding.typed_condition).kind = TreeKind::If(If {
            cond: nested_condition,
            then_branch: nested_then,
            else_branch: nested_else,
        });
    }
    let factory_tree = match &parsed.ast.get(parsed.root).kind {
        TreeKind::PackageDef(package) => *package
            .stats
            .iter()
            .find(|tree| {
                source_index
                    .symbol_at(source_id, **tree)
                    .is_some_and(|symbol| {
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(symbol).name.text())
                            == "PairFactory"
                    })
            })
            .unwrap(),
        _ => unreachable!(),
    };
    let factory = source_index.symbol_at(source_id, factory_tree).unwrap();
    let TreeKind::TypeDef(factory_definition) = &parsed.ast.get(factory_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_definition.rhs).kind else {
        unreachable!()
    };
    let mut planned_classes = Vec::new();
    let mut planned_methods = Vec::new();
    if let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind {
        for &class_tree in &package.stats {
            let class = source_index.symbol_at(source_id, class_tree).unwrap();
            planned_classes.push(class);
            let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
                unreachable!()
            };
            let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
                unreachable!()
            };
            planned_methods.push(
                source_index
                    .symbol_at(source_id, template.constructor)
                    .unwrap(),
            );
            for &member in &template.body {
                if matches!(parsed.ast.get(member).kind, TreeKind::DefDef(_)) {
                    planned_methods.push(source_index.symbol_at(source_id, member).unwrap());
                }
            }
        }
    }
    let mut initializers = Vec::new();
    for (index, local_binding) in local_bindings[..2].iter().enumerate() {
        let source_declaration = block.stats[index];
        let TreeKind::ValDef(definition) = &parsed.ast.get(source_declaration).kind else {
            unreachable!()
        };
        let source_initializer = definition.rhs.unwrap();
        let TreeKind::Apply(source_apply) = &parsed.ast.get(source_initializer).kind else {
            unreachable!()
        };
        let source_select = source_apply.function;
        let TreeKind::Select(selection) = &parsed.ast.get(source_select).kind else {
            unreachable!()
        };
        let source_new = selection.qualifier;
        let source_argument = source_apply.args[0];
        let typed_declaration = local_binding.declaration();
        let typed_initializer = source_typed_index
            .get(source_id, source_initializer)
            .unwrap();
        let TreeKind::Apply(typed_apply) = &typed_ast.get(typed_initializer).kind else {
            unreachable!()
        };
        let typed_select = typed_apply.function;
        let typed_new = source_typed_index.get(source_id, source_new).unwrap();
        let typed_argument = typed_apply.args[0];
        let class = match typer.store().types.get(binding.local_type) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            } => *class,
            _ => unreachable!(),
        };
        let argument_formal = match typer.store().types.get(typed_ast.get(typed_argument).ty) {
            Type::TermRef {
                target: TermRefTarget::Symbol(formal),
                ..
            } => *formal,
            _ => unreachable!(),
        };
        let ctor_source_tree = match source_index.definition_of(class).unwrap() {
            dotty_core::SourceDefinition::Canonical { tree, .. } => tree,
            _ => unreachable!(),
        };
        let TreeKind::TypeDef(counter_definition) = &parsed.ast.get(ctor_source_tree).kind else {
            unreachable!()
        };
        let TreeKind::Template(counter_template) = &parsed.ast.get(counter_definition.rhs).kind
        else {
            unreachable!()
        };
        let ctor = source_index
            .symbol_at(source_id, counter_template.constructor)
            .unwrap();
        let TreeKind::DefDef(ctor_definition) = &parsed.ast.get(counter_template.constructor).kind
        else {
            unreachable!()
        };
        let ctor_formal_tree = ctor_definition.value_param_clauses[0][0];
        let ctor_formal = source_index
            .derived_symbol_at(ctor, source_id, ctor_formal_tree)
            .unwrap();
        let formal_type = match typer.store().symbols.get(ctor_formal).info {
            SymbolInfo::Complete(ty) => ty,
            _ => unreachable!(),
        };
        let local_type = match typer.store().symbols.get(local_binding.symbol()).info {
            SymbolInfo::Complete(ty) => ty,
            _ => unreachable!(),
        };
        initializers.push(LocalCounterInitializerBinding {
            source_block: rhs,
            typed_block: source_typed_index.get(source_id, rhs).unwrap(),
            source_declaration,
            typed_declaration,
            source_initializer,
            typed_initializer,
            source_type_tree: definition.tpt,
            typed_type_tree: source_typed_index.get(source_id, definition.tpt).unwrap(),
            local: local_binding.symbol(),
            scope: local_binding.scope(),
            local_type,
            argument_formal,
            call: NestedConstructorCallBinding {
                source_apply: source_initializer,
                typed_apply: typed_initializer,
                source_select,
                source_new,
                source_argument,
                typed_select,
                typed_new,
                typed_argument,
                class,
                constructor: ctor,
                formal: ctor_formal,
                formal_type,
                result_type: typed_ast.get(typed_initializer).ty,
            },
        });
    }
    let source_formals: [dotty_core::TreeId<Untyped>; 2] =
        method_def.value_param_clauses[0][..2].try_into().unwrap();
    let pair = match &typed_ast
        .get(source_typed_index.get(source_id, block.expr).unwrap())
        .kind
    {
        TreeKind::Apply(application) => FormalLocalConstructorCallPairBinding {
            source_method_formals: source_formals,
            method_formals: [formals[0], formals[1]],
            arguments: std::array::from_fn(|index| {
                let source_argument = match &parsed.ast.get(block.expr).kind {
                    TreeKind::Apply(apply) => apply.args[index],
                    _ => unreachable!(),
                };
                let typed_argument = application.args[index];
                let ty = typed_ast.get(typed_argument).ty;
                let symbol = match typer.store().types.get(ty) {
                    Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    } => *symbol,
                    _ => unreachable!(),
                };
                FormalLocalConstructorCallArgumentBinding {
                    source_argument,
                    typed_argument,
                    origin: FormalLocalConstructorCallArgumentOrigin::LocalValue {
                        symbol,
                        result_type: ty,
                    },
                }
            }),
        },
        _ => unreachable!(),
    };
    let bindings = ConditionalPicklerBindings {
        index: &source_typed_index,
        source: source_id,
        method,
        classes: planned_classes,
        methods: planned_methods,
        locals: local_bindings
            .iter()
            .enumerate()
            .map(|(index, local)| LocalValueBinding {
                source_tree: block.stats[index],
                symbol: local.symbol(),
                typed_declaration: local.declaration(),
                scope: local.scope(),
            })
            .collect(),
        initializers,
        selection: binding,
        setter: LocalReceiverUnitCallBinding {
            source_apply: proof_source_setter,
            typed_apply: proof_typed_setter,
            source_select: match &parsed.ast.get(proof_source_setter).kind {
                TreeKind::Apply(apply) => apply.function,
                _ => unreachable!(),
            },
            typed_select: match &typed_ast.get(proof_typed_setter).kind {
                TreeKind::Apply(apply) => apply.function,
                _ => unreachable!(),
            },
            source_receiver: proof_source_setter_receiver,
            typed_receiver: proof_typed_setter_receiver,
            source_argument: proof_source_setter_argument,
            typed_argument: proof_typed_setter_argument,
            receiver: proof_selected,
            receiver_scope: proof_scope,
            method: proof_setter_method,
            argument_formal: formals[3],
        },
        pair,
    };
    let constructor = source_index
        .symbol_at(source_id, factory_template.constructor)
        .unwrap();
    let factory_constructor = PrimaryConstructorInputPlan {
        symbol: constructor,
        source_tree: factory_template.constructor,
        parameter_clauses: vec![],
    };
    let member = [ClassMemberPicklePlan::Method {
        symbol: method,
        source_tree: method_tree,
    }];
    let class_plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: &typed_ast,
        source_index: &source_index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source_id,
            name: conditional_reference_artifacts::PRODUCER_SOURCE_NAME.into(),
        },
        class: factory,
        class_tree: factory_tree,
        constructor: factory_constructor,
        members: &member,
    };
    let validated = class_plan
        .validate()
        .expect("semantic-only PairFactory plan accepts the authenticated receipt");
    if matches!(mutation, IndexMutation::PostReceiptConditionKindIf) {
        let condition_type = typed_ast.get(binding.typed_condition).ty;
        let expected = PickleError::BodyType {
            tree: binding.typed_condition,
            ty: condition_type,
        };
        assert_eq!(
            validated.validate_method_bodies(),
            Err(expected.clone()),
            "post-receipt nested IF fails at the typed condition-kind check"
        );
        match validated.pickle() {
            Err(error) => assert_eq!(
                error, expected,
                "pickle rejects the corrupted typed condition before producing output"
            ),
            Ok(completed) => panic!(
                "corrupted typed condition escaped as {} bytes of completed PickledTasty",
                completed.bytes().len()
            ),
        }
        return Ok(ReceiptIds {
            source: proof_source,
            method: proof_method,
            condition_formal: proof_condition_formal,
            targets: proof_targets,
            selected: proof_selected,
            scope: proof_scope,
            tasty: Vec::new(),
        });
    }
    validated
        .validate_method_bodies()
        .expect("conditional body validates");
    let limited = validated.pickle_with_limits(PickleLimits {
        max_file_bytes: 1,
        ..PickleLimits::default()
    });
    assert!(
        matches!(
            limited,
            Err(PickleError::LimitExceeded {
                kind: LimitKind::FileBytes,
                limit: 1,
            })
        ),
        "file-size limit returns no completed PickledTasty: {limited:?}"
    );
    let tasty = validated
        .pickle()
        .expect("conditional body serializes")
        .bytes()
        .to_vec();
    Ok(ReceiptIds {
        source: proof_source,
        method: proof_method,
        condition_formal: proof_condition_formal,
        targets: proof_targets,
        selected: proof_selected,
        scope: proof_scope,
        tasty,
    })
}

#[test]
fn authentic_conditional_local_receipt_preserves_session_identities_for_source_controls() {
    let original = authenticate(SOURCE, 22_650, IndexMutation::None).unwrap();
    assert!(!original.tasty.is_empty());
    assert_rust_candidate_if_shape(
        &original.tasty,
        ["first", "second"],
        "selected",
        ["first", "second"],
        ["first", "second"],
    );
    let _corrupted = corrupt_shared_branch_address(&original.tasty);
    verify_rust_candidate_with_pinned_scala(&original.tasty, "selected");
    let repeated = authenticate(SOURCE, 22_650, IndexMutation::None).unwrap();
    assert_eq!(
        original.tasty, repeated.tasty,
        "one session emits deterministic candidate bytes"
    );
    assert_eq!(original.source, SourceId::from_index(22_650));
    assert_eq!(original.method.index(), 60);
    assert_eq!(original.condition_formal.index(), 63);
    assert_eq!(original.targets.map(dotty_core::SymbolId::index), [65, 66]);
    assert_eq!(original.selected.index(), 67);
    assert_eq!(original.scope.index(), 18);
    assert_ne!(original.targets[0], original.targets[1]);
    assert!(!original.targets.contains(&original.selected));

    let renamed = SOURCE
        .replace("    val first: Counter", "    val leftLocal: Counter")
        .replace("    val second: Counter", "    val rightLocal: Counter")
        .replace("    val selected: Counter", "    val chosen: Counter")
        .replace("then first else second", "then leftLocal else rightLocal")
        .replace("selected.set", "chosen.set")
        .replace("new Pair(first, second)", "new Pair(leftLocal, rightLocal)");
    let renamed_receipt = authenticate(&renamed, 22_671, IndexMutation::None).unwrap();
    assert_eq!(renamed_receipt.source, SourceId::from_index(22_671));
    assert_ne!(renamed_receipt.targets[0], renamed_receipt.targets[1]);
    assert!(!renamed_receipt.targets.contains(&renamed_receipt.selected));
    assert_rust_candidate_if_shape(
        &renamed_receipt.tasty,
        ["leftLocal", "rightLocal"],
        "chosen",
        ["leftLocal", "rightLocal"],
        ["leftLocal", "rightLocal"],
    );
    verify_rust_candidate_with_pinned_scala(&renamed_receipt.tasty, "chosen");

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed_receipt = authenticate(&reversed, 22_672, IndexMutation::None).unwrap();
    assert_eq!(
        reversed_receipt.targets,
        [original.targets[1], original.targets[0]]
    );
    assert_rust_candidate_if_shape(
        &reversed_receipt.tasty,
        ["second", "first"],
        "selected",
        ["first", "second"],
        ["first", "second"],
    );
    verify_rust_candidate_with_pinned_scala(&reversed_receipt.tasty, "selected");

    let swapped = SOURCE.replace("then first else second", "then second else first");
    authenticate(&swapped, 22_673, IndexMutation::None).unwrap();
}

#[test]
fn post_receipt_typed_condition_corruption_rejects_without_completed_tasty() {
    let corrupted = authenticate(SOURCE, 22_679, IndexMutation::PostReceiptConditionKindIf)
        .expect("test helper asserts the first typed error and output absence");
    assert!(corrupted.tasty.is_empty());
}

#[test]
fn missing_or_forged_source_typed_if_edges_reject_at_the_typed_binding_stage() {
    let missing = authenticate(SOURCE, 22_674, IndexMutation::MissingCondition);
    assert!(
        matches!(
            missing,
            Err(CrossClassConstructorCallProofError::TypedBinding(_))
        ),
        "unexpected failure: {missing:?}"
    );
    assert!(matches!(
        authenticate(SOURCE, 22_675, IndexMutation::SwappedBranches),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_676, IndexMutation::ConditionAsIntFormal),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_677, IndexMutation::BranchAsIntFormal),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_678, IndexMutation::WrongSetterReceiver),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_679, IndexMutation::WrongPairArgument),
        Err(CrossClassConstructorCallProofError::TypedBinding(_))
    ));
    assert!(matches!(
        authenticate(SOURCE, 22_680, IndexMutation::WrongSelectedBinding),
        Err(CrossClassConstructorCallProofError::SourceIdentity(_))
    ));
    for mutation in [
        IndexMutation::SwapTypedBranchTypes,
        IndexMutation::SelectedTypedValDefInt,
        IndexMutation::SetterReceiverKindIf,
        IndexMutation::SetterArgumentKindIf,
        IndexMutation::PairFirstArgumentKindIf,
        IndexMutation::PairSecondArgumentKindIf,
        IndexMutation::ConditionKindIf,
        IndexMutation::ThenBranchKindIf,
        IndexMutation::ElseBranchKindIf,
        IndexMutation::FirstLocalValDefInt,
        IndexMutation::SecondLocalValDefInt,
        IndexMutation::BlockTypeInt,
        IndexMutation::FirstLocalAnnotationKindIf,
        IndexMutation::SecondLocalAnnotationKindIf,
        IndexMutation::FirstCounterNewTypeKindIf,
        IndexMutation::SecondCounterNewTypeKindIf,
        IndexMutation::SelectedAnnotationKindIf,
        IndexMutation::PairNewTypeKindIf,
    ] {
        assert!(
            matches!(
                authenticate(SOURCE, 22_698, mutation),
                Err(CrossClassConstructorCallProofError::TypedBinding(_))
            ),
            "forged typed arena mutation must fail closed: {mutation:?}"
        );
    }
}

#[test]
fn conditional_shape_reaches_the_authenticated_paired_producer_plan() {
    let mut callback_reached = false;
    let result = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        conditional_reference_artifacts::PRODUCER_SOURCE_NAME,
        SourceId::from_index(22_681),
        |_| callback_reached = true,
    );
    result.expect("authenticated conditional source reaches producer planning");
    assert!(callback_reached, "accepted source exposes its paired plan");
}

#[test]
fn frontend_invalid_condition_branch_and_result_inputs_fail_during_typing() {
    let controls = [
        SOURCE.replace("if chooseFirst then", "if firstSeed then"),
        SOURCE.replace("then first else second", "then firstSeed else second"),
        SOURCE.replace("then first else second", "then missingLocal else second"),
        SOURCE.replace("val selected: Counter", "val selected: Int"),
    ];
    let expected_failures = [
        "IfConditionTypeMismatch",
        "LocalValueTypeMismatch",
        "TermNameNotFound",
        "LocalValueTypeMismatch",
    ];
    for (index, (source, expected)) in controls.iter().zip(expected_failures).enumerate() {
        let mut callback_reached = false;
        let result = with_paired_jvm_tasty_batch_plan(
            source,
            "conditional-reference-invalid-control.scala",
            SourceId::from_index(22_690 + index as u32),
            |_| callback_reached = true,
        );
        assert!(
            matches!(
                result,
                Err(PairedPlanError::BatchClass {
                    phase: "method body typing",
                    ref reason,
                    ..
                }) if reason.contains(expected)
            ),
            "unexpected first failure for invalid control {index}: {result:?}"
        );
        assert!(!callback_reached);
    }
}

#[test]
fn mutable_or_nested_selections_are_outside_the_receipt_contract() {
    let mutable = SOURCE.replace("val selected: Counter", "var selected: Counter");
    assert!(matches!(
        authenticate(&mutable, 22_695, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceIdentity(_))
    ));

    let nested = SOURCE.replace(
        "if chooseFirst then first else second",
        "if chooseFirst then (if chooseFirst then first else second) else second",
    );
    assert!(matches!(
        authenticate(&nested, 22_696, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceShape(_))
    ));

    let allocation = SOURCE.replace(
        "if chooseFirst then first else second",
        "if chooseFirst then new Counter(firstSeed) else second",
    );
    assert!(matches!(
        authenticate(&allocation, 22_697, IndexMutation::None),
        Err(CrossClassConstructorCallProofError::SourceShape(_))
    ));
}
