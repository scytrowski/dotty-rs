//! Smoke coverage of the type pass over real compiler output, and the
//! measurement that scopes the next increment.
//!
//! The small-fixture test runs in CI. The corpus measurement is `#[ignore]`d
//! because it walks every unit of the two Scala 3 corpora; run it with
//! `cargo test -p dotty-tasty-unpickler --release --test type_corpus -- --ignored --nocapture`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{TastyFile, is_compact_annot_type_tag};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

/// Every node tag that is a type reference, or a `THIS` prefix.
const REFERENCE_TAGS: [u8; 10] = [61, 62, 63, 64, 65, 90, 114, 115, 116, 117];

/// The compound forms decoded since Milestone 2c1, with the names used in
/// the report. `SUPERtype` does not occur in either corpus.
const COMPOUND_TAGS: [(u8, &str); 5] = [
    (161, "APPLIEDtype"),
    (165, "ANDtype"),
    (167, "ORtype"),
    (158, "SUPERtype"),
    (93, "BYNAMEtype"),
];

const TYPEBOUNDS_TAG: u8 = 163;
const FLEXIBLETYPE_TAG: u8 = 193;
const CLASSCONST_TAG: u8 = 92;

/// The binder forms decoded since Milestones 3a and 3b.
const BINDER_TAGS: [(u8, &str); 4] = [
    (170, "TYPELAMBDAtype"),
    (169, "POLYtype"),
    (180, "METHODtype"),
    (172, "PARAMtype"),
];

/// The refined and recursive forms decoded since Milestone 4a.
const RECURSIVE_TAGS: [(u8, &str); 3] = [(159, "REFINEDtype"), (100, "RECtype"), (66, "RECthis")];

/// The bounds and flexible forms decoded since Milestone 2c2.
const WRAPPER_TAGS: [(u8, &str); 2] = [
    (TYPEBOUNDS_TAG, "TYPEBOUNDS"),
    (FLEXIBLETYPE_TAG, "FLEXIBLEtype"),
];

/// The constant nodes decoded since Milestone 2c2. A constant type and a
/// literal term are the same node, so these count every occurrence.
const CONSTANT_TAGS: [(u8, &str); 13] = [
    (2, "UNITconst"),
    (3, "FALSEconst"),
    (4, "TRUEconst"),
    (5, "NULLconst"),
    (67, "BYTEconst"),
    (68, "SHORTconst"),
    (69, "CHARconst"),
    (70, "INTconst"),
    (71, "LONGconst"),
    (72, "FLOATconst"),
    (73, "DOUBLEconst"),
    (74, "STRINGconst"),
    (CLASSCONST_TAG, "CLASSconst"),
];

/// The annotated type, decoded since Milestone 4b1 (compact annotations only).
const ANNOTATED_TAG: u8 = 153;
const ANNOTATED_TAGS: [(u8, &str); 1] = [(ANNOTATED_TAG, "ANNOTATEDtype")];
const TYPEREFIN_TAG: u8 = 175;
const TERMREFIN_TAG: u8 = 174;

/// Scala 3.9's compact annotation tags (`isCompactAnnotTypeTag`).
const COMPACT_HEADS: [(u8, &str); 6] = [
    (161, "APPLIEDtype"),
    (61, "SHAREDtype"),
    (117, "TYPEREF"),
    (63, "TYPEREFdirect"),
    (116, "TYPEREFsymbol"),
    (TYPEREFIN_TAG, "TYPEREFin"),
];

/// Whether `tag` is a form measured node by node (compound, bounds, flexible or
/// constant) rather than as a reference.
fn is_measured(tag: u8) -> bool {
    COMPOUND_TAGS.iter().any(|(t, _)| *t == tag)
        || WRAPPER_TAGS.iter().any(|(t, _)| *t == tag)
        || BINDER_TAGS.iter().any(|(t, _)| *t == tag)
        || RECURSIVE_TAGS.iter().any(|(t, _)| *t == tag)
        || ANNOTATED_TAGS.iter().any(|(t, _)| *t == tag)
        || CONSTANT_TAGS.iter().any(|(t, _)| *t == tag)
}

fn tasty_files(root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "tasty") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// How the reference nodes of one kind fared.
#[derive(Default)]
struct Outcomes {
    nodes: usize,
    decoded: usize,
    /// Well-formed references whose target lives outside the entered state, so
    /// only an external resolver (a classpath) can supply it.
    needs_external: usize,
    ambiguous: usize,
    signed: usize,
    unsupported_prefix: usize,
    /// References to definitions pass 1 does not enter (locals).
    missing_local: usize,
    /// A child of a compound node is a form with no decoder yet.
    unsupported_child: usize,
    /// A full annotation the pass defers under the node: a tree that is not a
    /// constructor call (`SHAREDterm`), or a constructor part or argument it
    /// does not read (Milestone 4b2a).
    deferred_annotation: usize,
    /// A binder reference or parameter that is malformed: an invalid binder
    /// address, kind, parameter index, or a parameter info that is not bounds.
    binder_errors: usize,
    /// `PARAMtype` roots whose binder had no type when they were asked for, so
    /// the binder was decoded on demand.
    binder_on_demand: usize,
    /// Errors that mean a bug or malformed input, never an expected gap.
    unexpected: usize,
}

/// How the `TYPEBOUNDS` nodes are shaped on the wire.
#[derive(Default)]
struct BoundsShapes {
    total: usize,
    /// `low` and `high`.
    two_sided: usize,
    /// One type only: the alias form.
    alias_only: usize,
    /// Any variance marker after the types.
    with_variance: usize,
    /// Decoded, by form.
    decoded_alias: usize,
    decoded_two_sided: usize,
    /// Not decodable structurally.
    malformed: usize,
}

/// How the variance-bearing `TYPEBOUNDS` nodes fared (Milestone 3c), and how
/// the lambdas decoded split between standalone ones and ones that are the
/// source of a variance application.
#[derive(Default)]
struct VarianceBounds {
    total: usize,
    decoded: usize,
    /// Failures, by kind.
    failures: BTreeMap<&'static str, usize>,
    lambdas_standalone: usize,
    lambdas_as_variance_source: usize,
}

/// Why a variance-bearing `TYPEBOUNDS` failed, for the report.
fn variance_failure(error: &UnpickleError) -> &'static str {
    match error {
        UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. } => {
            "external child"
        }
        UnpickleError::MissingReferencedSymbol { .. } => "local missing child",
        UnpickleError::UnsupportedType { .. } => "unsupported child form",
        UnpickleError::BoundsVarianceTargetPending { .. } => "pending variance target",
        UnpickleError::BoundsVarianceArityMismatch { .. } => "arity mismatch",
        UnpickleError::RebindFailed { .. } => "rebind failure",
        UnpickleError::AmbiguousMember { .. }
        | UnpickleError::UnsupportedSignedReference { .. }
        | UnpickleError::UnsupportedResolutionPrefix { .. } => {
            "other known (ambiguous/signed/prefix)"
        }
        _ => "unexpected",
    }
}

/// How the compact `ANNOTATEDtype` nodes fared (Milestone 4b1).
#[derive(Default)]
struct CompactOutcomes {
    total: usize,
    decoded: usize,
    external: usize,
    local_missing: usize,
    unsupported_child: usize,
    /// `TYPEREFin`/`TERMREFin` anywhere under the node (Milestone 4c).
    typerefin_deferred: usize,
    /// The annotation itself is a `TYPEREFin` that is not decoded yet.
    typerefin_head_deferred: usize,
    unsupported_prefix: usize,
    invalid_compact_type: usize,
    malformed: usize,
    other_known: usize,
    /// A full annotation tree met below a compact node: in its parent.
    parent_full_tree: usize,
}

/// How the full-tree `ANNOTATEDtype` nodes with one root tag fared
/// (Milestone 4b2a: `APPLY` and `NEW` are decoded, anything else is deferred).
#[derive(Default)]
struct FullOutcomes {
    total: usize,
    decoded: usize,
    /// Failed before the annotation boundary, on the parent.
    parent_failed: usize,
    /// The annotation's own type (or a class literal argument) is outside the
    /// entered state.
    external: usize,
    local_missing: usize,
    constructor_unsupported: usize,
    argument_unsupported: usize,
    invalid_type: usize,
    malformed: usize,
    other_known: usize,
    /// Refused as a tree whose root is not `APPLY`/`NEW` (`SHAREDterm`, ...).
    deferred_tree: usize,
}

/// The shapes of the full `APPLY`/`NEW` annotations, surveyed from the wire.
#[derive(Default)]
struct FullShapes {
    /// The constructor spine, arguments left out.
    spines: BTreeMap<String, usize>,
    argument_counts: BTreeMap<usize, usize>,
    /// Root tag of each term argument, a `NAMEDARG` looked through.
    argument_roots: BTreeMap<u8, usize>,
    named_arguments: usize,
    /// Every tag found anywhere below an annotation root (links not followed).
    descendant_tags: BTreeMap<u8, usize>,
}

/// Erased method parameters (Milestone 4b2a).
#[derive(Default)]
struct ErasedStats {
    method_roots: usize,
    /// `METHODtype` roots with a parameter type that is an `ANNOTATEDtype`
    /// whose annotation is a `new ErasedParam`.
    containing: usize,
    decoded_containing: usize,
    /// `MethodParam`s with `erased` set, over every decoded `METHODtype`.
    erased_params: usize,
}

/// The `ANNOTATEDtype` survey: wire shapes, then decoding outcomes.
#[derive(Default)]
struct AnnotationSurvey {
    total: usize,
    compact_heads: BTreeMap<u8, usize>,
    full_roots: BTreeMap<u8, usize>,
    compact: CompactOutcomes,
    full: BTreeMap<u8, FullOutcomes>,
    shapes: FullShapes,
    erased: ErasedStats,
}

/// `TYPEREFin` / `TERMREFin` roots (Milestone 4c1), classified by why they did
/// not decode: probing the prefix (child 0) and the owner space (child 1) on
/// their own first says which one failed.
#[derive(Default)]
struct InReferenceOutcomes {
    nodes: usize,
    decoded: usize,
    /// The prefix does not decode; by the kind of failure.
    prefix_failure: BTreeMap<&'static str, usize>,
    /// The owner space is outside the entered state (a classpath's).
    space_external: usize,
    /// The owner space names a definition pass 1 did not enter.
    space_local_missing: usize,
    /// The owner space has no declaration-scope semantics, or fails otherwise.
    space_unsupported: usize,
    ambiguous: usize,
    signed: usize,
    /// Both children decode and the declaration is in no scope the state has.
    resolver_unresolved: usize,
    /// A resolver answer that is malformed (wrong namespace or owner).
    resolver_malformed: usize,
    /// The prefix is an unstable singleton: Dotty wraps it in a
    /// `QualSkolemType`, which the model does not have.
    illegal_prefix: usize,
    other_child: usize,
    unexpected: usize,
    /// Wire shapes: the prefix's and the owner space's root tags, a
    /// `SHAREDtype` followed to its target.
    prefix_shapes: BTreeMap<String, usize>,
    space_shapes: BTreeMap<String, usize>,
    /// Both children decoded, whatever the root did.
    children_decoded: usize,
}

/// The name of a type tag in the shape survey.
fn shape_name(tag: u8) -> String {
    match tag {
        61 => "SHAREDtype".to_owned(),
        62 => "TERMREFdirect".to_owned(),
        63 => "TYPEREFdirect".to_owned(),
        64 => "TERMREFpkg".to_owned(),
        65 => "TYPEREFpkg".to_owned(),
        66 => "RECthis".to_owned(),
        90 => "THIS".to_owned(),
        114 => "TERMREFsymbol".to_owned(),
        115 => "TERMREF".to_owned(),
        116 => "TYPEREFsymbol".to_owned(),
        117 => "TYPEREF".to_owned(),
        161 => "APPLIEDtype".to_owned(),
        174 => "TERMREFin".to_owned(),
        175 => "TYPEREFin".to_owned(),
        193 => "FLEXIBLEtype".to_owned(),
        other => format!("tag {other}"),
    }
}

/// The root tag of the type at `at`, a chain of `SHAREDtype` links followed
/// (a bounded number of times), and the number of links followed.
fn shape_of(file: &TastyFile<'_>, tags: &HashMap<u32, u8>, mut at: u32) -> String {
    let mut links = 0;
    while tags.get(&at) == Some(&61) && links < 16 {
        let Some(payload) = file.section(dotty_tasty::tasty::StandardSection::Asts) else {
            break;
        };
        let mut value: u32 = 0;
        let mut target = None;
        for byte in payload.payload[usize::try_from(at).unwrap() + 1..].iter() {
            value = value.wrapping_mul(128) | u32::from(byte & 0x7f);
            if byte & 0x80 != 0 {
                target = Some(value);
                break;
            }
        }
        match target {
            Some(target) => at = target,
            None => break,
        }
        links += 1;
    }
    let name = tags
        .get(&at)
        .map_or_else(|| "?".to_owned(), |t| shape_name(*t));
    if links == 0 {
        name
    } else {
        format!("SHAREDtype -> {name}")
    }
}

/// How a failed child probe is filed.
fn child_failure_kind(error: &UnpickleError) -> &'static str {
    match error {
        UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. } => {
            "external"
        }
        UnpickleError::MissingReferencedSymbol { .. } => "local definition not entered",
        UnpickleError::UnsupportedType { .. } => "unsupported form",
        UnpickleError::UnsupportedResolutionPrefix { .. }
        | UnpickleError::UnsupportedResolutionSpace { .. } => "unsupported prefix or space",
        UnpickleError::UnsupportedSignedReference { .. } => "signed",
        UnpickleError::AmbiguousMember { .. } => "ambiguous",
        UnpickleError::UnsupportedAnnotationTree { .. }
        | UnpickleError::UnsupportedAnnotationConstructor { .. }
        | UnpickleError::UnsupportedAnnotationArgument { .. } => "deferred annotation",
        _ => "other",
    }
}

/// Files one `TYPEREFin` / `TERMREFin` root. `prefix` and `space` are what the
/// two children decoded to on their own, `result` what the root did.
fn record_in_reference(
    outcomes: &mut InReferenceOutcomes,
    prefix: &Result<dotty_core::ids::TypeId, UnpickleError>,
    space: &Result<dotty_core::ids::TypeId, UnpickleError>,
    result: &Result<dotty_core::ids::TypeId, UnpickleError>,
) {
    outcomes.nodes += 1;
    outcomes.children_decoded += usize::from(prefix.is_ok() && space.is_ok());
    if let Err(error) = prefix {
        *outcomes
            .prefix_failure
            .entry(child_failure_kind(error))
            .or_default() += 1;
        return;
    }
    if let Err(error) = space {
        match child_failure_kind(error) {
            "external" => outcomes.space_external += 1,
            "local definition not entered" => outcomes.space_local_missing += 1,
            "unsupported form" | "unsupported prefix or space" => outcomes.space_unsupported += 1,
            _ => outcomes.other_child += 1,
        }
        return;
    }
    match result {
        Ok(_) => outcomes.decoded += 1,
        Err(UnpickleError::UnresolvedMember { .. }) => outcomes.resolver_unresolved += 1,
        Err(UnpickleError::AmbiguousMember { .. }) => outcomes.ambiguous += 1,
        Err(UnpickleError::UnsupportedSignedReference { .. }) => outcomes.signed += 1,
        Err(UnpickleError::UnsupportedResolutionSpace { .. }) => outcomes.space_unsupported += 1,
        Err(UnpickleError::ResolverFailure { .. }) => outcomes.resolver_malformed += 1,
        Err(UnpickleError::IllegalTypePrefix { .. }) => outcomes.illegal_prefix += 1,
        Err(_) => outcomes.unexpected += 1,
    }
}

/// What an owner-space class's own declarations hold under a `REFin` name,
/// measured across every unit of a corpus after all of them are entered. The
/// lookup itself cannot see it: a class's scope belongs to the unit that
/// entered it, until symbols are completed (Milestone 5).
#[derive(Default)]
struct OwnerOracle {
    /// The owner space is not a class some unit of the corpus entered.
    no_scope: usize,
    /// The declaration name is not text.
    name_not_text: usize,
    none: usize,
    one: usize,
    several: usize,
}

#[derive(Default)]
struct Tally {
    /// Every class scope entered so far, by the class symbol.
    class_scopes: HashMap<dotty_core::ids::SymbolId, dotty_core::ids::ScopeId>,
    /// `REFin` nodes whose two children decoded: the owner space, the
    /// declaration name and its namespace, for the [`OwnerOracle`].
    oracle_queries: Vec<(dotty_core::ids::TypeId, Option<String>, Namespace)>,
    /// `TYPEREFin` (175) and `TERMREFin` (174).
    in_references: BTreeMap<u8, InReferenceOutcomes>,
    annotations: AnnotationSurvey,
    variance: VarianceBounds,
    units: usize,
    units_with_a_decoded_type: usize,
    units_fully_decoded: usize,
    /// References written by definition address (`*direct`, `*symbol`, `*pkg`,
    /// `THIS`, `SHAREDtype`).
    by_address: Outcomes,
    /// Name-based `TYPEREF` and `TERMREF`.
    named_type: Outcomes,
    named_term: Outcomes,
    /// Per measured tag: compound, bounds, flexible and constant nodes.
    compound: BTreeMap<u8, Outcomes>,
    bounds: BoundsShapes,
    /// Decoded `PARAMtype` roots, by the kind of node their binder address
    /// names (`TYPELAMBDAtype`, `POLYtype`, `METHODtype`, or another node).
    param_binders: BTreeMap<&'static str, usize>,
    /// Distinct `RECtype` binders that a decoded `RECthis` root names, and the
    /// distinct canonical `RecThis` types they got (Milestone 4a).
    rec_binders: usize,
    rec_canonical_ids: usize,
    unresolved_members: usize,
    unresolved_packages: usize,
    unsupported: BTreeMap<u8, usize>,
    missing_symbols: usize,
    /// Tags of the nodes that missing-symbol references point at.
    missing_targets: BTreeMap<u8, usize>,
    /// Missing targets with no `VALDEF`/`DEFDEF` body, `BLOCK`, `CASEDEF` or
    /// `LAMBDAtpt` ancestor. Those are the two things pass 1 documents as not entered
    /// (locals, including blocks and pattern binders in constructor arguments, and the parameters of
    /// type-lambda aliases); anything else would be a definition pass 1 should
    /// have entered.
    missing_outside_bodies: usize,
    /// Errors that mean a bug or malformed input, never an expected gap.
    unexpected: Vec<String>,
}

/// The name of a tag observed at the root of a full annotation tree.
fn full_root_name(tag: u8) -> String {
    match tag {
        60 => "SHAREDterm".to_owned(),
        95 => "NEW".to_owned(),
        136 => "APPLY".to_owned(),
        137 => "TYPEAPPLY".to_owned(),
        176 => "SELECTin".to_owned(),
        119 => "NAMEDARG".to_owned(),
        other => format!("tag {other}"),
    }
}

/// Files one `ANNOTATEDtype` root under its wire shape and its outcome.
/// `head` is the first tag of the annotation payload; `parent_failed` says the
/// parent, decoded on its own first, did not decode.
fn record_annotation(
    survey: &mut AnnotationSurvey,
    head: Option<u8>,
    parent_failed: bool,
    result: &Result<dotty_core::ids::TypeId, UnpickleError>,
) {
    let Some(head) = head else { return };
    if !is_compact_annot_type_tag(head) {
        let full = survey.full.entry(head).or_default();
        full.total += 1;
        match result {
            Ok(_) => full.decoded += 1,
            Err(_) if parent_failed => full.parent_failed += 1,
            Err(UnpickleError::UnsupportedAnnotationTree { .. }) => full.deferred_tree += 1,
            Err(
                UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. },
            ) => full.external += 1,
            Err(UnpickleError::MissingReferencedSymbol { .. }) => full.local_missing += 1,
            Err(UnpickleError::UnsupportedAnnotationConstructor { .. }) => {
                full.constructor_unsupported += 1;
            }
            Err(UnpickleError::UnsupportedAnnotationArgument { .. }) => {
                full.argument_unsupported += 1;
            }
            Err(UnpickleError::InvalidAnnotationType { .. }) => full.invalid_type += 1,
            Err(UnpickleError::MalformedType { .. } | UnpickleError::Ast(_)) => {
                full.malformed += 1;
            }
            Err(
                UnpickleError::AmbiguousMember { .. }
                | UnpickleError::UnsupportedSignedReference { .. }
                | UnpickleError::UnsupportedResolutionPrefix { .. }
                | UnpickleError::UnsupportedType { .. },
            ) => full.other_known += 1,
            Err(_) => {}
        }
        return;
    }
    let compact = &mut survey.compact;
    compact.total += 1;
    match result {
        Ok(_) => compact.decoded += 1,
        Err(UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. }) => {
            compact.external += 1;
        }
        Err(UnpickleError::MissingReferencedSymbol { .. }) => compact.local_missing += 1,
        Err(UnpickleError::UnsupportedType { tag, .. })
            if *tag == TYPEREFIN_TAG || *tag == TERMREFIN_TAG =>
        {
            compact.typerefin_deferred += 1;
            compact.typerefin_head_deferred += usize::from(head == TYPEREFIN_TAG);
        }
        Err(UnpickleError::UnsupportedType { .. }) => compact.unsupported_child += 1,
        Err(UnpickleError::UnsupportedResolutionPrefix { .. }) => compact.unsupported_prefix += 1,
        Err(UnpickleError::InvalidCompactAnnotationType { .. }) => {
            compact.invalid_compact_type += 1;
        }
        Err(UnpickleError::MalformedType { .. } | UnpickleError::Ast(_)) => compact.malformed += 1,
        Err(UnpickleError::UnsupportedAnnotationTree { .. }) => compact.parent_full_tree += 1,
        Err(
            UnpickleError::AmbiguousMember { .. }
            | UnpickleError::UnsupportedSignedReference { .. },
        ) => compact.other_known += 1,
        Err(_) => {}
    }
}

/// The tree rooted at `at`, decoded from the ASTs section (offsets absolute).
fn tree_from<'a>(file: &TastyFile<'a>, at: u32) -> dotty_tasty::tasty::RawTree<'a> {
    use dotty_tasty::tasty::{RawTree, Reader, StandardSection};
    let payload = file.section(StandardSection::Asts).unwrap().payload;
    let mut reader = Reader::with_range(payload, at as usize, payload.len()).unwrap();
    RawTree::decode_with_base_offset(&mut reader, 0).unwrap()
}

/// The constructor spine of a full annotation, arguments left out:
/// `APPLY(TYPEAPPLY(SELECTin(NEW(<class tag>))))`.
fn spine_shape(tree: &dotty_tasty::tasty::RawTree<'_>, arguments: &mut Vec<u32>) -> String {
    use dotty_tasty::tasty::RawTree;
    match tree {
        RawTree::LengthNode(node) if node.tag == 136 => {
            let apply = node.decode_apply().unwrap();
            arguments.push(u32::try_from(apply.arguments.len()).unwrap());
            format!("APPLY({})", spine_shape(&apply.function, arguments))
        }
        RawTree::LengthNode(node) if node.tag == 137 => {
            let apply = node.decode_type_apply().unwrap();
            format!("TYPEAPPLY({})", spine_shape(&apply.function, arguments))
        }
        RawTree::LengthNode(node) if node.tag == 176 => {
            let select = node.decode_select_in().unwrap();
            format!("SELECTin({})", spine_shape(&select.qualifier, arguments))
        }
        RawTree::Ast { tag: 95, child, .. } => match child.as_ref() {
            RawTree::Leaf(term) => format!("NEW(tag {})", term.tag),
            RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => format!("NEW(tag {tag})"),
            RawTree::LengthNode(node) => format!("NEW(tag {})", node.tag),
        },
        RawTree::Leaf(term) => format!("tag {}", term.tag),
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => format!("tag {tag}"),
        RawTree::LengthNode(node) => format!("tag {}", node.tag),
    }
}

/// Records the shape of the full `APPLY`/`NEW` annotation at `annotation_at`:
/// its spine, how many term arguments it applies and their root tags.
fn survey_shape(
    shapes: &mut FullShapes,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    annotation_at: u32,
) {
    let tree = tree_from(file, annotation_at);
    let mut layers = Vec::new();
    *shapes
        .spines
        .entry(spine_shape(&tree, &mut layers))
        .or_default() += 1;
    *shapes
        .argument_counts
        .entry(layers.iter().map(|count| *count as usize).sum())
        .or_default() += 1;

    // The arguments are the children of each `APPLY` after its function.
    let mut stack = vec![annotation_at];
    while let Some(at) = stack.pop() {
        let below = children.get(&at).map_or(&[][..], Vec::as_slice);
        for (child, tag) in below {
            *shapes.descendant_tags.entry(*tag).or_default() += 1;
            stack.push(*child);
        }
    }
    let mut current = annotation_at;
    loop {
        let below = children.get(&current).map_or(&[][..], Vec::as_slice);
        let tag = tree_from(file, current);
        let is_apply =
            matches!(&tag, dotty_tasty::tasty::RawTree::LengthNode(node) if node.tag == 136);
        let is_spine = matches!(&tag, dotty_tasty::tasty::RawTree::LengthNode(node) if node.tag == 137 || node.tag == 176);
        if is_apply {
            for (argument, argument_tag) in below.iter().skip(1) {
                let mut root = *argument_tag;
                if root == 119 {
                    shapes.named_arguments += 1;
                    root = children[argument][0].1;
                }
                *shapes.argument_roots.entry(root).or_default() += 1;
            }
        }
        match below.first() {
            Some((function, _)) if is_apply || is_spine => current = *function,
            _ => break,
        }
    }
}

/// Whether the full annotation at `annotation_at` is a constructor call of a
/// class named `ErasedParam` (a `TYPEREF` with that name below it).
fn names_erased_param(
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    annotation_at: u32,
) -> bool {
    use dotty_tasty::tasty::{RawName, RawTree};
    let mut stack = vec![annotation_at];
    while let Some(at) = stack.pop() {
        for (child, tag) in children.get(&at).map_or(&[][..], Vec::as_slice) {
            if *tag == 117
                && let RawTree::NatAst { value, .. } = tree_from(file, *child)
                && file
                    .names()
                    .entries()
                    .get(usize::try_from(value).unwrap())
                    .is_some_and(|name| name == &RawName::Utf8("ErasedParam".to_owned()))
            {
                return true;
            }
            stack.push(*child);
        }
    }
    false
}

/// Enters the unit and decodes every reference node in it.
fn run(
    label: &str,
    bytes: &[u8],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: Packages,
    tally: &mut Tally,
) -> Packages {
    let file = TastyFile::parse_compatible_with(bytes, 28, 9, 0).unwrap();
    let addresses: Vec<(u32, u8)> = {
        let index = file.ast_address_index().unwrap();
        index
            .iter_nodes()
            .filter(|node| {
                REFERENCE_TAGS.contains(&node.tag)
                    || is_measured(node.tag)
                    || matches!(node.tag, TYPEREFIN_TAG | TERMREFIN_TAG)
            })
            .map(|node| (u32::try_from(node.offset).unwrap(), node.tag))
            .collect()
    };

    let parents: HashMap<u32, u32> = file
        .ast_address_index()
        .unwrap()
        .iter_tree_edges()
        .map(|edge| {
            (
                u32::try_from(edge.child.offset).unwrap(),
                u32::try_from(edge.parent.offset).unwrap(),
            )
        })
        .collect();
    let tags: HashMap<u32, u8> = file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .map(|node| (u32::try_from(node.offset).unwrap(), node.tag))
        .collect();
    let inside_a_body = |mut at: u32| {
        while let Some(&parent) = parents.get(&at) {
            if matches!(tags.get(&parent), Some(129 | 130 | 140 | 155 | 171)) {
                return true;
            }
            at = parent;
        }
        false
    };

    {
        let index = file.ast_address_index().unwrap();
        for node in index.iter_nodes() {
            if node.tag == TYPEBOUNDS_TAG {
                tally.bounds.total += 1;
                let raw = index.get(u32::try_from(node.offset).unwrap());
                match raw
                    .ok_or(())
                    .and_then(|raw| raw.decode_type_bounds().map_err(|_| ()))
                {
                    Ok(bounds) => {
                        tally.bounds.two_sided += usize::from(bounds.high.is_some());
                        tally.bounds.alias_only += usize::from(bounds.high.is_none());
                        tally.bounds.with_variance += usize::from(!bounds.variances.is_empty());
                    }
                    Err(_) => tally.bounds.malformed += 1,
                }
            }
        }
    }

    // Variance-bearing bounds, and the lambdas that are their direct children.
    let (variance_bounds, variance_sources): (HashSet<u32>, HashSet<u32>) = {
        let index = file.ast_address_index().unwrap();
        let bounds: HashSet<u32> = index
            .iter_nodes()
            .filter(|node| node.tag == TYPEBOUNDS_TAG)
            .map(|node| u32::try_from(node.offset).unwrap())
            .filter(|at| {
                index
                    .get(*at)
                    .and_then(|raw| raw.decode_type_bounds().ok())
                    .is_some_and(|shape| !shape.variances.is_empty())
            })
            .collect();
        let sources = index
            .iter_tree_edges()
            .filter(|edge| {
                edge.child.tag == 170
                    && bounds.contains(&u32::try_from(edge.parent.offset).unwrap())
            })
            .map(|edge| u32::try_from(edge.child.offset).unwrap())
            .collect();
        (bounds, sources)
    };

    // Each `ANNOTATEDtype`'s parent and annotation payload: its two children.
    let annotated: HashMap<u32, (u32, u32, u8)> = {
        let index = file.ast_address_index().unwrap();
        let mut parents: HashMap<u32, u32> = HashMap::new();
        let mut found = HashMap::new();
        for edge in index.iter_tree_edges() {
            if edge.parent.tag == ANNOTATED_TAG {
                let annotated = u32::try_from(edge.parent.offset).unwrap();
                let child = u32::try_from(edge.child.offset).unwrap();
                match parents.get(&annotated) {
                    None => {
                        parents.insert(annotated, child);
                    }
                    Some(&parent) => {
                        found
                            .entry(annotated)
                            .or_insert((parent, child, edge.child.tag));
                    }
                }
            }
        }
        found
    };
    let annotation_heads: HashMap<u32, u8> = annotated
        .iter()
        .map(|(at, (_, _, head))| (*at, *head))
        .collect();

    tally.annotations.total += file
        .ast_address_index()
        .unwrap()
        .iter_nodes_with_tag(ANNOTATED_TAG)
        .count();
    for head in annotation_heads.values() {
        let heads = if is_compact_annot_type_tag(*head) {
            &mut tally.annotations.compact_heads
        } else {
            &mut tally.annotations.full_roots
        };
        *heads.entry(*head).or_default() += 1;
    }

    // The shapes of the full constructor applications, and which annotated
    // types carry `new ErasedParam`.
    let all_children: HashMap<u32, Vec<(u32, u8)>> = {
        let mut children: HashMap<u32, Vec<(u32, u8)>> = HashMap::new();
        for edge in file.ast_address_index().unwrap().iter_tree_edges() {
            children
                .entry(u32::try_from(edge.parent.offset).unwrap())
                .or_default()
                .push((u32::try_from(edge.child.offset).unwrap(), edge.child.tag));
        }
        children
    };
    let mut erased_annotated: HashSet<u32> = HashSet::new();
    for (at, (_, annotation_at, head)) in &annotated {
        if *head == 136 || *head == 95 {
            survey_shape(
                &mut tally.annotations.shapes,
                &file,
                &all_children,
                *annotation_at,
            );
            if names_erased_param(&file, &all_children, *annotation_at) {
                erased_annotated.insert(*at);
            }
        }
    }

    // For each `RECthis`, the binder address it names (its one `Nat`).
    let rec_this_target = |at: u32| -> Option<u32> {
        let payload = file
            .section(dotty_tasty::tasty::StandardSection::Asts)?
            .payload;
        let mut value: u32 = 0;
        for byte in payload.get(usize::try_from(at).ok()? + 1..)? {
            value = value.checked_mul(128)? | u32::from(byte & 0x7f);
            if byte & 0x80 != 0 {
                return Some(value);
            }
        }
        None
    };
    let mut rec_this_ids: HashMap<u32, HashSet<dotty_core::ids::TypeId>> = HashMap::new();

    let mut decoded_erased_methods: Vec<dotty_core::ids::TypeId> = Vec::new();
    let mut other_methods: Vec<dotty_core::ids::TypeId> = Vec::new();
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut *store, definitions, packages);
    unpickler.enter_symbols().unwrap();

    tally.units += 1;
    let (mut decoded, mut failed) = (0, 0);
    for (at, tag) in addresses {
        let outcomes = match tag {
            117 => &mut tally.named_type,
            115 => &mut tally.named_term,
            TYPEREFIN_TAG | TERMREFIN_TAG => tally.compound.entry(tag).or_default(),
            tag if is_measured(tag) => tally.compound.entry(tag).or_default(),
            _ => &mut tally.by_address,
        };
        outcomes.nodes += 1;
        if tag == 66 {
            let known = rec_this_target(at)
                .is_some_and(|binder| unpickler.index().type_at(binder).is_some());
            outcomes.binder_on_demand += usize::from(!known);
        }
        tally.variance.total += usize::from(variance_bounds.contains(&at));
        if tag == 172 {
            let binder_known = file
                .ast_address_index()
                .unwrap()
                .get(at)
                .and_then(|raw| raw.decode_param_type().ok())
                .is_some_and(|param| unpickler.index().type_at(param.binder.address).is_some());
            outcomes.binder_on_demand += usize::from(!binder_known);
        }
        // An annotated type decodes its parent first; asking for the parent
        // alone says whether a failure is the parent's.
        let parent_failed = tag == ANNOTATED_TAG
            && annotated
                .get(&at)
                .is_some_and(|(parent, _, _)| unpickler.unpickle_type(*parent).is_err());
        // The two children of a `REFin` decode on their own first, which says
        // whether a failure is theirs (as for the annotated parent above).
        let in_probe = (tag == TYPEREFIN_TAG || tag == TERMREFIN_TAG).then(|| {
            let children = all_children.get(&at).expect("a REFin has children");
            let [(prefix_at, _), (space_at, _)] = children[..] else {
                panic!("{label} @{at}: a REFin has two children");
            };
            let detail = tally.in_references.entry(tag).or_default();
            *detail
                .prefix_shapes
                .entry(shape_of(&file, &tags, prefix_at))
                .or_default() += 1;
            *detail
                .space_shapes
                .entry(shape_of(&file, &tags, space_at))
                .or_default() += 1;
            (
                unpickler.unpickle_type(prefix_at),
                unpickler.unpickle_type(space_at),
            )
        });
        let result = unpickler.unpickle_type(at);
        if let Some((Ok(_), Ok(space))) = &in_probe {
            let name = file
                .ast_address_index()
                .unwrap()
                .get(at)
                .and_then(|raw| raw.decode_in_reference().ok())
                .and_then(|node| file.names().get_utf8(node.name).map(str::to_owned));
            let namespace = if tag == TYPEREFIN_TAG {
                Namespace::Type
            } else {
                Namespace::Term
            };
            tally.oracle_queries.push((*space, name, namespace));
        }
        if let Some((prefix, space)) = &in_probe {
            record_in_reference(
                tally.in_references.entry(tag).or_default(),
                prefix,
                space,
                &result,
            );
        }
        if tag == ANNOTATED_TAG {
            record_annotation(
                &mut tally.annotations,
                annotation_heads.get(&at).copied(),
                parent_failed,
                &result,
            );
        }
        if tag == 180 {
            let erased = &mut tally.annotations.erased;
            erased.method_roots += 1;
            let contains = all_children.get(&at).is_some_and(|below| {
                below
                    .iter()
                    .any(|(child, _)| erased_annotated.contains(child))
            });
            erased.containing += usize::from(contains);
            if let (true, Ok(id)) = (contains, &result) {
                erased.decoded_containing += 1;
                decoded_erased_methods.push(*id);
            }
            if let (false, Ok(id)) = (contains, &result) {
                other_methods.push(*id);
            }
        }
        match result {
            Ok(first) => {
                decoded += 1;
                outcomes.decoded += 1;
                // Identity: decoding again never allocates a second type.
                assert_eq!(unpickler.unpickle_type(at), Ok(first));
                if variance_bounds.contains(&at) {
                    tally.variance.decoded += 1;
                }
                if tag == 170 {
                    if variance_sources.contains(&at) {
                        tally.variance.lambdas_as_variance_source += 1;
                    } else {
                        tally.variance.lambdas_standalone += 1;
                    }
                }
                if tag == 66 {
                    let binder = rec_this_target(at).expect("a decoded RECthis has a target");
                    rec_this_ids.entry(binder).or_default().insert(first);
                }
                if tag == 172 {
                    let binder = file
                        .ast_address_index()
                        .unwrap()
                        .get(at)
                        .and_then(|raw| raw.decode_param_type().ok())
                        .map(|param| param.binder.address);
                    let kind = match binder.and_then(|address| tags.get(&address)) {
                        Some(170) => "TYPELAMBDAtype",
                        Some(169) => "POLYtype",
                        Some(180) => "METHODtype",
                        _ => "another node",
                    };
                    *tally.param_binders.entry(kind).or_default() += 1;
                }
                if tag == TYPEBOUNDS_TAG {
                    let shape = file
                        .ast_address_index()
                        .unwrap()
                        .get(at)
                        .unwrap()
                        .decode_type_bounds()
                        .unwrap();
                    tally.bounds.decoded_alias += usize::from(shape.high.is_none());
                    tally.bounds.decoded_two_sided += usize::from(shape.high.is_some());
                }
            }
            Err(error) => {
                failed += 1;
                if variance_bounds.contains(&at) {
                    *tally
                        .variance
                        .failures
                        .entry(variance_failure(&error))
                        .or_default() += 1;
                }
                match error {
                    UnpickleError::UnsupportedType { tag, .. } => {
                        // For a reference node this is the node's own form;
                        // for a compound node it is one of its children.
                        outcomes.unsupported_child += 1;
                        *tally.unsupported.entry(tag).or_default() += 1;
                    }
                    UnpickleError::UnresolvedPackage { .. } => {
                        outcomes.needs_external += 1;
                        tally.unresolved_packages += 1;
                    }
                    UnpickleError::UnresolvedMember { .. } => {
                        outcomes.needs_external += 1;
                        tally.unresolved_members += 1;
                    }
                    UnpickleError::InvalidBinderReference { .. }
                    | UnpickleError::InvalidBinderKind { .. }
                    | UnpickleError::InvalidParameterIndex { .. }
                    | UnpickleError::InvalidTypeParameterBounds { .. }
                    | UnpickleError::InvalidMethodModifier { .. }
                    | UnpickleError::BoundsVarianceTargetPending { .. }
                    | UnpickleError::BoundsVarianceArityMismatch { .. }
                    | UnpickleError::RebindFailed { .. }
                    | UnpickleError::InvalidReferenceTarget { .. } => {
                        outcomes.binder_errors += 1;
                        outcomes.unexpected += 1;
                        tally.unexpected.push(format!("{label} @{at}: {error:?}"));
                    }
                    UnpickleError::UnsupportedAnnotationTree { .. }
                    | UnpickleError::UnsupportedAnnotationConstructor { .. }
                    | UnpickleError::UnsupportedAnnotationArgument { .. } => {
                        outcomes.deferred_annotation += 1;
                    }
                    UnpickleError::AmbiguousMember { .. } => outcomes.ambiguous += 1,
                    UnpickleError::UnsupportedSignedReference { .. } => outcomes.signed += 1,
                    UnpickleError::UnsupportedResolutionPrefix { .. }
                    | UnpickleError::UnsupportedResolutionSpace { .. } => {
                        outcomes.unsupported_prefix += 1;
                    }
                    // Counted in the `REFin` classification.
                    UnpickleError::IllegalTypePrefix { .. } => {}
                    UnpickleError::MissingReferencedSymbol { to, .. } => {
                        outcomes.missing_local += 1;
                        tally.missing_symbols += 1;
                        tally.missing_outside_bodies += usize::from(!inside_a_body(to));
                        let tag = tags.get(&to).copied();
                        *tally.missing_targets.entry(tag.unwrap_or(0)).or_default() += 1;
                    }
                    other => {
                        outcomes.unexpected += 1;
                        tally.unexpected.push(format!("{label} @{at}: {other:?}"));
                    }
                }
            }
        }
    }
    for (binder, ids) in &rec_this_ids {
        // One binder, one canonical `RecThis`, however many addresses name it.
        assert_eq!(ids.len(), 1, "{label}: binder {binder} has {ids:?}");
    }
    tally.rec_binders += rec_this_ids.len();
    tally.rec_canonical_ids += rec_this_ids.values().map(HashSet::len).sum::<usize>();
    tally.units_with_a_decoded_type += usize::from(decoded > 0);
    tally.units_fully_decoded += usize::from(failed == 0);
    let (index, packages) = unpickler.into_parts();
    for (at, tag) in &tags {
        if *tag == 131
            && let Some(symbol) = index.symbol_at(*at)
            && let Some(scope) = index.scope_of(symbol)
        {
            tally.class_scopes.insert(symbol, scope);
        }
    }
    // `erased` on the decoded method types: only the ones that name
    // `ErasedParam` may have it.
    let erased_count = |ids: &[dotty_core::ids::TypeId]| -> usize {
        ids.iter()
            .map(|id| match store.types.get(*id) {
                dotty_core::types::Type::Method(method) => {
                    method.params.iter().filter(|param| param.erased).count()
                }
                _ => 0,
            })
            .sum()
    };
    tally.annotations.erased.erased_params += erased_count(&decoded_erased_methods);
    assert_eq!(
        erased_count(&other_methods),
        0,
        "{label}: erased without ErasedParam"
    );
    packages
}

#[test]
fn the_corpus_report_names_exactly_the_compact_annotation_tags() {
    let named: Vec<u8> = COMPACT_HEADS.iter().map(|(tag, _)| *tag).collect();
    let compact: Vec<u8> = (0..=u8::MAX)
        .filter(|tag| is_compact_annot_type_tag(*tag))
        .collect();
    let mut sorted = named.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, compact);
}

#[test]
fn the_type_pass_never_fails_unexpectedly_on_the_small_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    let mut tally = Tally::default();
    for path in tasty_files(&root) {
        // The manifest-backed corpora are measured separately.
        if path.components().any(|part| {
            let part = part.as_os_str();
            part == "scala3-library" || part == "scala3-compiler"
        }) {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        let Ok(file) = TastyFile::parse_scala_3_9(&bytes) else {
            continue;
        };
        drop(file);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        run(
            &path.display().to_string(),
            &bytes,
            &mut store,
            definitions,
            Packages::new(),
            &mut tally,
        );
    }

    assert!(tally.units > 20, "found {} units", tally.units);
    assert!(tally.by_address.decoded > 0);
    assert_eq!(tally.unexpected, Vec::<String>::new());
}

/// Enters `scala.Any`, `scala.Nothing` and `scala.Null`, which the compiler
/// defines and no TASTy file declares.
fn provide_compiler_builtins(store: &mut SemanticStore, packages: &mut Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    let chain = packages.enter(store, SymbolOrigin::Synthetic, &["scala"]);
    let scala = chain.last().unwrap();
    for class in ["Any", "Nothing", "Null"] {
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(scala.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(scala.scope).enter(name, symbol);
    }
}

#[test]
#[ignore = "walks the whole scala3-library and scala3-compiler corpora"]
fn measure_the_type_pass_over_the_scala3_corpora() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    // The library's own TASTy has no `scala.Any` or `scala.Nothing`: the
    // compiler defines them. Every type-lambda parameter is bounded by them,
    // so without them no lambda can decode, and the binder path would go
    // unmeasured. The second pass provides just those (and `Null`), nothing
    // else, so the remaining failures are still real external references.
    for (corpus, builtins) in [
        ("scala3-library", false),
        ("scala3-compiler", false),
        ("scala3-library", true),
        ("scala3-compiler", true),
    ] {
        // One store and one package registry per corpus, as a classpath
        // would have.
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        if builtins {
            provide_compiler_builtins(&mut store, &mut packages);
        }
        let mut tally = Tally::default();
        for path in tasty_files(&root.join(corpus)) {
            let bytes = fs::read(&path).unwrap();
            packages = run(
                &path.display().to_string(),
                &bytes,
                &mut store,
                definitions,
                packages,
                &mut tally,
            );
        }

        let mut oracle = OwnerOracle::default();
        for (space, name, namespace) in &tally.oracle_queries {
            let scope = match store.types.get(*space) {
                dotty_core::types::Type::TypeRef { symbol, .. } => tally.class_scopes.get(symbol),
                _ => None,
            };
            let (Some(scope), Some(text)) = (scope, name) else {
                if scope.is_none() {
                    oracle.no_scope += 1;
                } else {
                    oracle.name_not_text += 1;
                }
                continue;
            };
            let declared = store.names.get(text).map_or(0, |text| {
                store
                    .scopes
                    .get(*scope)
                    .lookup_all(&dotty_core::names::Name::new(text, *namespace))
                    .len()
            });
            match declared {
                0 => oracle.none += 1,
                1 => oracle.one += 1,
                _ => oracle.several += 1,
            }
        }

        let mut unsupported: Vec<_> = tally.unsupported.iter().collect();
        unsupported.sort_by(|a, b| b.1.cmp(a.1));
        println!(
            "== {corpus} ({})",
            if builtins {
                "compiler builtins provided"
            } else {
                "no builtins"
            }
        );
        println!("units: {}", tally.units);
        println!(
            "units with a decoded type: {}",
            tally.units_with_a_decoded_type
        );
        println!(
            "units where every reference node decodes: {}",
            tally.units_fully_decoded
        );
        for (label, outcomes) in [
            ("by address", &tally.by_address),
            ("named TYPEREF", &tally.named_type),
            ("named TERMREF", &tally.named_term),
        ] {
            println!(
                "{label}: nodes {}, decoded {}, need external {}, ambiguous {}, signed {}, unsupported prefix {}, unexpected {}",
                outcomes.nodes,
                outcomes.decoded,
                outcomes.needs_external,
                outcomes.ambiguous,
                outcomes.signed,
                outcomes.unsupported_prefix,
                outcomes.unexpected,
            );
        }
        let report = |title: &str, tags: &[(u8, &str)]| {
            println!("{title} (decoded: the node and every child decode):");
            for (tag, label) in tags {
                let none = Outcomes::default();
                let o = tally.compound.get(tag).unwrap_or(&none);
                println!(
                    "  {label}: nodes {}, decoded {}; failures: external {}, ambiguous {}, signed {}, unsupported prefix {}, local {}, unsupported form {}, deferred full annotation {}, binder errors {}; binder decoded on demand {}; unexpected {}",
                    o.nodes,
                    o.decoded,
                    o.needs_external,
                    o.ambiguous,
                    o.signed,
                    o.unsupported_prefix,
                    o.missing_local,
                    o.unsupported_child,
                    o.deferred_annotation,
                    o.binder_errors,
                    o.binder_on_demand,
                    o.unexpected,
                );
            }
        };
        for (tag, label) in [(TYPEREFIN_TAG, "TYPEREFin"), (TERMREFIN_TAG, "TERMREFin")] {
            let none = InReferenceOutcomes::default();
            let o = tally.in_references.get(&tag).unwrap_or(&none);
            println!(
                "{label}: nodes {}, decoded {}, both children decoded {}; failures: prefix {:?}, ownerSpace external {}, ownerSpace local missing {}, ownerSpace unsupported {}, ambiguous {}, signed {}, resolver unresolved {}, resolver malformed {}, illegal prefix (QualSkolem) {}, other child {}; unexpected {}",
                o.nodes,
                o.decoded,
                o.children_decoded,
                o.prefix_failure,
                o.space_external,
                o.space_local_missing,
                o.space_unsupported,
                o.ambiguous,
                o.signed,
                o.resolver_unresolved,
                o.resolver_malformed,
                o.illegal_prefix,
                o.other_child,
                o.unexpected,
            );
            println!("  prefix shapes: {:?}", o.prefix_shapes);
            println!("  ownerSpace shapes: {:?}", o.space_shapes);
        }
        println!(
            "REFin owner-space oracle (both children decoded; the owner class's own scope, from the unit that entered it): no scope known {}, name is not text {}, no declaration {}, exactly one declaration {}, several {}",
            oracle.no_scope, oracle.name_not_text, oracle.none, oracle.one, oracle.several
        );
        report("compound types", &COMPOUND_TAGS);
        report("bounds and flexible types", &WRAPPER_TAGS);
        report("binder types", &BINDER_TAGS);
        report("refined and recursive types", &RECURSIVE_TAGS);
        println!(
            "RECthis: unique recursive binders named {}, canonical RecThis ids {}",
            tally.rec_binders, tally.rec_canonical_ids
        );
        println!(
            "decoded PARAMtype by binder kind: {:?}",
            tally.param_binders
        );
        report("constant nodes", &CONSTANT_TAGS);
        println!(
            "variance-bearing TYPEBOUNDS: total {}, decoded {}; failures {:?}",
            tally.variance.total, tally.variance.decoded, tally.variance.failures
        );
        println!(
            "TYPELAMBDAtype decoded: standalone {}, as variance source {}",
            tally.variance.lambdas_standalone, tally.variance.lambdas_as_variance_source
        );
        println!(
            "TYPEBOUNDS shapes: total {}, two-sided {}, alias-only {}, with variance {}, malformed {}; decoded alias-only {}, decoded two-sided {}",
            tally.bounds.total,
            tally.bounds.two_sided,
            tally.bounds.alias_only,
            tally.bounds.with_variance,
            tally.bounds.malformed,
            tally.bounds.decoded_alias,
            tally.bounds.decoded_two_sided,
        );
        let survey = &tally.annotations;
        let head_name = |tag: &u8| {
            COMPACT_HEADS
                .iter()
                .find(|(t, _)| t == tag)
                .map_or_else(|| format!("tag {tag}"), |(_, name)| (*name).to_owned())
        };
        let full_total: usize = survey.full.values().map(|full| full.total).sum();
        println!(
            "ANNOTATEDtype: total {}, compact {}, full tree {}, unclassified {}",
            survey.total,
            survey.compact.total,
            full_total,
            survey.total - survey.compact.total - full_total,
        );
        println!(
            "  compact by wire head: {:?}",
            survey
                .compact_heads
                .iter()
                .map(|(tag, count)| (head_name(tag), *count))
                .collect::<Vec<_>>()
        );
        println!(
            "  full tree by root tag: {:?}",
            survey
                .full_roots
                .iter()
                .map(|(tag, count)| (full_root_name(*tag), *count))
                .collect::<Vec<_>>()
        );
        let c = &survey.compact;
        println!(
            "  compact outcomes: total {}, decoded {}, external {}, local missing {}, unsupported child {}, TYPEREFin/TERMREFin deferred {} (annotation is TYPEREFin: {}), unsupported prefix {}, invalid compact type {}, malformed {}, other known {}, full tree in parent {}",
            c.total,
            c.decoded,
            c.external,
            c.local_missing,
            c.unsupported_child,
            c.typerefin_deferred,
            c.typerefin_head_deferred,
            c.unsupported_prefix,
            c.invalid_compact_type,
            c.malformed,
            c.other_known,
            c.parent_full_tree,
        );
        for (root, f) in &survey.full {
            println!(
                "  full {} outcomes: total {}, decoded {}, parent failed first {}, external {}, local missing {}, constructor unsupported {}, argument unsupported {}, invalid type {}, malformed {}, other known {}, deferred tree {}",
                full_root_name(*root),
                f.total,
                f.decoded,
                f.parent_failed,
                f.external,
                f.local_missing,
                f.constructor_unsupported,
                f.argument_unsupported,
                f.invalid_type,
                f.malformed,
                f.other_known,
                f.deferred_tree,
            );
        }
        let shapes = &survey.shapes;
        println!("  APPLY/NEW spines: {:?}", shapes.spines);
        println!("  argument counts: {:?}", shapes.argument_counts);
        println!(
            "  argument root tags: {:?}, named arguments {}",
            shapes.argument_roots, shapes.named_arguments
        );
        println!(
            "  tags below annotation roots: {:?}",
            shapes.descendant_tags
        );
        let e = &survey.erased;
        println!(
            "  METHODtype roots {}, naming ErasedParam {}, of which decoded {}; erased MethodParams {}",
            e.method_roots, e.containing, e.decoded_containing, e.erased_params
        );
        println!(
            "unresolved: members {}, packages {}",
            tally.unresolved_members, tally.unresolved_packages
        );
        println!(
            "missing symbols (locals, other units): {}",
            tally.missing_symbols
        );
        println!(
            "missing symbols that are neither locals nor type-lambda parameters: {}",
            tally.missing_outside_bodies
        );
        println!("missing symbols by target tag: {:?}", tally.missing_targets);
        println!("unsupported by tag: {unsupported:?}");
        println!("unexpected errors: {}", tally.unexpected.len());
        for error in tally.unexpected.iter().take(10) {
            println!("  {error}");
        }
        assert!(tally.unexpected.is_empty());
        assert_eq!(tally.missing_outside_bodies, 0);
        // Every annotated type is either compact or a full tree, and every
        // compact one is filed under an outcome.
        assert_eq!(survey.total, survey.compact.total + full_total);
        assert_eq!(
            survey.compact.total,
            survey.compact_heads.values().sum::<usize>()
        );
        assert_eq!(
            survey.compact.total,
            c.decoded
                + c.external
                + c.local_missing
                + c.unsupported_child
                + c.typerefin_deferred
                + c.unsupported_prefix
                + c.invalid_compact_type
                + c.malformed
                + c.other_known
                + c.parent_full_tree
        );
        // Every full annotation is filed under an outcome. Only `APPLY` and
        // `NEW` are read; any other root is deferred, never decoded.
        for (root, f) in &survey.full {
            let filed = f.decoded
                + f.parent_failed
                + f.external
                + f.local_missing
                + f.constructor_unsupported
                + f.argument_unsupported
                + f.invalid_type
                + f.malformed
                + f.other_known
                + f.deferred_tree;
            assert_eq!(f.total, filed, "root {root}");
            if *root == 136 || *root == 95 {
                assert_eq!(f.deferred_tree, 0, "root {root}");
            } else {
                assert_eq!(f.decoded, 0, "root {root}");
            }
        }
        for tag in survey.compact_heads.keys() {
            assert!(is_compact_annot_type_tag(*tag));
        }
        // A well-formed marker never fails for being a marker.
        for kind in [
            "pending variance target",
            "arity mismatch",
            "rebind failure",
            "unexpected",
        ] {
            assert_eq!(
                tally.variance.failures.get(kind).copied().unwrap_or(0),
                0,
                "{kind}"
            );
        }
        // No binder form is ever "unsupported" for being that form.
        for (tag, label) in BINDER_TAGS.into_iter().chain(RECURSIVE_TAGS) {
            assert!(
                !tally.unsupported.contains_key(&tag),
                "{label} was reported as an unsupported form"
            );
        }
    }
}
