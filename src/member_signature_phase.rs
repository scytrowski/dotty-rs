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
        if let Some(&index) = self.failure_indices.get(&member) {
            return Err(&self.failures[index]);
        }
        match self
            .selection
            .try_complete(member, self.store, self.definitions, self.packages)
        {
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

    /// Stops at a direct failure and moves its original phase error to the caller.
    /// Returns `None` if that symbol has no direct failure.
    pub fn into_failure(mut self, member: SymbolId) -> Option<MemberSignatureError> {
        let index = *self.failure_indices.get(&member)?;
        Some(self.failures.swap_remove(index))
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
            .map(|failure| match failure {
                MemberSignatureError::InvalidMember(id) => {
                    MemberSignatureFailure::InvalidMember(id)
                }
                MemberSignatureError::Typing {
                    member,
                    declaration,
                    error,
                } => MemberSignatureFailure::Typing {
                    declaration,
                    report: Box::new(ReportedTyperError::from_class(
                        error,
                        declaration.class,
                        Some(member),
                        |id| self.selection.sources.file(id).map(|f| f.ast()),
                    )),
                },
            })
            .collect();
        MemberSignatureReport {
            ready,
            remaining,
            failures,
            blockages,
        }
    }
}
