//! Reporting data captured from the classified waits of one source signature phase.
use crate::{
    ClassHeaderReport, MemberSignatureReport, SignatureBlockage, SignatureCycle,
    SignatureDeclaration, SignaturePhase, SourceDeclarationIndex,
    core::{SemanticStore, SymbolId},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Owned snapshot of dependency causes from one finished phase. The snapshot
/// retains declaration metadata for the original source versions. Symbols
/// remain local to the originating session.
#[derive(Debug)]
pub struct SignatureBlockageReport {
    /// Phase whose attempts produced the classified waits.
    pub phase: SignaturePhase,
    waits: Vec<(SymbolId, SignatureBlockage)>,
    cycles: Vec<Arc<SignatureCycle>>,
    declarations: HashMap<SymbolId, SignatureDeclaration>,
}

impl SignatureBlockageReport {
    fn capture(
        phase: SignaturePhase,
        waits: &[(SymbolId, SignatureBlockage)],
        index: &SourceDeclarationIndex,
        store: &SemanticStore,
    ) -> Self {
        let mut cycles = Vec::new();
        let mut seen = HashSet::new();
        let mut declarations = HashMap::new();
        let mut capture_declaration = |symbol| {
            declarations
                .entry(symbol)
                .or_insert_with(|| index.signature_declaration(symbol, store));
        };
        for (waiter, cause) in waits {
            capture_declaration(*waiter);
            match cause {
                SignatureBlockage::Pending(id) | SignatureBlockage::Failed(id) => {
                    capture_declaration(*id)
                }
                SignatureBlockage::CycleMember(cycle)
                | SignatureBlockage::DependsOnCycle(cycle) => {
                    if seen.insert(cycle.path()[0]) {
                        for &id in cycle.participants() {
                            capture_declaration(id);
                        }
                        cycles.push(Arc::clone(cycle));
                    }
                }
            }
        }
        Self {
            phase,
            waits: waits.to_vec(),
            cycles,
            declarations,
        }
    }

    /// Each blocked root and its original terminal cause, in the phase report's
    /// order. A failed provider refers to the independently retained direct
    /// failure. Pending providers can still be scheduled by the caller.
    pub fn waiters(
        &self,
    ) -> impl ExactSizeIterator<Item = (&SignatureDeclaration, &SignatureBlockage)> {
        self.waits
            .iter()
            .map(|(id, cause)| (&self.declarations[id], cause))
    }

    /// Details captured for a waiter, terminal provider or cycle participant.
    /// Returns `None` for symbols not mentioned by this snapshot.
    pub fn declaration(&self, id: SymbolId) -> Option<&SignatureDeclaration> {
        self.declarations.get(&id)
    }

    /// Distinct cycles in first-observed order in the classified waits,
    /// paired with their participant declarations in directed path order.
    /// Each participant occurs once, including for a self-cycle.
    pub fn cycles(
        &self,
    ) -> impl ExactSizeIterator<
        Item = (
            &Arc<SignatureCycle>,
            impl ExactSizeIterator<Item = &SignatureDeclaration>,
        ),
    > {
        self.cycles.iter().map(|cycle| {
            (
                cycle,
                cycle.participants().iter().map(|id| &self.declarations[id]),
            )
        })
    }
}

impl ClassHeaderReport {
    /// Captures source details for this phase's already classified header
    /// dependencies. Supply the original declaration index and semantic store.
    /// This borrows the phase report and performs no further typing.
    pub fn blockage_report(
        &self,
        index: &SourceDeclarationIndex,
        store: &SemanticStore,
    ) -> SignatureBlockageReport {
        SignatureBlockageReport::capture(SignaturePhase::Headers, &self.blockages, index, store)
    }
}

impl MemberSignatureReport {
    /// Captures source details for this phase's already classified member
    /// dependencies, independently of its movable direct diagnostics. Supply
    /// the original declaration index and semantic store.
    pub fn blockage_report(
        &self,
        index: &SourceDeclarationIndex,
        store: &SemanticStore,
    ) -> SignatureBlockageReport {
        SignatureBlockageReport::capture(SignaturePhase::Members, &self.blockages, index, store)
    }
}
