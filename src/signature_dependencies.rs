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
    waiters: HashMap<SymbolId, Vec<SymbolId>>,
}

impl SignatureDependencies {
    /// Starts tracking a caller-selected phase with no observed dependencies.
    pub fn new(phase: SignaturePhase) -> Self {
        Self {
            phase,
            waiting: HashMap::new(),
            waiters: HashMap::new(),
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
    /// When a provider completes, returns its direct waiters once in registration
    /// order and clears their edges. The caller decides when to retry them.
    pub fn record(&mut self, symbol: SymbolId, outcome: SignatureCompletion) -> Vec<SymbolId> {
        if let SignatureCompletion::BlockedOn(provider) = outcome
            && self.waiting_on(symbol) == Some(provider)
        {
            return Vec::new();
        }
        self.detach(symbol);
        match outcome {
            SignatureCompletion::BlockedOn(provider) => {
                self.waiting.insert(symbol, provider);
                self.waiters.entry(provider).or_default().push(symbol);
                Vec::new()
            }
            SignatureCompletion::Complete(_) => {
                let ready = self.waiters.remove(&symbol).unwrap_or_default();
                for waiter in &ready {
                    self.waiting.remove(waiter);
                }
                ready
            }
        }
    }

    fn detach(&mut self, symbol: SymbolId) {
        if let Some(previous) = self.waiting.remove(&symbol)
            && let Some(waiters) = self.waiters.get_mut(&previous)
        {
            waiters.retain(|&waiter| waiter != symbol);
            if waiters.is_empty() {
                self.waiters.remove(&previous);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Definitions, SemanticStore};
    use SignatureCompletion::{BlockedOn, Complete};

    #[test]
    fn wakes_only_current_waiters_once_in_registration_order() {
        let d = Definitions::bootstrap(&mut SemanticStore::new());
        let (first, second, provider, old) =
            (d.any_class, d.object_class, d.null_class, d.nothing_class);
        let mut graph = SignatureDependencies::new(SignaturePhase::Members);
        graph.record(first, BlockedOn(old));
        graph.record(first, BlockedOn(provider));
        graph.record(second, BlockedOn(provider));
        graph.record(first, BlockedOn(provider));
        assert!(graph.record(old, Complete(d.int)).is_empty());
        assert_eq!(graph.record(provider, Complete(d.int)), [first, second]);
        assert_eq!(graph.waiting_on(first), None);
        assert_eq!(graph.waiting_on(second), None);
        assert!(graph.record(provider, Complete(d.int)).is_empty());
    }

    #[test]
    fn completing_headers_wakes_only_the_next_link() {
        let d = Definitions::bootstrap(&mut SemanticStore::new());
        let (leaf, mid, base) = (d.any_class, d.object_class, d.null_class);
        for reverse in [true, false] {
            let mut graph = SignatureDependencies::new(SignaturePhase::Headers);
            let mut attempts = Vec::new();
            if reverse {
                for (symbol, parent) in [(leaf, mid), (mid, base)] {
                    attempts.push(symbol);
                    assert!(graph.record(symbol, BlockedOn(parent)).is_empty());
                }
            }
            for (symbol, next) in [(base, Some(mid)), (mid, Some(leaf)), (leaf, None)] {
                attempts.push(symbol);
                let expected: Vec<_> = next.filter(|_| reverse).into_iter().collect();
                assert_eq!(graph.record(symbol, Complete(d.int)), expected);
            }
            assert_eq!(attempts.len(), if reverse { 5 } else { 3 });
        }
    }
}
