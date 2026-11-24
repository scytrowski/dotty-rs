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
        for (_, cause) in waits {
            if let SignatureBlockage::CycleMember(cycle) | SignatureBlockage::DependsOnCycle(cycle) =
                cause
                && seen.insert(cycle.path()[0])
            {
                for &symbol in cycle.participants() {
                    declarations.insert(symbol, index.signature_declaration(symbol, store));
                }
                cycles.push(Arc::clone(cycle));
            }
        }
        Self {
            phase,
            cycles,
            declarations,
        }
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
