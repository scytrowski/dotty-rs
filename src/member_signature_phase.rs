//! Stateful completion for the selected ordinary-class member signatures.
use std::collections::HashMap;

use crate::core::{Definitions, Packages, SemanticStore, SymbolId, SymbolInfo};
use crate::signature_dependencies::ObservedSignatures;
use crate::{
    MemberSignatureError, MemberSignatureSelection, ReportedTyperError, SignatureAttempt,
    SignatureBlockage, SignatureDependencies, SignaturePhase, SourceDeclaration,
};

/// A direct signature error captured against its original source before the phase ends.
#[derive(Debug)]
pub enum MemberSignatureFailure {
    /// The requested symbol was ineligible or did not match the canonical declaration.
    InvalidMember(SymbolId),
    /// The selected field or method was rejected. The source owning the member
    /// and the actual site of a recursive failure can differ.
    Typing {
        /// Declaring file, enclosing class and kind in the original naming session.
        declaration: SourceDeclaration,
        /// Original typer error, member and validated byte span when available.
        report: Box<ReportedTyperError>,
    },
}

impl MemberSignatureFailure {
    pub(crate) fn capture(
        error: MemberSignatureError,
        sources: &crate::NamedClassSources<'_>,
    ) -> Self {
        match error {
            MemberSignatureError::InvalidMember(id) => Self::InvalidMember(id),
            MemberSignatureError::Typing {
                member,
                declaration,
                error,
            } => Self::Typing {
                declaration,
                report: Box::new(ReportedTyperError::from_class(
                    error,
                    declaration.class,
                    Some(member),
                    |id| sources.file(id).map(|file| file.ast()),
                )),
            },
        }
    }

    /// Moves a captured signature failure into an owned semantic diagnostic.
    /// For an invalid member the same-session index supplies its owner if
    /// known. The requested symbol survives even when outside the index.
    pub fn into_semantic_diagnostic(
        self,
        sources: &crate::NamedClassSources<'_>,
    ) -> crate::SemanticDiagnostic {
        let (owner, issue) = match self {
            Self::Typing {
                declaration,
                report,
            } => (Some(declaration), crate::SemanticIssue::Typing(report)),
            Self::InvalidMember(id) => (
                sources.declarations().get(id),
                crate::SemanticIssue::InvalidMember(id),
            ),
        };
        crate::SemanticDiagnostic {
            phase: crate::SemanticPhase::MemberSignature,
            owner_source: owner.map(|d| d.source),
            class: owner.map(|d| d.class),
            issue,
        }
    }
}

/// Owned classification of members when the caller ends the signature phase.
/// Apply IDs and ranges only to the originating semantic session and source versions.
#[derive(Debug)]
pub struct MemberSignatureReport {
    /// Complete signatures, including recursively completed members, in selection order.
    pub ready: Vec<SymbolId>,
    /// One owned direct error per rejected symbol, in observation order.
    pub failures: Vec<MemberSignatureFailure>,
    /// Unresolved waiters and their causes in selection order.
    pub blockages: Vec<(SymbolId, SignatureBlockage)>,
    /// Members still needing an initial attempt or a released retry in selection order.
    pub remaining: Vec<SymbolId>,
}

/// One caller-scheduled phase over members selected from ready classes.
/// Holds the original store throughout. Successful attempts commit canonical types;
/// blocked or failed attempts roll back. Drop the phase state after source edits.
#[derive(Debug)]
pub struct MemberSignaturePhase<'s, 'file> {
    selection: &'s MemberSignatureSelection<'s, 'file>,
    store: &'s mut SemanticStore,
    definitions: Definitions,
    packages: &'s Packages,
    observed: ObservedSignatures,
    failures: Vec<MemberSignatureError>,
    failure_indices: HashMap<SymbolId, usize>,
}

impl<'s, 'file> MemberSignaturePhase<'s, 'file> {
    /// Begins tracking a selection without typing any declarations. All inputs
    /// must belong to the session which produced the selected classes.
    pub fn new(
        selection: &'s MemberSignatureSelection<'s, 'file>,
        store: &'s mut SemanticStore,
        definitions: Definitions,
        packages: &'s Packages,
    ) -> Self {
        Self {
            selection,
            store,
            definitions,
            packages,
            observed: ObservedSignatures::new(SignaturePhase::Members),
            failures: Vec::new(),
            failure_indices: HashMap::new(),
        }
    }

    /// Whether the selected member has a committed signature and no direct rejection.
    pub fn is_ready(&self, member: SymbolId) -> bool {
        self.selection.contains(member)
            && !self.failure_indices.contains_key(&member)
            && matches!(self.store.symbols.info(member), SymbolInfo::Complete(_))
    }

    /// Observed dependencies for clients that follow a blocked provider immediately.
    pub fn dependencies(&self) -> &SignatureDependencies {
        &self.observed.graph
    }

    /// Attempts one selected member. On completion, the caller receives the
    /// unresolved waiters to retry. A repeated direct rejection returns the
    /// original error without rerunning the typer or duplicating its report.
    pub fn attempt(&mut self, member: SymbolId) -> Result<SignatureAttempt, &MemberSignatureError> {
        self.attempt_with_resolver(member, crate::core::NoResolver)
    }

    /// Attempts a selected signature using the caller's semantic resolver.
    /// Reuse the same resolver, store, package identities and classpath for
    /// retries and subsequent body checks in this revision. Failed attempts
    /// keep their typed errors and roll back newly loaded semantic state.
    pub fn attempt_with_resolver(
        &mut self,
        member: SymbolId,
        resolver: impl crate::core::SymbolResolver,
    ) -> Result<SignatureAttempt, &MemberSignatureError> {
        if let Some(&index) = self.failure_indices.get(&member) {
            return Err(&self.failures[index]);
        }
        match self.selection.try_complete_with_resolver(
            member,
            self.store,
            self.definitions,
            self.packages,
            resolver,
        ) {
            Ok(completion) => Ok(self.observed.record(member, completion, self.store)),
            Err(error) => {
                self.observed.graph.record_failure(member);
                let index = self.failures.len();
                self.failure_indices.insert(member, index);
                self.failures.push(error);
                Err(&self.failures[index])
            }
        }
    }

    /// Completes the supplied field and method roots in order. Follows an eligible
    /// missing provider immediately, then retries released waiters in registration
    /// order. Already-ready roots are skipped. The observer receives every
    /// completed or blocked attempt. Pass the same session resolver on retries
    /// and subsequent body checks, or [`crate::core::NoResolver`] for source-only work.
    /// On success, use the returned phase for more roots or for body selection.
    /// A direct rejection or dependency cycle ends the run before later roots.
    /// Earlier committed signatures remain in the store.
    pub fn try_complete_roots(
        self,
        roots: impl IntoIterator<Item = SymbolId>,
        mut resolver: impl crate::core::SymbolResolver,
        observe: impl FnMut(SymbolId, crate::typer::SignatureCompletion),
    ) -> Result<Self, crate::SignatureRunError<MemberSignatureError>> {
        crate::signature_schedule::try_complete_roots(
            self,
            roots,
            Self::is_ready,
            |phase, id| phase.attempt_with_resolver(id, &mut resolver).ok(),
            Self::dependencies,
            Self::into_failure,
            observe,
        )
    }

    /// Stops at a direct failure and moves its original phase error to the caller.
    /// Returns `None` if that symbol has no direct failure.
    pub fn into_failure(mut self, member: SymbolId) -> Option<MemberSignatureError> {
        let index = *self.failure_indices.get(&member)?;
        Some(self.failures.swap_remove(index))
    }

    /// Finishes as in [`Self::finish`] and indexes body eligibility for the same
    /// selection. The snapshot includes selected empty classes. Its decisions
    /// survive moving the signature diagnostics and remain tied to this session.
    pub fn finish_with_body_work(self) -> (MemberSignatureReport, crate::ClassBodyWork<'file>) {
        let selection = self.selection;
        let report = self.finish();
        let work = crate::ClassBodyWork::from_report(selection, &report);
        (report, work)
    }

    /// Releases the store and captures any errors and byte ranges using the
    /// original files. Classification performs no additional typing. Unattempted
    /// siblings remain distinguishable from members waiting on a rejected provider.
    pub fn finish(self) -> MemberSignatureReport {
        let mut ready = Vec::new();
        let mut remaining = Vec::new();
        let members = self.selection.members();
        for &member in members {
            if self.is_ready(member) {
                ready.push(member);
            } else if !self.failure_indices.contains_key(&member)
                && self.dependencies().waiting_on(member).is_none()
            {
                remaining.push(member);
            }
        }
        let blockages = self.dependencies().blockages(members.iter().copied());
        let failures = self
            .failures
            .into_iter()
            .map(|failure| MemberSignatureFailure::capture(failure, self.selection.sources))
            .collect();
        MemberSignatureReport {
            ready,
            remaining,
            failures,
            blockages,
        }
    }
}
