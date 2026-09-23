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
use dotty_tasty_unpickler::tasty_unpickler::{
    IdentityOutcome, TastyUnpickler, UnpickleError, identity_reachability,
};

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

/// Match types (Milestone 4d): decoded, but present in neither baseline corpus.
const MATCH_TAGS: [(u8, &str); 2] = [(190, "MATCHtype"), (192, "MATCHCASEtype")];

/// `MATCHtpt`: the source syntax of a match type, a tree and not a type.
const MATCHTPT_TAG: u8 = 191;

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
        || MATCH_TAGS.iter().any(|(t, _)| *t == tag)
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
    /// constructor call (also behind a `SHAREDterm`), or a constructor part or
    /// argument it does not read (Milestones 4b2a, 4b2b).
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
    /// Refused as a tree whose root is not `APPLY`/`NEW` (or a `SHAREDterm`
    /// that ends at no such tree).
    deferred_tree: usize,
    /// A `SHAREDterm` chain that could not be followed: an invalid target, a
    /// cycle or an overlong chain (Milestone 4b2b).
    invalid_reference: usize,
}

/// The wire shape of the full annotations rooted at `SHAREDterm` (Milestone
/// 4b2b), independent of whether they decode.
#[derive(Default)]
struct SharedShapes {
    roots: usize,
    /// The link to a node that is not a node, or a chain that never ends.
    invalid_chains: usize,
    /// Number of links followed, from the annotation's own to the tree.
    depths: BTreeMap<usize, usize>,
    /// Links whose target is another `SHAREDterm`.
    links_to_links: usize,
    /// Root tag of the tree each valid chain ends at.
    target_tags: BTreeMap<u8, usize>,
    /// Per unit, the outer annotations naming each target address.
    per_target: HashMap<(usize, u32), usize>,
    /// Outcomes by the tag the chain ends at: total, decoded.
    outcomes_by_target: BTreeMap<u8, (usize, usize)>,
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

/// What became of the symbols of one kind when completion was tried.
#[derive(Default)]
struct CompletionOutcomes {
    entered: usize,
    already_complete: usize,
    by_bucket: BTreeMap<&'static str, usize>,
}

/// Which symbols the corpus completes before decoding references.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Completion {
    Off,
    /// Everything but class-like symbols (the 5c behavior).
    WithoutClasses,
    All,
}

/// A class-like symbol completed to a `ClassInfo`, queued until the unit's
/// store is free to read (Milestone 5d1).
struct CompletedClass {
    kind: String,
    symbol: dotty_core::ids::SymbolId,
    info: dotty_core::ids::TypeId,
    /// The scope pass 1 entered for it.
    scope: Option<dotty_core::ids::ScopeId>,
    /// The number of parent trees and whether a `SELFDEF` exists, on the wire.
    wire_parents: usize,
    wire_self: bool,
}

/// The class completion and parent / self survey (Milestone 5d1).
#[derive(Default)]
struct ClassSurvey {
    /// (kind, phase, failure) -> count. The phase is the part of the class the
    /// failing reference lies in (header parameter, parent, self type, or
    /// elsewhere: a shared link's target).
    failures: BTreeMap<(String, &'static str, String), usize>,
    /// The name every unresolved member / package failure of a class asked for.
    external_names: BTreeMap<String, usize>,
    /// Classes by kind and number of parents.
    parent_counts: BTreeMap<(String, usize), usize>,
    /// (kind, root) -> count of parent trees; a `SHAREDterm` is named by the
    /// tag its chain ends at.
    parent_roots: BTreeMap<(String, String), usize>,
    /// The constructor-call spine of every parent tree, e.g.
    /// `APPLY>TYPEAPPLY>SELECTin>NEW>APPLIEDtpt`.
    spines: BTreeMap<String, usize>,
    /// Term arguments of each `APPLY` in a parent spine.
    argument_counts: BTreeMap<usize, usize>,
    /// `TYPEAPPLY` in a parent: whether the constructor selection's `NEW` type
    /// tree is already applied (upstream then skips the arguments) or not.
    type_apply: BTreeMap<&'static str, usize>,
    /// `BLOCK` parents: the statement count and their spine.
    blocks: BTreeMap<String, usize>,
    /// Ordinary (non-call) parent trees by root tag.
    direct_roots: BTreeMap<String, usize>,
    /// Completed parents by semantic result shape: (kind, shape) -> count.
    parent_shapes: BTreeMap<(String, &'static str), usize>,
    /// `SELFDEF`: (kind, what, value) -> count.
    selfs: BTreeMap<(String, &'static str, String), usize>,
    /// Why each class that failed to complete failed, by symbol (the last
    /// attempt), for explaining the `REFin` owners that never completed.
    failure_of: HashMap<dotty_core::ids::SymbolId, String>,
    /// Completed classes not yet checked against the store.
    pending: Vec<CompletedClass>,
    /// `ClassInfo` field sanity checks passed.
    checked: usize,
}

/// The completion measurement (Milestone 5a) and the type-tree survey it is
/// scoped by.
#[derive(Default)]
struct CompletionSurvey {
    /// Class-like completion (Milestone 5d1).
    classes: ClassSurvey,
    /// Outcomes by symbol kind.
    outcomes: BTreeMap<String, CompletionOutcomes>,
    /// `SymbolInfo` per kind after completion: kind -> state -> count.
    infos: BTreeMap<(String, &'static str), usize>,
    /// Type-tree root by definition context; a `SHAREDterm` is reported by
    /// the tag its chain ends at.
    tree_roots: BTreeMap<(&'static str, String), usize>,
    /// Tags of the type trees completion refused.
    unsupported_trees: BTreeMap<u8, usize>,
    /// Root tag of each `APPLIEDtpt` constructor, and how many are named `&`
    /// or `|` (the special constructors upstream canonicalizes).
    applied_constructors: BTreeMap<String, usize>,
    applied_and_or: usize,
    /// The wire shapes of every `SELECTtpt` / `SINGLETONtpt` / `ANNOTATEDtpt`
    /// (Milestone 5b), independent of what any of them decodes to: (survey,
    /// root) -> count. A `SHAREDterm` is reported by the tag its chain ends at.
    tpt_roots: BTreeMap<(&'static str, String), usize>,
    /// `SHAREDterm` chain lengths under those trees: (survey, links) -> count.
    tpt_links: BTreeMap<(&'static str, usize), usize>,
    /// The constructor spines and arguments of the `ANNOTATEDtpt` annotations.
    annotated_tpt_shapes: FullShapes,
    /// Completed symbols by kind and by which of the 5b trees (`SELECTtpt`,
    /// `ANNOTATEDtpt`, `SINGLETONtpt`) their declared tree contains: the
    /// completions those forms made possible. `-` is none of them.
    completed_by_new_trees: BTreeMap<(String, String), usize>,
    /// The outcome of every symbol whose tree contains one of them, by tree
    /// set and outcome: where the formerly unsupported ones went.
    outcomes_by_new_trees: BTreeMap<(String, &'static str), usize>,
    /// Tags of the term trees the projection refused.
    unsupported_terms: BTreeMap<u8, usize>,
    /// The `LAMBDAtpt` survey (Milestone 5c): (what, value) -> count.
    lambdas: BTreeMap<(&'static str, String), usize>,
    /// The method / constructor survey (Milestone 5c): (kind, what, value) ->
    /// count.
    methods: BTreeMap<(&'static str, &'static str, String), usize>,
    /// The first few symbols of the buckets worth a look.
    examples: BTreeMap<&'static str, Vec<String>>,
    unexpected: Vec<String>,
}

/// The wire shape of `MATCHtpt`, measured for context (Milestone 4d).
#[derive(Default)]
struct MatchTptSurvey {
    total: usize,
    with_bound: usize,
    case_counts: BTreeMap<usize, usize>,
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
    shared: SharedShapes,
    /// The unit being walked, to tell target addresses of different units
    /// apart.
    unit: usize,
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
    /// Of those with a scope, the owner ended the corpus with a `ClassInfo`.
    owner_completed: usize,
    /// Why the owners without one failed: reason -> queries.
    owner_failures: BTreeMap<String, usize>,
}

/// The identity-reachability oracle's corpus-wide measurement (Milestone
/// 5d2c's follow-up review of issue #101): every `LAMBDAtpt`/`REFINEDtpt`
/// physically present in the wire, classified by
/// [`dotty_tasty_unpickler::tasty_unpickler::identity_reachability`], plus
/// the route (`crate::discovery::DiscoveryRoute`'s name) each *entered*
/// identity's first owner was found through. `unaccounted` is the number the
/// milestone's own parity claim lives or dies by: any corpus unit that
/// reports one has a real, undiscovered `LAMBDAtpt`/`REFINEDtpt`, not merely
/// an unmeasured one.
#[derive(Default)]
struct IdentitySurvey {
    lambda_entered: usize,
    lambda_out_of_scope: usize,
    lambda_unaccounted: usize,
    lambda_conflicts: usize,
    refined_entered: usize,
    refined_out_of_scope: usize,
    refined_unaccounted: usize,
    refined_conflicts: usize,
    /// The route each entered `LAMBDAtpt`'s first owner was found through.
    lambda_routes: BTreeMap<&'static str, usize>,
    /// The route each entered `REFINEDtpt`'s first owner was found through.
    refined_routes: BTreeMap<&'static str, usize>,
    /// `unit label @address, tag` of every `Unaccounted` node, for triage.
    unaccounted_examples: Vec<String>,
}

#[derive(Default)]
struct Tally {
    /// Simple symbol completion (Milestone 5a).
    completion: CompletionSurvey,
    /// `MATCHtpt` trees: total, those with an explicit bound, and how many
    /// have each number of cases. Never given to `unpickle_type`.
    match_tpt: MatchTptSurvey,
    /// Every class scope entered so far, by the class symbol.
    class_scopes: HashMap<dotty_core::ids::SymbolId, dotty_core::ids::ScopeId>,
    /// `REFin` nodes whose two children decoded: the owner space, the
    /// declaration name and its namespace, for the [`OwnerOracle`].
    oracle_queries: Vec<(dotty_core::ids::TypeId, Option<String>, Namespace)>,
    /// Name-designated references created (Milestone 4c2), by reference kind
    /// and the semantic variant of their prefix.
    name_targets: BTreeMap<(&'static str, &'static str), usize>,
    /// Name-based `TYPEREF` / `TERMREF` roots that failed on an unsupported
    /// prefix (Milestone 4c2 survey), by tag and the prefix's wire shape.
    unsupported_prefix_shapes: BTreeMap<(&'static str, String), usize>,
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
    /// The identity-reachability oracle (Milestone 5d2c's follow-up review).
    identity: IdentitySurvey,
    /// Serialized symbol annotations (Milestone 5e1's own corpus survey).
    symbol_annotations: SymbolAnnotationSurvey,
    /// Symbol-annotation *completion* outcomes (Milestone 5e1's own corpus
    /// outcome survey, issue #129 §30/31).
    symbol_annotation_completion: SymbolAnnotationCompletionSurvey,
    /// Errors that mean a bug or malformed input, never an expected gap.
    unexpected: Vec<String>,
}

/// Serialized symbol annotations (Milestone 5e1): every
/// `DefinitionTail::Annotation` pass 1 indexes, over every `TYPEDEF`,
/// `VALDEF`, `DEFDEF`, `TYPEPARAM` and `PARAM` address pass 1 actually
/// entered a symbol for. Decodes nothing: this counts wire occurrences only,
/// exactly what pass 1's index records.
#[derive(Default)]
struct SymbolAnnotationSurvey {
    /// Every `ANNOTATION` tail entry recorded, over every entered address.
    total: usize,
    /// Entered addresses with at least one `ANNOTATION` tail entry.
    annotated_definitions: usize,
    /// Every entered address, annotated or not (the denominator for
    /// `annotated_definitions`).
    definitions: usize,
    /// Annotation tail entries, summed per definition/parameter tag.
    by_definition_tag: BTreeMap<&'static str, usize>,
    /// Entered addresses with at least one annotation, per definition/
    /// parameter tag.
    annotated_by_definition_tag: BTreeMap<&'static str, usize>,
    /// "Annotations on one definition" -> how many annotated definitions
    /// have exactly that many.
    per_definition_distribution: BTreeMap<usize, usize>,
}

/// The name of a definition/parameter tag, for the symbol-annotation survey.
fn definition_tag_name(tag: u8) -> &'static str {
    use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG};
    match tag {
        TYPEDEF_TAG => "TYPEDEF",
        VALDEF_TAG => "VALDEF",
        DEFDEF_TAG => "DEFDEF",
        TYPEPARAM_TAG => "TYPEPARAM",
        PARAM_TAG => "PARAM",
        _ => "other",
    }
}

/// Surveys the `ANNOTATION` tail entries pass 1 indexed for `file`
/// (Milestone 5e1). Reads only [`TastySemanticIndex::annotation_tail_at`]:
/// an address pass 1 never entered a symbol for (a local definition inside a
/// method body, in particular) has no recorded tail and is skipped, exactly
/// as it is skipped by every other survey in this module.
fn survey_symbol_annotations(
    file: &TastyFile<'_>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    survey: &mut SymbolAnnotationSurvey,
) {
    use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG};
    let index = file.ast_address_index().unwrap();
    for tag in [
        TYPEDEF_TAG,
        VALDEF_TAG,
        DEFDEF_TAG,
        TYPEPARAM_TAG,
        PARAM_TAG,
    ] {
        for node in index.iter_nodes_with_tag(tag) {
            let at = u32::try_from(node.offset).unwrap();
            let Some(annotations) = unpickler.index().annotation_tail_at(at) else {
                continue;
            };
            let name = definition_tag_name(tag);
            survey.definitions += 1;
            survey.total += annotations.len();
            *survey.by_definition_tag.entry(name).or_default() += annotations.len();
            if !annotations.is_empty() {
                survey.annotated_definitions += 1;
                *survey.annotated_by_definition_tag.entry(name).or_default() += 1;
                *survey
                    .per_definition_distribution
                    .entry(annotations.len())
                    .or_default() += 1;
            }
        }
    }
}

/// Symbol-annotation completion outcomes (Milestone 5e1's own corpus outcome
/// survey, issue #129 §30/31): every definition/parameter with at least one
/// indexed `ANNOTATION` entry is completed through the real
/// `TastyUnpickler::complete_symbol_annotations`, and classified.
/// `unexpected` must stay empty: every other bucket is an expected outcome
/// on real compiler output (most of all `external`, since this survey never
/// enters the annotation classes of another unit the way a real multi-unit
/// session would), but an error this survey does not recognize is not.
#[derive(Default)]
struct SymbolAnnotationCompletionSurvey {
    /// Definitions/parameters with at least one indexed `ANNOTATION` entry.
    attempted: usize,
    /// Fully decoded: every `ANNOTATION` entry became an `AnnotationId`.
    decoded: usize,
    /// `UnresolvedPackage` / `UnresolvedMember`: the annotation class (or one
    /// of its type arguments) is not entered in this unit-only session.
    external: usize,
    /// `UnsupportedAnnotationConstructor`: a full annotation's constructor
    /// spine is not `APPLY`/`TYPEAPPLY`/`SELECTin`/`NEW`, or its class tree
    /// is not `SELECTtpt`/`IDENTtpt`/a bare type.
    unsupported_constructor: usize,
    /// `UnsupportedAnnotationArgument`: a full annotation's argument is not a
    /// literal or class literal.
    unsupported_argument: usize,
    /// `InvalidAnnotationType` / `InvalidCompactAnnotationType`: the
    /// annotation's resolved type is not `TypeRef`/`Applied` (or is still
    /// pending).
    invalid_type: usize,
    /// `UnsupportedAnnotationTree`: the payload is neither a compact type nor
    /// a constructor application (directly, or through `SHAREDterm` links).
    unsupported_tree: usize,
    /// `MalformedType`: the wrapper's own shape (`ANNOTATION`'s two
    /// children) does not match what `RawNode::decode_annotation` expects.
    malformed: usize,
    /// Any other typed error this survey recognizes but does not bucket
    /// separately (for example `InvalidReferenceTarget`, a bad `SHAREDterm`
    /// chain).
    other_known: usize,
    /// Errors this survey does not recognize: `label @address: error`.
    unexpected: Vec<String>,
}

fn survey_symbol_annotation_completion(
    file: &TastyFile<'_>,
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    label: &str,
    survey: &mut SymbolAnnotationCompletionSurvey,
) {
    use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG};
    let addresses: Vec<u32> = {
        let index = file.ast_address_index().unwrap();
        [
            TYPEDEF_TAG,
            VALDEF_TAG,
            DEFDEF_TAG,
            TYPEPARAM_TAG,
            PARAM_TAG,
        ]
        .into_iter()
        .flat_map(|tag| {
            index
                .iter_nodes_with_tag(tag)
                .map(|node| u32::try_from(node.offset).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
    };
    for at in addresses {
        let has_annotations = unpickler
            .index()
            .annotation_tail_at(at)
            .is_some_and(|tail| !tail.is_empty());
        if !has_annotations {
            continue;
        }
        survey.attempted += 1;
        match unpickler.complete_symbol_annotations(at) {
            Ok(_) => survey.decoded += 1,
            Err(
                UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. },
            ) => {
                survey.external += 1;
            }
            Err(UnpickleError::UnsupportedAnnotationConstructor { .. }) => {
                survey.unsupported_constructor += 1;
            }
            Err(UnpickleError::UnsupportedAnnotationArgument { .. }) => {
                survey.unsupported_argument += 1;
            }
            Err(
                UnpickleError::InvalidAnnotationType { .. }
                | UnpickleError::InvalidCompactAnnotationType { .. },
            ) => {
                survey.invalid_type += 1;
            }
            Err(UnpickleError::UnsupportedAnnotationTree { .. }) => {
                survey.unsupported_tree += 1;
            }
            Err(UnpickleError::MalformedType { .. }) => survey.malformed += 1,
            Err(UnpickleError::InvalidReferenceTarget { .. }) => survey.other_known += 1,
            Err(other) => survey.unexpected.push(format!("{label} @{at}: {other:?}")),
        }
    }
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
        61 => "SHAREDtype".to_owned(),
        101 => "SINGLETONtpt".to_owned(),
        111 => "IDENTtpt".to_owned(),
        113 => "SELECTtpt".to_owned(),
        116 => "TERMREFpkg".to_owned(),
        117 => "TYPEREF".to_owned(),
        94 => "BYNAMEtpt".to_owned(),
        103 => "EXPLICITtpt".to_owned(),
        140 => "BLOCK".to_owned(),
        153 => "ANNOTATEDtype".to_owned(),
        154 => "ANNOTATEDtpt".to_owned(),
        160 => "REFINEDtpt".to_owned(),
        161 => "APPLIEDtype".to_owned(),
        162 => "APPLIEDtpt".to_owned(),
        164 => "TYPEBOUNDStpt".to_owned(),
        167 => "ORtype".to_owned(),
        171 => "LAMBDAtpt".to_owned(),
        191 => "MATCHtpt".to_owned(),
        193 => "FLEXIBLEtype".to_owned(),
        other => format!("tag {other}"),
    }
}

fn info_state(info: dotty_core::symbols::SymbolInfo) -> &'static str {
    use dotty_core::symbols::SymbolInfo;
    match info {
        SymbolInfo::Missing => "Missing",
        SymbolInfo::Deferred(_) => "Deferred",
        SymbolInfo::Complete(_) => "Complete",
        SymbolInfo::Error => "Error",
    }
}

/// The name of a tree root for the survey: a `SHAREDterm` is followed to its
/// end.
fn tree_root_name(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    at: u32,
) -> String {
    match follow_shared_terms_in(index, file, at, 16) {
        Some((links, _, tag, _)) if links > 0 => format!("SHAREDterm -> {}", full_root_name(tag)),
        Some((_, _, tag, _)) => full_root_name(tag),
        None => "SHAREDterm (invalid)".to_owned(),
    }
}

/// Completes every eligible symbol of the unit, in document order, and
/// records what happened, and surveys the type trees each definition context
/// carries (whether or not completion supports them).
/// Runs the identity-reachability oracle over `file`'s whole AST section
/// (independent of the top-down declared-type walk `enter_symbols` already
/// did) and folds the result into `survey`: every `LAMBDAtpt`/`REFINEDtpt`
/// discovery should have entered, in fact did, plus the route each one's
/// first owner was found through.
fn survey_identity_reachability(
    file: &TastyFile<'_>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    label: &str,
    survey: &mut IdentitySurvey,
) {
    const LAMBDATPT_TAG: u8 = 171;
    for node in identity_reachability(file, unpickler.index()).unwrap() {
        let is_lambda = node.tag == LAMBDATPT_TAG;
        match node.outcome {
            IdentityOutcome::Entered => {
                let (route, routes) = if is_lambda {
                    (
                        unpickler.index().lambda_route(node.address),
                        &mut survey.lambda_routes,
                    )
                } else {
                    (
                        unpickler.index().refined_route(node.address),
                        &mut survey.refined_routes,
                    )
                };
                *routes.entry(route.unwrap_or("?")).or_default() += 1;
                if is_lambda {
                    survey.lambda_entered += 1;
                    survey.lambda_conflicts +=
                        usize::from(unpickler.index().has_lambda_owner_conflict(node.address));
                } else {
                    survey.refined_entered += 1;
                    survey.refined_conflicts +=
                        usize::from(unpickler.index().has_refined_owner_conflict(node.address));
                }
            }
            IdentityOutcome::OutOfScope => {
                if is_lambda {
                    survey.lambda_out_of_scope += 1;
                } else {
                    survey.refined_out_of_scope += 1;
                }
            }
            IdentityOutcome::Unaccounted => {
                if is_lambda {
                    survey.lambda_unaccounted += 1;
                } else {
                    survey.refined_unaccounted += 1;
                }
                if survey.unaccounted_examples.len() < 20 {
                    survey
                        .unaccounted_examples
                        .push(format!("{label} @{}: tag {}", node.address, node.tag));
                }
            }
        }
    }
}

fn complete_unit(
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    file: &TastyFile<'_>,
    label: &str,
    survey: &mut CompletionSurvey,
    complete_classes: bool,
) {
    const VALDEF: u8 = 129;
    const TYPEDEF: u8 = 131;
    const DEFDEF: u8 = 130;
    const TYPEPARAM: u8 = 133;
    const PARAM: u8 = 134;
    const TEMPLATE: u8 = 156;
    const SELFDEF: u8 = 118;
    let index = file.ast_address_index().unwrap();
    let mut definitions: Vec<(u32, u8)> = index
        .iter_nodes()
        .filter(|node| matches!(node.tag, VALDEF | TYPEDEF | DEFDEF | TYPEPARAM | PARAM))
        .map(|node| (u32::try_from(node.offset).unwrap(), node.tag))
        .collect();
    definitions.sort_unstable();
    let mut children: HashMap<u32, Vec<(u32, u8)>> = HashMap::new();
    for edge in index.iter_tree_edges() {
        children
            .entry(u32::try_from(edge.parent.offset).unwrap())
            .or_default()
            .push((u32::try_from(edge.child.offset).unwrap(), edge.child.tag));
    }
    let below = |at: u32| children.get(&at).map_or(&[][..], Vec::as_slice);

    // The tree survey, for every definition the pass entered a symbol for.
    for (at, tag) in &definitions {
        if unpickler.symbol_state_at(*at).is_none() {
            continue;
        }
        let mut record = |context: &'static str, tree: u32| {
            *survey
                .tree_roots
                .entry((context, tree_root_name(&index, file, tree)))
                .or_default() += 1;
        };
        match *tag {
            VALDEF => {
                if let Some((tree, _)) = below(*at).first() {
                    record("VALDEF type tree", *tree);
                }
            }
            PARAM => {
                if let Some((tree, _)) = below(*at).first() {
                    record("PARAM type tree", *tree);
                }
            }
            TYPEPARAM => {
                if let Some((tree, _)) = below(*at).first() {
                    record("TYPEPARAM bounds tree", *tree);
                }
            }
            TYPEDEF => match below(*at).first() {
                Some((template, tag)) if *tag == TEMPLATE => {
                    // Parents come after the parameters, before the self
                    // definition and the statements.
                    let mut parents = below(*template)
                        .iter()
                        .skip_while(|(_, tag)| matches!(*tag, TYPEPARAM | PARAM));
                    for (parent, tag) in parents.by_ref() {
                        if matches!(*tag, SELFDEF | VALDEF | TYPEDEF | DEFDEF) {
                            break;
                        }
                        record("class parent tree", *parent);
                    }
                    for (self_def, tag) in below(*template) {
                        if *tag == SELFDEF
                            && let Some((tree, _)) = below(*self_def).first()
                        {
                            record("SELFDEF type tree", *tree);
                        }
                    }
                }
                Some((rhs, _)) => record("TYPEDEF rhs tree (non-template)", *rhs),
                None => {}
            },
            DEFDEF => {
                if let Some((result, _)) = below(*at)
                    .iter()
                    .find(|(_, tag)| !matches!(*tag, TYPEPARAM | PARAM))
                {
                    record("DEFDEF result tree", *result);
                }
            }
            _ => {}
        }
    }
    // The constructors of every `APPLIEDtpt`.
    for node in index.iter_nodes_with_tag(162) {
        let at = u32::try_from(node.offset).unwrap();
        let Some((tycon, _)) = below(at).first() else {
            continue;
        };
        *survey
            .applied_constructors
            .entry(tree_root_name(&index, file, *tycon))
            .or_default() += 1;
        // A textual name, for the survey only: it decides nothing.
        if let dotty_tasty::tasty::RawTree::NatAst { value, .. } = tree_from(file, *tycon)
            && file
                .names()
                .get_utf8(value)
                .is_some_and(|name| name == "&" || name == "|")
        {
            survey.applied_and_or += 1;
        }
    }

    survey_type_trees(&index, file, &children, survey);
    survey_lambdas(&index, file, &children, unpickler, survey);
    survey_methods(&index, file, &children, unpickler, survey);
    survey_classes(&index, file, &children, unpickler, &mut survey.classes);

    // Completion itself: every simple symbol first, in document order (so the
    // simple-symbol figures stay comparable), then the methods.
    definitions.sort_by_key(|(at, tag)| (*tag == DEFDEF, *at));
    for (at, tag) in definitions {
        let Some((kind, before)) = unpickler.symbol_state_at(at) else {
            continue;
        };
        let class_like = tag == TYPEDEF
            && matches!(
                kind,
                dotty_core::symbols::SymbolKind::Class
                    | dotty_core::symbols::SymbolKind::Trait
                    | dotty_core::symbols::SymbolKind::ModuleClass
            );
        if class_like && !complete_classes {
            continue;
        }
        let name = format!("{kind:?} ({})", full_root_name_of_definition(tag));
        let outcomes = survey.outcomes.entry(name).or_default();
        outcomes.entered += 1;
        if matches!(before, dotty_core::symbols::SymbolInfo::Complete(_)) {
            outcomes.already_complete += 1;
            continue;
        }
        let trees = new_trees_below(&children, below(at).first().copied());
        let result = unpickler.complete_symbol(at);
        if class_like {
            record_class_outcome(&result, &children, at, kind, unpickler, &mut survey.classes);
        }
        let bucket = match result {
            Ok(_) => "completed",
            Err(
                UnpickleError::UnresolvedPackage { .. } | UnpickleError::UnresolvedMember { .. },
            ) => "external child failure",
            Err(UnpickleError::MissingReferencedSymbol { .. }) => "local missing reference",
            Err(UnpickleError::UnsupportedTypeTree { tag, .. }) => {
                *survey.unsupported_trees.entry(tag).or_default() += 1;
                "unsupported type tree"
            }
            Err(UnpickleError::UnsupportedType { .. }) => "unsupported type node",
            Err(UnpickleError::UnsupportedTermTree { tag, address }) => {
                *survey.unsupported_terms.entry(tag).or_default() += 1;
                let examples = survey.examples.entry("unsupported term tree").or_default();
                if examples.len() < 3 {
                    examples.push(format!("{label} @{at}: term {address} tag {tag}"));
                }
                "unsupported qualifier or reference term tree"
            }
            Err(UnpickleError::UnstableSelectQualifier { .. }) => "unstable select qualifier",
            Err(UnpickleError::InvalidSingletonTypeTree { .. }) => "invalid singleton",
            Err(UnpickleError::OpaqueAliasDeferred { .. }) => "opaque alias",
            Err(UnpickleError::ConstructorOwnerNotClassLike { .. }) => {
                "constructor owner not class-like"
            }
            Err(UnpickleError::MalformedOwnerClassInfo { .. }) => {
                "constructor owner ClassInfo malformed"
            }
            Err(UnpickleError::UnsupportedMethodParameterSemantics { .. }) => {
                "unsupported parameter semantics"
            }
            Err(UnpickleError::MalformedDefinition { .. }) => "clause malformed",
            Err(UnpickleError::SharedLambdaOwnerConflict { .. }) => "shared lambda owner conflict",
            Err(UnpickleError::SharedRefinementOwnerConflict { .. }) => {
                "shared refinement owner conflict"
            }
            Err(UnpickleError::UnsupportedRefinementStat { tag, .. }) => {
                *survey.unsupported_trees.entry(tag).or_default() += 1;
                "unsupported refinement stat"
            }
            Err(
                UnpickleError::MissingRefinementClass { .. }
                | UnpickleError::InvalidRefinementClass { .. }
                | UnpickleError::MissingRefinementScope { .. },
            ) => "refinement semantic-state error",
            Err(UnpickleError::MalformedRefinedTypeTree { .. }) => "malformed refined type tree",
            Err(UnpickleError::UnsupportedRefinementOverload { .. }) => {
                "unsupported refinement overload"
            }
            Err(UnpickleError::CloseOverThis { .. }) => "close-over-this failed",
            Err(UnpickleError::ParameterAbstraction { .. }) => "abstraction failed",
            Err(UnpickleError::UnsupportedSymbolCompletion { .. }) => "kind deferred (5d)",
            Err(UnpickleError::MissingClassScope { .. }) => "missing class scope",
            Err(UnpickleError::UnsupportedParentTree { .. }) => "unsupported parent wrapper",
            Err(
                UnpickleError::MalformedParentTree { .. }
                | UnpickleError::InvalidSelfTypeTree { .. },
            ) => "malformed template",
            Err(UnpickleError::InvalidCompletedBounds { .. }) => "invalid bounds",
            // An annotation inside the type (4b's deferrals).
            Err(
                UnpickleError::UnsupportedAnnotationTree { .. }
                | UnpickleError::UnsupportedAnnotationConstructor { .. }
                | UnpickleError::UnsupportedAnnotationArgument { .. },
            ) => "deferred annotation",
            Err(
                UnpickleError::UnsupportedResolutionPrefix { address, .. }
                | UnpickleError::UnsupportedResolutionSpace { address, .. },
            ) => {
                let examples = survey.examples.entry("prefix unsupported").or_default();
                if examples.len() < 4 {
                    examples.push(format!("{label} @{at}: reference {address}"));
                }
                "dependency prefix unsupported or still Missing"
            }
            Err(
                UnpickleError::AmbiguousMember { .. }
                | UnpickleError::UnsupportedSignedReference { .. }
                | UnpickleError::IllegalTypePrefix { .. },
            ) => "other known",
            Err(UnpickleError::MalformedType { .. } | UnpickleError::Ast(_)) => "malformed",
            Err(other) => {
                survey.unexpected.push(format!("{label} @{at}: {other:?}"));
                "unexpected"
            }
        };
        *outcomes.by_bucket.entry(bucket).or_default() += 1;
        if bucket == "completed" {
            *survey
                .completed_by_new_trees
                .entry((format!("{kind:?}"), trees.clone()))
                .or_default() += 1;
        }
        if trees != "-" {
            *survey
                .outcomes_by_new_trees
                .entry((trees, bucket))
                .or_default() += 1;
        }
    }
    // The info distribution after completion.
    let index = file.ast_address_index().unwrap();
    for node in index.iter_nodes() {
        let at = u32::try_from(node.offset).unwrap();
        if let Some((kind, info)) = unpickler.symbol_state_at(at) {
            *survey
                .infos
                .entry((format!("{kind:?}"), info_state(info)))
                .or_default() += 1;
        }
    }
}

/// The parts of the class at `class_at`, on the wire alone: its template, the
/// header parameter nodes, the parent trees and the `SELFDEF`. This applies
/// `decode_template_structure`'s rule (independently of the crate's own
/// splitting): leading parameters, then parents and the self definition, then
/// statements.
struct WireClass {
    params: Vec<u32>,
    parents: Vec<u32>,
    self_def: Option<u32>,
}

fn wire_class(children: &HashMap<u32, Vec<(u32, u8)>>, class_at: u32) -> Option<WireClass> {
    use dotty_tasty::tasty::{EXPORT_TAG, IMPORT_TAG};
    const TEMPLATE: u8 = 156;
    const SELFDEF: u8 = 118;
    let (template, tag) = children.get(&class_at)?.first().copied()?;
    if tag != TEMPLATE {
        return None;
    }
    let mut parts = WireClass {
        params: Vec::new(),
        parents: Vec::new(),
        self_def: None,
    };
    let (mut past_header, mut in_stats) = (false, false);
    for (at, tag) in children.get(&template).map_or(&[][..], Vec::as_slice) {
        if matches!(*tag, 133 | 134) {
            parts.params.push(*at);
            in_stats |= past_header;
            continue;
        }
        past_header = true;
        if in_stats {
            continue;
        }
        match *tag {
            SELFDEF => parts.self_def = Some(*at),
            128..=131 => in_stats = true,
            tag if tag == IMPORT_TAG || tag == EXPORT_TAG => in_stats = true,
            _ => parts.parents.push(*at),
        }
    }
    Some(parts)
}

/// Every node at or below `root`.
fn subtree(children: &HashMap<u32, Vec<(u32, u8)>>, root: u32) -> HashSet<u32> {
    let mut found = HashSet::new();
    let mut stack = vec![root];
    while let Some(at) = stack.pop() {
        if found.insert(at) {
            stack.extend(
                children
                    .get(&at)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(|(child, _)| *child),
            );
        }
    }
    found
}

/// The address a failure names, when it names one.
fn error_address(error: &UnpickleError) -> Option<u32> {
    match error {
        UnpickleError::UnresolvedMember { address, .. }
        | UnpickleError::UnresolvedPackage { address, .. }
        | UnpickleError::UnsupportedTypeTree { address, .. }
        | UnpickleError::UnsupportedTermTree { address, .. }
        | UnpickleError::UnsupportedType { address, .. }
        | UnpickleError::UnsupportedResolutionPrefix { address, .. }
        | UnpickleError::UnsupportedResolutionSpace { address, .. }
        | UnpickleError::SharedLambdaOwnerConflict { address }
        | UnpickleError::UnsupportedParentTree { address, .. }
        | UnpickleError::MalformedParentTree { address, .. }
        | UnpickleError::InvalidSelfTypeTree { address, .. }
        | UnpickleError::MalformedType { address, .. } => Some(*address),
        UnpickleError::MissingReferencedSymbol { from, .. } => Some(*from),
        _ => None,
    }
}

/// Records how one class completion ended: the failing part and reason, or
/// the completed class for the later field checks.
fn record_class_outcome(
    result: &Result<dotty_core::ids::TypeId, UnpickleError>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    at: u32,
    kind: dotty_core::symbols::SymbolKind,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    survey: &mut ClassSurvey,
) {
    let kind = format!("{kind:?}");
    let wire = wire_class(children, at);
    match result {
        Ok(info) => {
            let symbol = unpickler.index().symbol_at(at).unwrap();
            survey.pending.push(CompletedClass {
                kind,
                symbol,
                info: *info,
                scope: unpickler.index().scope_of(symbol),
                wire_parents: wire.as_ref().map_or(0, |wire| wire.parents.len()),
                wire_self: wire.as_ref().is_some_and(|wire| wire.self_def.is_some()),
            });
        }
        Err(error) => {
            let phase = match (error_address(error), &wire) {
                (Some(address), Some(wire)) => {
                    let within = |roots: &[u32]| {
                        roots
                            .iter()
                            .any(|root| subtree(children, *root).contains(&address))
                    };
                    if within(&wire.params) {
                        "header parameter"
                    } else if within(&wire.parents) {
                        "parent"
                    } else if wire
                        .self_def
                        .is_some_and(|self_def| subtree(children, self_def).contains(&address))
                    {
                        "self type"
                    } else {
                        "elsewhere (a shared link's target)"
                    }
                }
                (None, _) => "no address",
                (Some(_), None) => "not a template",
            };
            let failure = match error {
                UnpickleError::UnsupportedTypeTree { tag, .. } => {
                    format!("UnsupportedTypeTree({})", full_root_name(*tag))
                }
                other => format!("{other:?}")
                    .split([' ', '{'])
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            };
            match error {
                UnpickleError::UnresolvedMember { name, .. } => {
                    *survey
                        .external_names
                        .entry(format!("member {name}"))
                        .or_default() += 1;
                }
                UnpickleError::UnresolvedPackage { package, .. } => {
                    *survey
                        .external_names
                        .entry(format!("package {package}"))
                        .or_default() += 1;
                }
                _ => {}
            }
            if let Some(symbol) = unpickler.index().symbol_at(at) {
                let detail = match error {
                    UnpickleError::UnresolvedMember { name, .. } => format!("member {name}"),
                    UnpickleError::UnresolvedPackage { package, .. } => {
                        format!("package {package}")
                    }
                    _ => String::new(),
                };
                survey
                    .failure_of
                    .insert(symbol, format!("{phase}: {failure} {detail}"));
            }
            *survey.failures.entry((kind, phase, failure)).or_default() += 1;
        }
    }
}

/// Surveys the parents and the self definition of every class-like symbol on
/// the wire, whatever completion later makes of them.
fn survey_classes(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    unpickler: &mut TastyUnpickler<'_, '_, '_>,
    survey: &mut ClassSurvey,
) {
    use dotty_core::symbols::SymbolKind;
    for node in index.iter_nodes_with_tag(131) {
        let at = u32::try_from(node.offset).unwrap();
        let Some((kind, _)) = unpickler.symbol_state_at(at) else {
            continue;
        };
        if !matches!(
            kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            continue;
        }
        let Some(wire) = wire_class(children, at) else {
            continue;
        };
        let kind = format!("{kind:?}");
        *survey
            .parent_counts
            .entry((kind.clone(), wire.parents.len()))
            .or_default() += 1;
        for parent in &wire.parents {
            *survey
                .parent_roots
                .entry((kind.clone(), tree_root_name(index, file, *parent)))
                .or_default() += 1;
            let mut spine = Vec::new();
            describe_parent(index, file, children, *parent, false, &mut spine, survey);
            *survey.spines.entry(spine.join(">")).or_default() += 1;
        }
        if let Some(self_def) = wire.self_def {
            let mut record = |what: &'static str, value: String| {
                *survey.selfs.entry((kind.clone(), what, value)).or_default() += 1;
            };
            record("SELFDEF", String::new());
            if let Some((tree, tag)) = children.get(&self_def).and_then(|below| below.first()) {
                record("root", tree_root_name(index, file, *tree));
                record("5b trees", new_trees_below(children, Some((*tree, *tag))));
                let lambdas = subtree(children, *tree)
                    .iter()
                    .filter(|at| index.get_node(**at).is_some_and(|node| node.tag == 171))
                    .count();
                record("nested LAMBDAtpt", lambdas.to_string());
                let projection = match unpickler.unpickle_type_tree_type(*tree) {
                    Ok(_) => "decoded",
                    Err(
                        UnpickleError::UnresolvedMember { .. }
                        | UnpickleError::UnresolvedPackage { .. },
                    ) => "external",
                    Err(UnpickleError::UnsupportedTypeTree { .. }) => "deferred",
                    Err(_) => "other",
                };
                record("projection", projection.to_owned());
            }
        }
    }
}

/// Follows one parent tree the way `readParentType` does, on the wire alone,
/// recording the spine it reads.
fn describe_parent(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    at: u32,
    under_type_apply: bool,
    spine: &mut Vec<String>,
    survey: &mut ClassSurvey,
) {
    if spine.len() > 32 {
        spine.push("(too deep)".to_owned());
        return;
    }
    let Some((_, at, tag, _)) = follow_shared_terms_in(index, file, at, 16) else {
        spine.push("SHAREDterm (invalid)".to_owned());
        return;
    };
    let below = children.get(&at).map_or(&[][..], Vec::as_slice);
    match tag {
        136 | 140 | 137 => {
            let name = match tag {
                136 => "APPLY",
                140 => "BLOCK",
                _ => "TYPEAPPLY",
            };
            spine.push(name.to_owned());
            match tag {
                136 => {
                    *survey
                        .argument_counts
                        .entry(below.len().saturating_sub(1))
                        .or_default() += 1;
                }
                140 => {
                    *survey
                        .blocks
                        .entry(format!("{} statements", below.len().saturating_sub(1)))
                        .or_default() += 1;
                }
                _ => {}
            }
            if let Some((first, _)) = below.first() {
                describe_parent(
                    index,
                    file,
                    children,
                    *first,
                    under_type_apply || tag == 137,
                    spine,
                    survey,
                );
            }
        }
        176 => {
            spine.push("SELECTin".to_owned());
            match below.first() {
                Some((new, 95)) => {
                    spine.push("NEW".to_owned());
                    if let Some((tpt, _)) = children.get(new).and_then(|below| below.first()) {
                        let root = tree_root_name(index, file, *tpt);
                        if under_type_apply {
                            let applied = root.ends_with("APPLIEDtpt");
                            *survey
                                .type_apply
                                .entry(if applied {
                                    "NEW type already applied (arguments skipped)"
                                } else {
                                    "NEW type bare (arguments applied)"
                                })
                                .or_default() += 1;
                        }
                        spine.push(root);
                    }
                }
                _ => spine.push("(qualifier is not NEW)".to_owned()),
            }
        }
        other => {
            let root = full_root_name(other);
            *survey.direct_roots.entry(root.clone()).or_default() += 1;
            spine.push(root);
        }
    }
}

/// Which of the 5b trees (`SELECTtpt`, `ANNOTATEDtpt`, `SINGLETONtpt`) occur
/// at or below `root` (links not followed), as a stable label; `-` for none.
fn new_trees_below(children: &HashMap<u32, Vec<(u32, u8)>>, root: Option<(u32, u8)>) -> String {
    let Some((root, root_tag)) = root else {
        return "-".to_owned();
    };
    let mut found = std::collections::BTreeSet::new();
    let mut note = |tag: u8| match tag {
        113 => {
            found.insert("SELECTtpt");
        }
        154 => {
            found.insert("ANNOTATEDtpt");
        }
        101 => {
            found.insert("SINGLETONtpt");
        }
        _ => {}
    };
    note(root_tag);
    let mut stack = vec![root];
    while let Some(at) = stack.pop() {
        for (child, tag) in children.get(&at).map_or(&[][..], Vec::as_slice) {
            note(*tag);
            stack.push(*child);
        }
    }
    if found.is_empty() {
        "-".to_owned()
    } else {
        found.into_iter().collect::<Vec<_>>().join("+")
    }
}

/// Every `DEFDEF` of the unit (Milestone 5c): its clause shape, parameter
/// modifiers, and how its signature refers to its own parameters. Constructors
/// are surveyed (and reported apart) but never completed.
fn survey_methods(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    survey: &mut CompletionSurvey,
) {
    use dotty_tasty::tasty::{DefDefHeaderItem, DefinitionTail, ParameterNode};
    const TYPEPARAM: u8 = 133;
    const PARAM: u8 = 134;
    const EMPTYCLAUSE: u8 = 45;
    let below = |at: u32| children.get(&at).map_or(&[][..], Vec::as_slice);
    let add =
        |survey: &mut CompletionSurvey, kind: &'static str, what: &'static str, value: String| {
            *survey.methods.entry((kind, what, value)).or_default() += 1;
        };
    // The enclosing template's type-parameter count, for constructors.
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    for edge in index.iter_tree_edges() {
        parent_of.insert(
            u32::try_from(edge.child.offset).unwrap(),
            u32::try_from(edge.parent.offset).unwrap(),
        );
    }

    for node in index.iter_nodes_with_tag(130) {
        let at = u32::try_from(node.offset).unwrap();
        let Some((symbol_kind, _)) = unpickler.symbol_state_at(at) else {
            continue;
        };
        let kind: &'static str = match symbol_kind {
            dotty_core::symbols::SymbolKind::Method => "Method",
            dotty_core::symbols::SymbolKind::Constructor => "Constructor",
            _ => continue,
        };
        let Some(raw) = index.get(at) else { continue };
        let Ok(body) = raw.decode_defdef_body() else {
            add(survey, kind, "undecodable", String::new());
            continue;
        };
        add(survey, kind, "total", String::new());

        // The clauses, as upstream `readParamss` reads them: consecutive
        // parameters of one tag are a clause, `EMPTYCLAUSE` an empty term
        // clause, `SPLITCLAUSE` a boundary.
        let mut clauses: Vec<(u8, Vec<&ParameterNode<'_>>)> = Vec::new();
        let (mut empties, mut splits) = (0usize, 0usize);
        let mut open: Option<u8> = None;
        for item in &body.header_items {
            match item {
                DefDefHeaderItem::Parameter(parameter) => {
                    let tag = parameter.tag();
                    match (open, clauses.last_mut()) {
                        (Some(current), Some(last)) if current == tag => last.1.push(parameter),
                        _ => clauses.push((tag, vec![parameter])),
                    }
                    open = Some(tag);
                }
                DefDefHeaderItem::Clause(EMPTYCLAUSE) => {
                    empties += 1;
                    clauses.push((EMPTYCLAUSE, Vec::new()));
                    open = None;
                }
                DefDefHeaderItem::Clause(_) => {
                    splits += 1;
                    open = None;
                }
            }
        }
        let modifiers = |parameter: &ParameterNode<'_>| -> Vec<u8> {
            parameter
                .decode_body()
                .map(|body| {
                    body.tail
                        .iter()
                        .filter_map(|entry| match entry {
                            DefinitionTail::Modifier(tag) => Some(*tag),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let term_clauses = clauses.iter().filter(|c| c.0 != TYPEPARAM).count();
        let type_clauses = clauses.iter().filter(|c| c.0 == TYPEPARAM).count();
        if clauses.is_empty() {
            add(survey, kind, "no parameter clauses", String::new());
        }
        if empties > 0 {
            add(survey, kind, "explicit empty clauses", empties.to_string());
        }
        add(survey, kind, "term clauses", term_clauses.to_string());
        add(survey, kind, "type clauses", type_clauses.to_string());
        add(survey, kind, "clause depth", clauses.len().to_string());
        add(survey, kind, "SPLITCLAUSE", splits.to_string());
        let sequence: Vec<&str> = clauses
            .iter()
            .map(|(tag, params)| match *tag {
                TYPEPARAM => "T",
                EMPTYCLAUSE => "()",
                _ => match params.first().map(|p| modifiers(p)) {
                    Some(mods) if mods.contains(&37) => "using",
                    Some(mods) if mods.contains(&13) => "implicit",
                    _ => "P",
                },
            })
            .collect();
        add(survey, kind, "clause sequence", sequence.join(" "));
        let widest = clauses.iter().map(|c| c.1.len()).max().unwrap_or(0);
        add(
            survey,
            kind,
            "max parameters in a clause",
            widest.to_string(),
        );
        // Mixed given/implicit inside one clause.
        for (tag, params) in &clauses {
            if *tag == PARAM && !params.is_empty() {
                let flags: Vec<(bool, bool)> = params
                    .iter()
                    .map(|p| {
                        let mods = modifiers(p);
                        (mods.contains(&37), mods.contains(&13))
                    })
                    .collect();
                if flags.iter().any(|f| *f != flags[0]) {
                    add(
                        survey,
                        kind,
                        "clause with mixed given/implicit",
                        String::new(),
                    );
                }
            }
            for parameter in params {
                for tag in modifiers(parameter) {
                    add(survey, kind, "parameter modifier tag", tag.to_string());
                }
            }
        }
        // AstView lists the parameter nodes (clause markers are bare tags,
        // not nodes), then the result type tree.
        let kids = below(at);
        let header = body
            .header_items
            .iter()
            .filter(|item| matches!(item, DefDefHeaderItem::Parameter(_)))
            .count();
        if let Some((result, _)) = kids.get(header) {
            add(
                survey,
                kind,
                "result root",
                tree_root_name(index, file, *result),
            );
        }
        // Repeated parameters (`T*` is an applied `<repeated>`), and own
        // parameter references.
        let mut addresses: Vec<(u32, u8, usize)> = Vec::new(); // (address, tag, clause)
        {
            let mut clause = 0usize;
            let mut previous: Option<u8> = None;
            let mut next = kids.iter();
            for item in &body.header_items {
                match item {
                    DefDefHeaderItem::Parameter(parameter) => {
                        let tag = parameter.tag();
                        if previous.is_some() && previous != Some(tag) {
                            clause += 1;
                        }
                        if let Some((child, _)) = next.next() {
                            addresses.push((*child, tag, clause));
                        }
                        previous = Some(tag);
                    }
                    DefDefHeaderItem::Clause(EMPTYCLAUSE) => {
                        clause += usize::from(previous.is_some()) + 1;
                        previous = None;
                    }
                    DefDefHeaderItem::Clause(_) => {
                        clause += usize::from(previous.is_some());
                        previous = None;
                    }
                }
            }
        }
        let own: HashMap<u32, (u8, usize)> =
            addresses.iter().map(|(a, t, c)| (*a, (*t, *c))).collect();
        for (param, tag, _) in &addresses {
            if *tag == PARAM
                && let Some((tree, 162)) = below(*param).first().copied()
                && let Some((tycon, _)) = below(tree).first()
                && let dotty_tasty::tasty::RawTree::NatAst { value, .. } = tree_from(file, *tycon)
                && file.names().get_utf8(value) == Some("<repeated>")
            {
                add(survey, kind, "repeated parameter", String::new());
            }
        }
        // Where do references to own parameters occur? (target tag, place)
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let scan = |root: u32,
                    place: &str,
                    from_clause: usize,
                    seen: &mut std::collections::BTreeSet<String>| {
            let mut stack = vec![root];
            while let Some(current) = stack.pop() {
                let tag = index.get_node(current).map(|n| n.tag);
                let target = match (tag, tree_from(file, current)) {
                    (Some(62 | 63), dotty_tasty::tasty::RawTree::Leaf(term)) => match term.value {
                        dotty_tasty::tasty::TermValue::AstRef(target) => Some(target),
                        _ => None,
                    },
                    (Some(114 | 116), dotty_tasty::tasty::RawTree::NatAst { value, .. }) => {
                        Some(value)
                    }
                    _ => None,
                };
                if let Some(target) = target
                    && let Some((target_tag, target_clause)) = own.get(&target)
                {
                    let what = if *target_tag == TYPEPARAM {
                        "type parameter"
                    } else {
                        "term parameter"
                    };
                    seen.insert(format!("{what} referenced in {place}"));
                    if *target_clause < from_clause {
                        seen.insert("reference across clauses".to_owned());
                    }
                    if target == root {
                        seen.insert("self".to_owned());
                    }
                }
                for (child, _) in below(current) {
                    stack.push(*child);
                }
            }
        };
        for (param, tag, clause) in &addresses {
            let place = if *tag == TYPEPARAM {
                "type parameter bounds"
            } else {
                "parameter type"
            };
            if let Some((tree, _)) = below(*param).first() {
                scan(*tree, place, *clause, &mut seen);
            }
        }
        if let Some((result, _)) = kids.get(header) {
            scan(*result, "result type", clauses.len(), &mut seen);
        }
        if seen.is_empty() {
            add(survey, kind, "own-parameter references", "none".to_owned());
        } else {
            add(survey, kind, "own-parameter references", "some".to_owned());
        }
        for what in seen {
            add(survey, kind, "own reference", what);
        }
        // Constructors: the class's own arity.
        if kind == "Constructor"
            && let Some(template) = parent_of.get(&at)
        {
            let arity = below(*template)
                .iter()
                .filter(|(_, t)| *t == TYPEPARAM)
                .count();
            add(
                survey,
                kind,
                "owner class type parameters",
                arity.to_string(),
            );
        }
    }
}

/// The `LAMBDAtpt` roots of the unit (Milestone 5c): where they sit, their
/// shape, and whether pass 1 entered their parameters for one owner.
fn survey_lambdas(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    unpickler: &TastyUnpickler<'_, '_, '_>,
    survey: &mut CompletionSurvey,
) {
    const LAMBDATPT: u8 = 171;
    const TYPEPARAM: u8 = 133;
    let mut parents: HashMap<u32, (u32, u8)> = HashMap::new();
    for edge in index.iter_tree_edges() {
        parents.insert(
            u32::try_from(edge.child.offset).unwrap(),
            (u32::try_from(edge.parent.offset).unwrap(), edge.parent.tag),
        );
    }
    let below = |at: u32| children.get(&at).map_or(&[][..], Vec::as_slice);
    let add = |survey: &mut CompletionSurvey, what: &'static str, value: String| {
        *survey.lambdas.entry((what, value)).or_default() += 1;
    };
    for node in index.iter_nodes_with_tag(LAMBDATPT) {
        let at = u32::try_from(node.offset).unwrap();
        add(survey, "total", String::new());
        // Nearest definition above, and whether a lambda sits in between.
        let (mut current, mut in_lambda, mut context) = (at, false, "none");
        while let Some((parent, tag)) = parents.get(&current) {
            match *tag {
                LAMBDATPT => in_lambda = true,
                129 | 130 | 131 | 133 | 134 if context == "none" => {
                    context = match *tag {
                        129 => "VALDEF",
                        130 => "DEFDEF",
                        131 => "TYPEDEF",
                        133 => "TYPEPARAM",
                        _ => "PARAM",
                    };
                }
                _ => {}
            }
            current = *parent;
        }
        add(
            survey,
            "context",
            format!("{context}{}", if in_lambda { " (nested)" } else { "" }),
        );
        let kids = below(at);
        let params: Vec<u32> = kids
            .iter()
            .filter(|(_, tag)| *tag == TYPEPARAM)
            .map(|(at, _)| *at)
            .collect();
        add(survey, "type parameters", params.len().to_string());
        if let Some((body, _)) = kids.last() {
            add(survey, "body root", tree_root_name(index, file, *body));
        }
        for param in &params {
            let inside = below(*param);
            if let Some((bounds, _)) = inside.first() {
                add(survey, "bounds root", tree_root_name(index, file, *bounds));
            }
            for (_, tag) in inside.iter().skip(1) {
                add(survey, "type parameter modifier tag", tag.to_string());
            }
        }
        // References among the lambda's own parameters (by address).
        let mut refers_to_own = false;
        let mut stack: Vec<u32> = kids.iter().map(|(at, _)| *at).collect();
        while let Some(current) = stack.pop() {
            for (child, tag) in below(current) {
                stack.push(*child);
                if matches!(*tag, 63 | 62)
                    && let dotty_tasty::tasty::RawTree::Leaf(term) = tree_from(file, *child)
                    && let dotty_tasty::tasty::TermValue::AstRef(target) = term.value
                    && params.contains(&target)
                {
                    refers_to_own = true;
                }
            }
        }
        add(
            survey,
            "refers to its own parameters",
            refers_to_own.to_string(),
        );
        add(
            survey,
            "owner entered",
            unpickler.index().lambda_owner(at).is_some().to_string(),
        );
        add(
            survey,
            "owner conflict",
            unpickler.index().has_lambda_owner_conflict(at).to_string(),
        );
    }
    // How many `SHAREDterm` links end at a lambda.
    for node in index.iter_nodes_with_tag(60) {
        let at = u32::try_from(node.offset).unwrap();
        if let Some((_, target, tag, _)) = follow_shared_terms_in(index, file, at, 16)
            && tag == LAMBDATPT
        {
            let _ = target;
            add(survey, "SHAREDterm links to a lambda", String::new());
        }
    }
}

/// The wire shapes of the trees Milestone 5b projects, over every node of the
/// unit and whatever they decode to.
fn survey_type_trees(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    children: &HashMap<u32, Vec<(u32, u8)>>,
    survey: &mut CompletionSurvey,
) {
    const SELECTTPT: u8 = 113;
    const SINGLETONTPT: u8 = 101;
    const ANNOTATEDTPT: u8 = 154;
    let below = |at: u32| children.get(&at).map_or(&[][..], Vec::as_slice);
    let record = |survey: &mut CompletionSurvey, what: &'static str, tree: u32| {
        *survey
            .tpt_roots
            .entry((what, tree_root_name(index, file, tree)))
            .or_default() += 1;
        if let Some((links, ..)) = follow_shared_terms_in(index, file, tree, 16) {
            *survey.tpt_links.entry((what, links)).or_default() += 1;
        }
    };
    for node in index.iter_nodes_with_tag(SELECTTPT) {
        let at = u32::try_from(node.offset).unwrap();
        if let Some((qualifier, _)) = below(at).first() {
            record(survey, "SELECTtpt qualifier", *qualifier);
        }
    }
    for node in index.iter_nodes_with_tag(SINGLETONTPT) {
        let at = u32::try_from(node.offset).unwrap();
        if let Some((reference, _)) = below(at).first() {
            record(survey, "SINGLETONtpt ref", *reference);
        }
    }
    for node in index.iter_nodes_with_tag(ANNOTATEDTPT) {
        let at = u32::try_from(node.offset).unwrap();
        let [(base, _), (annotation, _)] = below(at) else {
            continue;
        };
        record(survey, "ANNOTATEDtpt base", *base);
        record(survey, "ANNOTATEDtpt annotation", *annotation);
        if let Some((_, target, tag, _)) = follow_shared_terms_in(index, file, *annotation, 16)
            && matches!(tag, 136 | 95)
        {
            survey_shape(&mut survey.annotated_tpt_shapes, file, children, target);
        }
    }
}

fn full_root_name_of_definition(tag: u8) -> &'static str {
    match tag {
        129 => "VALDEF",
        131 => "TYPEDEF",
        130 => "DEFDEF",
        133 => "TYPEPARAM",
        134 => "PARAM",
        _ => "?",
    }
}

/// Files one `ANNOTATEDtype` root under its wire shape and its outcome.
/// `head` is the first tag of the annotation payload; `parent_failed` says the
/// parent, decoded on its own first, did not decode.
fn record_annotation(
    survey: &mut AnnotationSurvey,
    head: Option<u8>,
    parent_failed: bool,
    shared_target: Option<u8>,
    result: &Result<dotty_core::ids::TypeId, UnpickleError>,
) {
    let Some(head) = head else { return };
    if let Some(tag) = shared_target {
        let entry = survey.shared.outcomes_by_target.entry(tag).or_default();
        entry.0 += 1;
        entry.1 += usize::from(result.is_ok());
    }
    if !is_compact_annot_type_tag(head) {
        let full = survey.full.entry(head).or_default();
        full.total += 1;
        match result {
            Ok(_) => full.decoded += 1,
            Err(_) if parent_failed => full.parent_failed += 1,
            Err(UnpickleError::UnsupportedAnnotationTree { .. }) => full.deferred_tree += 1,
            Err(UnpickleError::InvalidReferenceTarget { .. }) => full.invalid_reference += 1,
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

/// Follows the `SHAREDterm` links from `at` on the wire alone: the number of
/// links followed and the address the chain ends at, or `None` when a link
/// names no node or the chain does not end within `bound` links.
fn follow_shared_terms(
    file: &TastyFile<'_>,
    at: u32,
    bound: usize,
) -> Option<(usize, u32, u8, bool)> {
    follow_shared_terms_in(&file.ast_address_index().unwrap(), file, at, bound)
}

/// [`follow_shared_terms`] over an index built once by the caller.
fn follow_shared_terms_in(
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    file: &TastyFile<'_>,
    at: u32,
    bound: usize,
) -> Option<(usize, u32, u8, bool)> {
    use dotty_tasty::tasty::{RawTree, TermValue};
    let mut current = at;
    let mut links = 0;
    let mut link_to_link = false;
    loop {
        let tag = index.get_node(current)?.tag;
        if tag != 60 {
            return Some((links, current, tag, link_to_link));
        }
        let RawTree::Leaf(term) = tree_from(file, current) else {
            return None;
        };
        let TermValue::AstRef(target) = term.value else {
            return None;
        };
        links += 1;
        if links > bound {
            return None;
        }
        link_to_link |= index.get_node(target).is_some_and(|node| node.tag == 60);
        current = target;
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
    completion: Completion,
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

    let match_index = file.ast_address_index().unwrap();
    for node in match_index.iter_nodes_with_tag(MATCHTPT_TAG) {
        let shape = match_index
            .get(u32::try_from(node.offset).unwrap())
            .unwrap()
            .decode_match_tpt()
            .unwrap();
        let survey = &mut tally.match_tpt;
        survey.total += 1;
        survey.with_bound += usize::from(shape.bound.is_some());
        *survey.case_counts.entry(shape.cases.len()).or_default() += 1;
    }
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
    tally.annotations.unit += 1;
    let unit = tally.annotations.unit;
    let mut shared_targets: HashMap<u32, u8> = HashMap::new();
    for (at, (_, annotation_at, head)) in &annotated {
        if *head == 60 {
            let shared = &mut tally.annotations.shared;
            shared.roots += 1;
            match follow_shared_terms(&file, *annotation_at, 16) {
                None => shared.invalid_chains += 1,
                Some((links, target, tag, link_to_link)) => {
                    *shared.depths.entry(links).or_default() += 1;
                    shared.links_to_links += usize::from(link_to_link);
                    *shared.target_tags.entry(tag).or_default() += 1;
                    *shared.per_target.entry((unit, target)).or_default() += 1;
                    shared_targets.insert(*at, tag);
                }
            }
        }
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

    let mut named_roots: Vec<dotty_core::ids::TypeId> = Vec::new();
    let mut decoded_erased_methods: Vec<dotty_core::ids::TypeId> = Vec::new();
    let mut other_methods: Vec<dotty_core::ids::TypeId> = Vec::new();
    let mut unpickler = TastyUnpickler::with_packages(&file, &mut *store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    survey_identity_reachability(&file, &unpickler, label, &mut tally.identity);
    survey_symbol_annotations(&file, &unpickler, &mut tally.symbol_annotations);
    survey_symbol_annotation_completion(
        &file,
        &mut unpickler,
        label,
        &mut tally.symbol_annotation_completion,
    );
    if completion != Completion::Off {
        complete_unit(
            &mut unpickler,
            &file,
            label,
            &mut tally.completion,
            completion == Completion::All,
        );
    }

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
        if matches!(tag, 115 | 117)
            && matches!(
                result,
                Err(UnpickleError::UnsupportedResolutionPrefix { .. })
            )
            && let Some(&[(prefix_at, _)]) = all_children.get(&at).map(Vec::as_slice)
        {
            let kind = if tag == 117 { "TYPEREF" } else { "TERMREF" };
            *tally
                .unsupported_prefix_shapes
                .entry((kind, shape_of(&file, &tags, prefix_at)))
                .or_default() += 1;
        }
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
                shared_targets.get(&at).copied(),
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
                if matches!(tag, 115 | 117) {
                    named_roots.push(first);
                }
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
    for id in named_roots {
        use dotty_core::types::{TermRefTarget, TypeRefTarget};
        let (kind, prefix) = match store.types.get(id) {
            dotty_core::types::Type::TypeRef {
                prefix,
                target: TypeRefTarget::Name(_),
            } => ("TypeRef", *prefix),
            dotty_core::types::Type::TermRef {
                prefix,
                target: TermRefTarget::Name(_),
            } => ("TermRef", *prefix),
            _ => continue,
        };
        let variant = match store.types.get(prefix) {
            dotty_core::types::Type::RecThis { .. } => "RecThis",
            dotty_core::types::Type::Refined { .. } => "Refined",
            dotty_core::types::Type::Recursive { .. } => "Recursive",
            dotty_core::types::Type::Flexible { .. } => "Flexible (proxy)",
            dotty_core::types::Type::Annotated { .. } => "Annotated (proxy)",
            _ => "other",
        };
        *tally.name_targets.entry((kind, variant)).or_default() += 1;
    }
    for (at, tag) in &tags {
        if *tag == 131
            && let Some(symbol) = index.symbol_at(*at)
            && let Some(scope) = index.scope_of(symbol)
        {
            tally.class_scopes.insert(symbol, scope);
        }
    }
    // The completed classes of this unit: `ClassInfo` field sanity, and the
    // semantic shape of every parent.
    for done in std::mem::take(&mut tally.completion.classes.pending) {
        let dotty_core::types::Type::ClassInfo(info) = store.types.get(done.info) else {
            panic!("{label}: a completed class is not a ClassInfo");
        };
        assert_eq!(info.class, done.symbol, "{label}");
        assert_eq!(info.prefix, definitions.no_prefix, "{label}");
        assert_eq!(Some(info.declarations), done.scope, "{label}");
        assert_eq!(
            store.scopes.get(info.declarations).owner,
            Some(done.symbol),
            "{label}"
        );
        assert_eq!(info.parents.len(), done.wire_parents, "{label}");
        assert_eq!(info.self_type.is_some(), done.wire_self, "{label}");
        for parent in &info.parents {
            let shape = match store.types.get(*parent) {
                ty @ dotty_core::types::Type::TypeRef { .. } => {
                    let aliased = ty.reference_symbol().is_some_and(|symbol| {
                        matches!(
                            store.symbols.get(symbol).info,
                            dotty_core::symbols::SymbolInfo::Complete(target)
                                if matches!(
                                    store.types.get(target),
                                    dotty_core::types::Type::AliasingBounds { .. }
                                )
                        )
                    });
                    if aliased { "alias TypeRef" } else { "TypeRef" }
                }
                dotty_core::types::Type::Applied { .. } => "Applied",
                dotty_core::types::Type::Annotated { .. } => "Annotated",
                dotty_core::types::Type::Refined { .. }
                | dotty_core::types::Type::Recursive { .. } => "Refined / Recursive",
                _ => "other",
            };
            *tally
                .completion
                .classes
                .parent_shapes
                .entry((done.kind.clone(), shape))
                .or_default() += 1;
        }
        tally.completion.classes.checked += 1;
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
            Completion::All,
        );
    }

    assert_eq!(tally.completion.unexpected, Vec::<String>::new());
    assert!(tally.units > 20, "found {} units", tally.units);
    assert!(tally.by_address.decoded > 0);
    assert_eq!(tally.unexpected, Vec::<String>::new());
}

/// Enters `scala.Any`, `scala.Nothing` and `scala.Null`, which the compiler
/// defines and no TASTy file declares.
/// The `n` most frequent entries of `counts`.
fn top(counts: &BTreeMap<String, usize>, n: usize) -> Vec<(&String, &usize)> {
    let mut all: Vec<_> = counts.iter().collect();
    all.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    all.truncate(n);
    all
}

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

/// Stub `java.lang.Object` and `scala.AnyRef` classes (the classpath and the
/// compiler supply the real ones), so that a class's implicit parents do not
/// hide what else its completion needs. The primitive classes are not stubbed:
/// the library corpus defines them, later in path order.
fn provide_java_object(store: &mut SemanticStore, packages: &mut Packages) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    let stubs = [
        (&["java", "lang"][..], "Object"),
        (&["scala"][..], "AnyRef"),
    ];
    for (path, class) in stubs {
        let chain = packages.enter(store, SymbolOrigin::Synthetic, path);
        let package = chain.last().unwrap();
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(package.scope).enter(name, symbol);
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
    //
    // The last two flags are Milestone 5d1's: whether class-like symbols are
    // completed (`false` is the ablation, the 5c behavior), and whether a
    // `java.lang.Object` / `scala.AnyRef` stub pair is provided (the corpus has
    // neither, so without them nearly every class parent is external before
    // anything else is reached).
    for (corpus, builtins, complete, classes, object, reverse) in [
        ("scala3-library", false, false, true, false, false),
        ("scala3-compiler", false, false, true, false, false),
        ("scala3-library", true, false, true, false, false),
        ("scala3-compiler", true, false, true, false, false),
        // The same, with every eligible symbol completed first (Milestone 5a).
        ("scala3-library", false, true, true, false, false),
        ("scala3-compiler", false, true, true, false, false),
        ("scala3-library", true, true, true, false, false),
        ("scala3-compiler", true, true, true, false, false),
        // Completion without class-like symbols (the 5c baseline), and with
        // `Object` provided, with and without them.
        ("scala3-library", true, true, false, false, false),
        ("scala3-compiler", true, true, false, false, false),
        ("scala3-library", true, true, true, true, false),
        ("scala3-compiler", true, true, true, true, false),
        ("scala3-library", true, true, false, true, false),
        ("scala3-compiler", true, true, false, true, false),
        // The same completion in the reverse of path order: a class can only
        // benefit from units entered before it, so the order matters, and the
        // two orders bracket what a smarter schedule could reach.
        ("scala3-library", true, true, true, true, true),
        ("scala3-compiler", true, true, true, true, true),
    ] {
        // One store and one package registry per corpus, as a classpath
        // would have.
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        // Declared for every run, with and without completion, so that the
        // only difference between the two is completion itself: it makes the
        // `scala` package exist and `scala.&` / `scala.|` resolve.
        definitions.declare_special_aliases(&mut store, &mut packages);
        if builtins {
            provide_compiler_builtins(&mut store, &mut packages);
        }
        if object {
            provide_java_object(&mut store, &mut packages);
        }
        let mut tally = Tally::default();
        let mut paths = tasty_files(&root.join(corpus));
        if reverse {
            paths.reverse();
        }
        for path in paths {
            let bytes = fs::read(&path).unwrap();
            packages = run(
                &path.display().to_string(),
                &bytes,
                &mut store,
                definitions,
                packages,
                &mut tally,
                match (complete, classes) {
                    (false, _) => Completion::Off,
                    (true, false) => Completion::WithoutClasses,
                    (true, true) => Completion::All,
                },
            );
        }

        let mut oracle = OwnerOracle::default();
        for (space, name, namespace) in &tally.oracle_queries {
            let scope = match store.types.get(*space) {
                ty @ dotty_core::types::Type::TypeRef { .. } => ty
                    .reference_symbol()
                    .and_then(|symbol| tally.class_scopes.get(&symbol)),
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
            // Whether the owner class ended the corpus with a `ClassInfo`
            // anywhere in the shared store (the order-independent bound).
            let owner_completed = store
                .types
                .get(*space)
                .reference_symbol()
                .is_some_and(|symbol| {
                    matches!(
                        store.symbols.get(symbol).info,
                        dotty_core::symbols::SymbolInfo::Complete(info)
                            if matches!(store.types.get(info), dotty_core::types::Type::ClassInfo(_))
                    )
                });
            oracle.owner_completed += usize::from(owner_completed);
            if !owner_completed && let Some(symbol) = store.types.get(*space).reference_symbol() {
                let why = tally
                    .completion
                    .classes
                    .failure_of
                    .get(&symbol)
                    .cloned()
                    .unwrap_or_else(|| "never attempted".to_owned());
                *oracle.owner_failures.entry(why).or_default() += 1;
            }
        }

        let mut unsupported: Vec<_> = tally.unsupported.iter().collect();
        unsupported.sort_by(|a, b| b.1.cmp(a.1));
        println!(
            "== {corpus} ({}{})",
            if builtins {
                "compiler builtins provided"
            } else {
                "no builtins"
            },
            if !complete {
                ""
            } else if classes {
                ", simple symbols and classes completed first"
            } else {
                ", simple symbols completed first, classes NOT completed (ablation)"
            }
        );
        if object {
            println!("java.lang.Object and scala.AnyRef provided (empty stub classes)");
        }
        if reverse {
            println!("units processed in the REVERSE of path order");
        }
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
        println!(
            "  of the REFin queries with an owner scope, the owner class ends the corpus with a ClassInfo: {}",
            oracle.owner_completed
        );
        println!(
            "  owners without a ClassInfo, by why (top 8): {:?}",
            top(&oracle.owner_failures, 8)
        );
        println!(
            "name-designated references created, by kind and prefix variant: {:?}",
            tally.name_targets
        );
        println!(
            "unsupported-prefix name references by prefix wire shape: {:?}",
            tally.unsupported_prefix_shapes
        );
        report("compound types", &COMPOUND_TAGS);
        report("bounds and flexible types", &WRAPPER_TAGS);
        report("match types", &MATCH_TAGS);
        println!(
            "MATCHtpt (a tree, not decoded here): total {}, with explicit bound {}, cases per tree {:?}",
            tally.match_tpt.total, tally.match_tpt.with_bound, tally.match_tpt.case_counts
        );
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
                "  full {} outcomes: total {}, decoded {}, parent failed first {}, external {}, local missing {}, constructor unsupported {}, argument unsupported {}, invalid type {}, malformed {}, other known {}, deferred tree {}, invalid reference {}",
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
                f.invalid_reference,
            );
        }
        let shared = &survey.shared;
        let mut reuse: BTreeMap<usize, usize> = BTreeMap::new();
        for count in shared.per_target.values() {
            *reuse.entry(*count).or_default() += 1;
        }
        println!(
            "  SHAREDterm roots {}: invalid chains {}, link depths {:?}, links whose target is another SHAREDterm {}, final target tags {:?}, outer annotations per target (count -> targets) {:?}, targets reused by more than one annotation {}",
            shared.roots,
            shared.invalid_chains,
            shared.depths,
            shared.links_to_links,
            shared
                .target_tags
                .iter()
                .map(|(tag, count)| (full_root_name(*tag), *count))
                .collect::<Vec<_>>(),
            reuse,
            reuse
                .iter()
                .filter(|(count, _)| **count > 1)
                .map(|(_, n)| n)
                .sum::<usize>(),
        );
        println!(
            "  SHAREDterm outcomes by target tag (total, decoded): {:?}",
            shared
                .outcomes_by_target
                .iter()
                .map(|(tag, counts)| (full_root_name(*tag), *counts))
                .collect::<Vec<_>>()
        );
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
        if complete {
            let survey = &tally.completion;
            println!("simple symbol completion (per symbol kind and definition tag):");
            for (kind, outcomes) in &survey.outcomes {
                println!(
                    "  {kind}: entered {}, already complete {}, {:?}",
                    outcomes.entered, outcomes.already_complete, outcomes.by_bucket
                );
            }
            println!(
                "  SymbolInfo after completion (kind, state -> count): {:?}",
                survey.infos
            );
            println!(
                "  type-tree roots by definition context: {:?}",
                survey.tree_roots
            );
            println!(
                "  unsupported type trees by tag: {:?}",
                survey.unsupported_trees
            );
            println!(
                "  APPLIEDtpt constructors by root: {:?}, named & or | (survey only): {}",
                survey.applied_constructors, survey.applied_and_or
            );
            println!(
                "  5b wire shapes (survey, root -> count): {:?}",
                survey.tpt_roots
            );
            println!(
                "  5b SHAREDterm chain lengths (survey, links -> count): {:?}",
                survey.tpt_links
            );
            let shapes = &survey.annotated_tpt_shapes;
            println!(
                "  ANNOTATEDtpt annotation spines: {:?}, argument counts: {:?}, argument roots: {:?}, named: {}",
                shapes.spines,
                shapes.argument_counts,
                shapes.argument_roots,
                shapes.named_arguments
            );
            println!(
                "  completed symbols by kind and 5b trees in their declared tree: {:?}",
                survey.completed_by_new_trees
            );
            println!(
                "  outcomes of symbols whose tree has 5b trees: {:?}",
                survey.outcomes_by_new_trees
            );
            println!(
                "  unsupported term trees by tag: {:?}, examples: {:?}",
                survey.unsupported_terms, survey.examples
            );
            println!("  LAMBDAtpt survey: {:?}", survey.lambdas);
            println!("  method survey: {:?}", survey.methods);
            let classes = &survey.classes;
            println!(
                "  class completion (5d1): {} ClassInfo values checked (class, no_prefix, exact scope, scope owner, parent count, self)",
                classes.checked
            );
            println!("    failures (kind, phase, error): {:?}", classes.failures);
            println!(
                "    unresolved names by class failures: {:?}",
                top(&classes.external_names, 15)
            );
            println!(
                "    parent count per class (kind, n): {:?}",
                classes.parent_counts
            );
            println!("    parent roots (kind, root): {:?}", classes.parent_roots);
            println!("    parent constructor spines: {:?}", classes.spines);
            println!("    APPLY argument counts: {:?}", classes.argument_counts);
            println!("    TYPEAPPLY parents: {:?}", classes.type_apply);
            println!(
                "    BLOCK parents: {:?}, ordinary roots: {:?}",
                classes.blocks, classes.direct_roots
            );
            println!(
                "    completed parents by semantic shape: {:?}",
                classes.parent_shapes
            );
            println!("    self definitions: {:?}", classes.selfs);
            println!("  completion unexpected: {}", survey.unexpected.len());
            for error in survey.unexpected.iter().take(10) {
                println!("    {error}");
            }
            assert!(survey.unexpected.is_empty());
        }
        {
            let id = &tally.identity;
            println!(
                "identity reachability (Milestone 5d2c review): LAMBDAtpt entered {} (conflicts {}), out of scope {}, unaccounted {}; REFINEDtpt entered {} (conflicts {}), out of scope {}, unaccounted {}",
                id.lambda_entered,
                id.lambda_conflicts,
                id.lambda_out_of_scope,
                id.lambda_unaccounted,
                id.refined_entered,
                id.refined_conflicts,
                id.refined_out_of_scope,
                id.refined_unaccounted,
            );
            println!("  LAMBDAtpt route attribution: {:?}", id.lambda_routes);
            println!("  REFINEDtpt route attribution: {:?}", id.refined_routes);
            if !id.unaccounted_examples.is_empty() {
                println!("  unaccounted examples: {:?}", id.unaccounted_examples);
            }
        }
        {
            let sa = &tally.symbol_annotations;
            println!(
                "symbol annotations (Milestone 5e1 survey): {} ANNOTATION tail entries over {} entered definitions/parameters, {} of them annotated",
                sa.total, sa.definitions, sa.annotated_definitions
            );
            println!("  entries by definition tag: {:?}", sa.by_definition_tag);
            println!(
                "  annotated definitions by tag: {:?}",
                sa.annotated_by_definition_tag
            );
            println!(
                "  annotations-per-definition distribution: {:?}",
                sa.per_definition_distribution
            );
        }
        {
            let sac = &tally.symbol_annotation_completion;
            println!(
                "symbol annotation completion (Milestone 5e1 outcome survey): {} attempted, {} decoded, {} external, {} unsupported constructor, {} unsupported argument, {} invalid type, {} unsupported tree, {} malformed, {} other known, {} unexpected",
                sac.attempted,
                sac.decoded,
                sac.external,
                sac.unsupported_constructor,
                sac.unsupported_argument,
                sac.invalid_type,
                sac.unsupported_tree,
                sac.malformed,
                sac.other_known,
                sac.unexpected.len()
            );
            for error in sac.unexpected.iter().take(10) {
                println!("  {error}");
            }
            assert!(sac.unexpected.is_empty(), "{:?}", sac.unexpected);
            assert_eq!(
                sac.attempted,
                sac.decoded
                    + sac.external
                    + sac.unsupported_constructor
                    + sac.unsupported_argument
                    + sac.invalid_type
                    + sac.unsupported_tree
                    + sac.malformed
                    + sac.other_known
            );
        }
        println!("unexpected errors: {}", tally.unexpected.len());
        for error in tally.unexpected.iter().take(10) {
            println!("  {error}");
        }
        assert!(tally.unexpected.is_empty());
        assert_eq!(
            tally.identity.lambda_unaccounted + tally.identity.refined_unaccounted,
            0,
            "the identity-reachability oracle found a LAMBDAtpt/REFINEDtpt discovery should have entered but did not: {:?}",
            tally.identity.unaccounted_examples
        );
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
        // `NEW` are read, directly or at the end of a `SHAREDterm` chain; any
        // other tree is deferred, never decoded.
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
                + f.deferred_tree
                + f.invalid_reference;
            assert_eq!(f.total, filed, "root {root}");
            if *root == 136 || *root == 95 {
                assert_eq!(f.deferred_tree, 0, "root {root}");
                assert_eq!(f.invalid_reference, 0, "root {root}");
            } else if *root == 60 {
                // A shared root fails to follow exactly when its chain is
                // invalid on the wire (unless its parent failed first).
                assert!(f.invalid_reference <= survey.shared.invalid_chains);
            } else {
                assert_eq!(f.decoded, 0, "root {root}");
            }
        }
        assert_eq!(
            survey.shared.roots,
            survey.shared.invalid_chains + survey.shared.target_tags.values().sum::<usize>()
        );
        // Only a constructor tree at the end of the chain ever decodes.
        for (tag, (_, decoded)) in &survey.shared.outcomes_by_target {
            assert!(*tag == 136 || *tag == 95 || *decoded == 0, "target {tag}");
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
