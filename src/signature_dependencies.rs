//! Dependencies observed during a single source signature completion phase.
use std::collections::HashMap;

use crate::core::SymbolId;
use crate::typer::SignatureCompletion;

/// Which declarations are being completed in this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePhase {
    /// Class headers after constructor signature preparation.
    Headers,
    /// Field and method signatures after class headers.
    Members,
}

/// Latest dependency reported by each blocked declaration in one phase.
/// Use separate instances for headers and members. All symbols must come from
/// the same naming session. Discard the state when the inputs change.
#[derive(Debug)]
pub struct SignatureDependencies {
    phase: SignaturePhase,
    waiting: HashMap<SymbolId, SymbolId>,
}

impl SignatureDependencies {
    /// Starts tracking a caller-selected phase with no observed dependencies.
    pub fn new(phase: SignaturePhase) -> Self {
        Self {
            phase,
            waiting: HashMap::new(),
        }
    }

    /// Phase in which all recorded attempts were made.
    pub fn phase(&self) -> SignaturePhase {
        self.phase
    }

    /// Current direct dependency, if the most recent attempt is still blocked.
    pub fn waiting_on(&self, symbol: SymbolId) -> Option<SymbolId> {
        self.waiting.get(&symbol).copied()
    }

    /// Records the typer's actual outcome. A new attempt replaces the previous
    /// outgoing edge for this symbol; a completed signature has none.
    pub fn record(&mut self, symbol: SymbolId, outcome: SignatureCompletion) {
        match outcome {
            SignatureCompletion::BlockedOn(provider) => {
                self.waiting.insert(symbol, provider);
            }
            SignatureCompletion::Complete(_) => {
                self.waiting.remove(&symbol);
            }
        }
    }
}
