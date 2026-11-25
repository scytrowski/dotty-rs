#![doc = include_str!("guide.md")]
// Apply this to the library, whose public items are consumer APIs, rather than
// imposing API documentation on the package's integration-test harnesses.
#![deny(missing_docs)]

/// Scala 3 TASTy encoding and structural APIs.
pub use dotty_tasty::tasty;

/// JVM class file decoding and encoding APIs.
pub use dotty_classfile::classfile;

/// Shared compiler foundation (source text, diagnostics, tokens, symbols,
/// types, and phase-indexed trees) for the source parser, classfile loader,
/// TASTy unpickler, and namer/typer.
pub use dotty_core::core;

/// Scala 3 source lexer and contextual scanner.
pub use dotty_lexer as lexer;

/// Scala 3.9.0 source parser infrastructure and parser-facing AST APIs.
pub use dotty_parser as parser;

/// Source declaration naming and semantic indexing.
pub use dotty_namer as namer;

/// Source signature completion and expression typing.
pub use dotty_typer as typer;

/// JVM class planning and emission from typed trees.
pub use dotty_jvm as jvm;

/// Classpath loading APIs, unifying TASTy and class file entries.
pub use dotty_classloader::classloader;

/// Semantic unpickling of TASTy into the shared `dotty-core` model.
pub use dotty_tasty_unpickler::tasty_unpickler;

mod class_header;
pub use class_header::{
    ClassHeaderBindings, ClassHeaderError, ClassHeaderFailure, try_complete_class_header,
};

mod class_header_phase;
pub use class_header_phase::{ClassHeaderAttempt, ClassHeaderPhase, ClassHeaderReport};

mod class_field_signatures;
pub use class_field_signatures::{
    ClassFieldSignatureCompletion, ClassFieldSignatureError, try_complete_class_field_signatures,
};

mod signature_blockage_report;
pub use signature_blockage_report::SignatureBlockageReport;

mod signature_schedule;
pub use signature_schedule::SignatureRunError;

mod signature_dependencies;
pub use signature_dependencies::{
    SignatureAttempt, SignatureBlockage, SignatureCycle, SignatureDependencies, SignaturePhase,
};

mod member_signature_phase;
pub use member_signature_phase::{
    MemberSignatureFailure, MemberSignaturePhase, MemberSignatureReport,
};

mod member_signature_selection;
pub use member_signature_selection::{MemberSelectionError, MemberSignatureSelection};

mod member_signature;
pub use member_signature::{MemberSignatureError, try_complete_member_signature};

mod named_class_sources;
pub use named_class_sources::{NamedClassSources, NamedClassSourcesError};

mod named_source;
pub use named_source::{
    ClassProjection, ClassProjectionError, NameSourceError, NamedClassSource, NamedSource,
};

mod class_preparation;
pub use class_preparation::{ClassPreparation, PreparationDiagnostic};

mod source_parse;
pub use source_parse::{DetachedSource, ParsedSource, SyntaxDiagnostic, parse_source};

mod semantic_diagnostic;
pub use semantic_diagnostic::{SemanticDiagnostic, SemanticIssue, SemanticPhase};

mod reported_typer_error;
pub use reported_typer_error::ReportedTyperError;

mod call_diagnostic;
pub use call_diagnostic::{CallDiagnostic, CallErrorIdentity};

mod source_diagnostic;
pub use source_diagnostic::SourceDiagnostic;
/// Report returned by typer diagnostics.
pub use source_diagnostic::SourceDiagnostic as TypingDiagnostic;
/// Diagnostic returned by class body checking.
pub use source_diagnostic::SourceDiagnostic as ClassBodyDiagnostic;

mod checked_class_bodies;
pub use checked_class_bodies::{CheckedBodyError, CheckedClassBodies};

mod class_body_work;
pub use class_body_work::{BodySkipReason, ClassBodyWork};

mod class_body;
pub use class_body::{
    CheckedClassBody, ClassBodyCheck, ClassBodyError, ClassBodyReport, PartialClassBody,
    check_class_body, type_class_body, type_class_body_with_resolver,
};

/// JVM class plans for simple source classes with methods and private instance fields.
pub mod jvm_class_plan;

/// JVM method plans from canonical source and typed declarations.
pub mod jvm_method_plan;

/// One-session planning for the audited JVM and Scala 3 TASTy class overlap.
pub mod jvm_tasty_plan;

/// Read-only cross-class source/typed identity proofs for JVM/TASTy planning.
pub mod jvm_tasty_cross_class;

/// Callback-scoped proof for canonical constructor-reference field aliases.
pub mod jvm_tasty_constructor_alias;

/// Opt-in emission of a linked `.class` and complete `.tasty` pair.
pub mod jvm_tasty_artifacts;

/// Source declarations available after naming.
pub mod source_declarations;

mod source_declaration_index;
pub use source_declaration_index::{
    SignatureDeclaration, SourceDeclaration, SourceDeclarationIndex, SourceDeclarationIndexError,
};

mod incomplete_calls;
pub use incomplete_calls::{
    ActiveCallSignature, CallSignatureCandidate, CallSignatureCandidates, IncompleteCallIndex,
};

mod source_call_bindings;
pub use source_call_bindings::{
    ResolvedCallArguments, ResolvedDefaultArgument, ResolvedWrittenArgument,
};

mod resolved_call_index;
pub use resolved_call_index::{
    ResolvedCallIndex, ResolvedCallSignature, ResolvedMemberIndex, ResolvedSourceParent,
    SourceSymbolSite,
};

mod source_symbol_info;
pub use source_symbol_info::{
    SourceNamedType, SourceSignatureParameter, SourceSymbolInfo, SourceSymbolSignature,
};

mod source_locals;
mod source_signature_types;
pub use source_signature_types::{MethodTypeRole, ResolvedConstructorType, ResolvedMethodType};
mod source_parameters;
pub use source_parameters::{SourceConstructorParameter, SourceMethodParameter};

mod source_construction;
pub use source_construction::ResolvedSourceConstruction;
mod source_field_access;
pub use source_field_access::{
    FieldAccessKind, ResolvedFieldAccess, ValueAccessKind, selected_field_access,
};

mod source_file_calls;
pub use source_file_calls::{
    ResolvedSourceCall, SourceFileCalls, SourceMemberTarget, SourceMethodTarget,
};

mod typer_error_span;
pub use typer_error_span::{typer_error_span, typer_error_span_with};
