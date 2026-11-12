use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, ImmutableLocalConditionalReferenceSelectionProofInput,
    ImmutableLocalInitializerPairProofInput,
    validate_immutable_local_conditional_reference_selection_proof,
};
use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_batch_plan};
use dotty_core::ast::{AstArena, TreeKind, Typed};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolId,
    SymbolInfo, TermRefTarget, TreeId, Type, TypeRefTarget, Untyped,
};
use dotty_jvm::JvmCompilationUnit;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::ast::{
    APPLY_TAG, BLOCK_TAG, DEFDEF_TAG, DefinitionBody, IF_TAG, SHAREDTYPE_TAG, TERMREFDIRECT_TAG,
    VALDEF_TAG,
};
use dotty_tasty::file::TastyFile;
use dotty_tasty::term::RawTree;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, ConditionalLocalReferenceSelectionBinding,
    FormalLocalConstructorCallArgumentBinding, FormalLocalConstructorCallArgumentOrigin,
    FormalLocalConstructorCallPairBinding, LocalCounterInitializerBinding,
    LocalReceiverUnitCallBinding, LocalValueBinding, NestedConstructorCallBinding,
    PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings,
};
use dotty_typer::{SourceTyper, TypedLocalBinding};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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

const SOURCE: &str = r#"package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    val selected: Counter =
      if chooseFirst then first else second
    selected.set(nextValue)
    new Pair(first, second)
"#;

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

fn assert_rust_candidate_if_shape(bytes: &[u8]) {
    let tasty = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let index = tasty.ast_address_index().unwrap();
    let create = index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index.get(node.offset as u32)?.decode_defdef_body().ok()?;
            (tasty.names().render(body.name).ok()? == "create").then_some(body)
        })
        .expect("Rust candidate contains PairFactory.create");
    let RawTree::LengthNode(block) = create.rhs.unwrap() else {
        panic!("create body is BLOCK")
    };
    assert_eq!(block.tag, BLOCK_TAG);
    let block = block.decode_block().unwrap();
    assert_eq!(block.stats.len(), 4);
    assert_eq!(
        block.stats.iter().map(raw_tag).collect::<Vec<_>>(),
        [VALDEF_TAG, VALDEF_TAG, VALDEF_TAG, APPLY_TAG]
    );
    let selected = index
        .iter_nodes_with_tag(VALDEF_TAG)
        .find_map(|node| {
            let DefinitionBody::ValDef { name, rhs, .. } = index
                .get(node.offset as u32)?
                .decode_definition_body()
                .ok()?
            else {
                return None;
            };
            (tasty.names().render(name).ok()? == "selected").then_some(rhs?)
        })
        .expect("selected VALDEF has an initializer");
    let RawTree::LengthNode(if_node) = selected else {
        panic!("selected initializer is bare IF, without TYPED")
    };
    assert_eq!(if_node.tag, IF_TAG);
    let if_node = if_node.decode_if().unwrap();
    assert_eq!(raw_tag(&if_node.condition), TERMREFDIRECT_TAG);
    assert_eq!(raw_tag(&if_node.then_branch), SHAREDTYPE_TAG);
    assert_eq!(raw_tag(&if_node.else_branch), SHAREDTYPE_TAG);
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

fn verify_rust_candidate_with_pinned_scala(candidate: &[u8]) {
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
        printed.contains("[selected]"),
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
        "conditional-reference-selection.scala",
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
    }
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: &typed_ast,
        classes: Vec::new(),
    };
    let proof_result = validate_immutable_local_conditional_reference_selection_proof(
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
            selected_binding: Some(if matches!(mutation, IndexMutation::WrongSelectedBinding) {
                local_bindings[0]
            } else {
                local_bindings[2]
            }),
        },
    );
    let proof = proof_result?;
    let binding = proof.binding();
    assert_eq!(binding.source_id, source_id);
    assert_eq!(binding.method, method);
    assert_eq!(binding.selected, proof.selected());
    assert_eq!(binding.scope, local_bindings[2].scope());
    assert_eq!(binding.condition_formal, formals[2]);
    assert_ne!(binding.local_type, definitions.boolean);
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
            source_apply: proof.source_setter(),
            typed_apply: proof.typed_setter(),
            source_select: match &parsed.ast.get(proof.source_setter()).kind {
                TreeKind::Apply(apply) => apply.function,
                _ => unreachable!(),
            },
            typed_select: match &typed_ast.get(proof.typed_setter()).kind {
                TreeKind::Apply(apply) => apply.function,
                _ => unreachable!(),
            },
            source_receiver: proof.source_setter_receiver(),
            typed_receiver: proof.typed_setter_receiver(),
            source_argument: proof.source_setter_argument(),
            typed_argument: proof.typed_setter_argument(),
            receiver: proof.selected(),
            receiver_scope: proof.scope(),
            method: proof.setter_method(),
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
            name: "conditional-reference-selection.scala".into(),
        },
        class: factory,
        class_tree: factory_tree,
        constructor: factory_constructor,
        members: &member,
    };
    let validated = class_plan
        .validate()
        .expect("semantic-only PairFactory plan accepts the authenticated receipt");
    validated
        .validate_method_bodies()
        .expect("conditional body validates");
    let tasty = validated
        .pickle()
        .expect("conditional body serializes")
        .bytes()
        .to_vec();
    Ok(ReceiptIds {
        source: proof.source_id(),
        method: proof.method(),
        condition_formal: proof.condition_formal(),
        targets: proof.targets(),
        selected: proof.selected(),
        scope: proof.scope(),
        tasty,
    })
}

#[test]
fn authentic_conditional_local_receipt_preserves_session_identities_for_source_controls() {
    let original = authenticate(SOURCE, 22_650, IndexMutation::None).unwrap();
    assert!(!original.tasty.is_empty());
    assert_rust_candidate_if_shape(&original.tasty);
    verify_rust_candidate_with_pinned_scala(&original.tasty);
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

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed_receipt = authenticate(&reversed, 22_672, IndexMutation::None).unwrap();
    assert_eq!(
        reversed_receipt.targets,
        [original.targets[1], original.targets[0]]
    );

    let swapped = SOURCE.replace("then first else second", "then second else first");
    authenticate(&swapped, 22_673, IndexMutation::None).unwrap();
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
fn conditional_shape_keeps_the_original_four_formal_producer_rejection_before_callback() {
    let mut callback_reached = false;
    let result = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "conditional-reference-selection.scala",
        SourceId::from_index(22_681),
        |_| callback_reached = true,
    );
    assert!(
        matches!(
            result,
            Err(PairedPlanError::BatchUnit {
                phase: "cross-class constructor-call identity proof",
                ref reason,
            }) if reason.contains("SourceShape")
                && reason.contains("candidate method must have one plain formal clause")
        ),
        "unexpected producer failure: {result:?}"
    );
    assert!(
        !callback_reached,
        "the rejected source must return no callback plan"
    );
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
