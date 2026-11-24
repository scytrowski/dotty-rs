//! Dependencies observed during a single source signature completion phase.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::core::{SemanticStore, SourceId, SourceSpan, SymbolId, SymbolInfo, SymbolOrigin};
use crate::typer::SignatureCompletion;

/// Outcome of one successful or blocked header or member signature attempt.
#[derive(Debug)]
#[must_use]
pub struct SignatureAttempt {
    /// Actual completion or the provider requested by the typer.
    pub completion: SignatureCompletion,
    /// Unresolved waiters to retry once. Direct waiters come first in registration
    /// order. When the typer completes other providers in the same file, their
    /// waiters follow in provider observation order and then registration order.
    pub retry: Vec<SymbolId>,
}

// The typer can complete additional declarations recursively within its file.
// Watch only providers on observed edges in that file.
#[derive(Debug)]
pub(crate) struct ObservedSignatures {
    pub(crate) graph: SignatureDependencies,
    watched: HashSet<SymbolId>,
    providers: HashMap<SourceId, Vec<SymbolId>>,
}

impl ObservedSignatures {
    pub(crate) fn new(phase: SignaturePhase) -> Self {
        Self {
            graph: SignatureDependencies::new(phase),
            watched: HashSet::new(),
            providers: HashMap::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        symbol: SymbolId,
        completion: SignatureCompletion,
        store: &SemanticStore,
    ) -> SignatureAttempt {
        let mut retry = self.graph.record(symbol, completion);
        match completion {
            SignatureCompletion::BlockedOn(provider) => {
                if let SymbolOrigin::Source(source) = store.symbols.get(provider).origin
                    && self.watched.insert(provider)
                {
                    self.providers.entry(source).or_default().push(provider);
                }
            }
            SignatureCompletion::Complete(_) => {
                if let SymbolOrigin::Source(source) = store.symbols.get(symbol).origin
                    && let Some(providers) = self.providers.get_mut(&source)
                {
                    providers.retain(|id| {
                        let SymbolInfo::Complete(ty) = *store.symbols.info(*id) else {
                            return true;
                        };
                        self.watched.remove(id);
                        if *id != symbol && !self.graph.failed.contains(id) {
                            retry.extend(self.graph.record(*id, SignatureCompletion::Complete(ty)));
                        }
                        false
                    });
                }
                retry.retain(|&waiter| {
                    !matches!(store.symbols.info(waiter), SymbolInfo::Complete(_))
                });
            }
        }
        SignatureAttempt { completion, retry }
    }
}

/// Which declarations are being completed in this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePhase {
    /// Class headers after constructor signature preparation.
    Headers,
    /// Field and method signatures after class headers.
    Members,
}

/// A directed cycle of canonical symbols observed during one completion phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureCycle {
    phase: SignaturePhase,
    path: Vec<SymbolId>,
}

impl SignatureCycle {
    /// Phase whose attempts supplied the edges.
    pub fn phase(&self) -> SignaturePhase {
        self.phase
    }

    /// Stable diagnostic code for this phase, independent of path or spelling.
    pub fn code(&self) -> &'static str {
        match self.phase {
            SignaturePhase::Headers => "signature.header_cycle",
            SignaturePhase::Members => "signature.member_cycle",
        }
    }

    /// Participants once each in path order, paired with their original declaration
    /// positions when available. Supply the same naming store. A missing position
    /// or a position from a different source leaves the symbol unlocated.
    pub fn declarations<'a>(
        &'a self,
        store: &'a SemanticStore,
    ) -> impl Iterator<Item = (SymbolId, Option<SourceSpan>)> + 'a {
        self.path[..self.path.len() - 1].iter().map(|&id| {
            let position = store
                .symbols
                .contains(id)
                .then(|| store.symbols.get(id))
                .and_then(|symbol| {
                    symbol
                        .position
                        .filter(|p| symbol.origin == SymbolOrigin::Source(p.source()))
                });
            (id, position)
        })
    }

    /// Closed path in dependency order. The first and last symbols are equal.
    /// A traversal beginning outside the cycle omits its leading path.
    pub fn path(&self) -> &[SymbolId] {
        &self.path
    }
}

/// Why a declaration still waits after the recorded attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureBlockage {
    /// The chain ends at a provider with no recorded outgoing edge or failure.
    Pending(SymbolId),
    /// The provider whose attempt failed. Look up its original phase error in
    /// the caller's reports. Every transitive waiter refers to that same symbol.
    Failed(SymbolId),
    /// This declaration participates in the closed path.
    CycleMember(Arc<SignatureCycle>),
    /// This declaration is outside the cycle but transitively needs a participant.
    DependsOnCycle(Arc<SignatureCycle>),
}

/// Latest dependency reported by each blocked declaration in one phase.
/// Use separate instances for headers and members. All symbols must come from
/// the same naming session. Discard the state when the inputs change.
#[derive(Debug)]
pub struct SignatureDependencies {
    phase: SignaturePhase,
    failed: HashSet<SymbolId>,
    waiting: HashMap<SymbolId, SymbolId>,
    waiters: HashMap<SymbolId, Vec<SymbolId>>,
}

impl SignatureDependencies {
    /// Starts tracking a caller-selected phase with no observed dependencies.
    pub fn new(phase: SignaturePhase) -> Self {
        Self {
            phase,
            failed: HashSet::new(),
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
        self.failed.remove(&symbol);
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

    /// Marks a directly rejected attempt. Keep its original phase error under
    /// this symbol. Dependents remain blocked and can identify the provider via
    /// [`Self::blockages`]. A subsequent attempt replaces this failure state.
    pub fn record_failure(&mut self, symbol: SymbolId) {
        self.detach(symbol);
        self.failed.insert(symbol);
    }

    /// Classifies the observed wait for one symbol without changing the graph
    /// or releasing retries. A symbol without an outgoing wait yields `None`.
    /// Use [`Self::blockages`] to share traversal when querying several roots.
    pub fn blockage(&self, symbol: SymbolId) -> Option<SignatureBlockage> {
        self.waiting_on(symbol)?;
        Some(self.trace(symbol, &mut HashMap::new()))
    }

    /// Classifies still-blocked roots in input order, omitting roots without a wait.
    /// Paths shared by several roots are visited once per query. All references
    /// to the same cycle share its path, starting at the first participant reached.
    pub fn blockages(
        &self,
        roots: impl IntoIterator<Item = SymbolId>,
    ) -> Vec<(SymbolId, SignatureBlockage)> {
        let mut known = HashMap::new();
        let mut result = Vec::new();
        for root in roots {
            if self.waiting.contains_key(&root) {
                let cause = self.trace(root, &mut known);
                result.push((root, cause));
            }
        }
        result
    }

    fn trace(
        &self,
        root: SymbolId,
        known: &mut HashMap<SymbolId, SignatureBlockage>,
    ) -> SignatureBlockage {
        let mut node = root;
        let mut path = Vec::new();
        let mut active = HashMap::new();
        let cause = loop {
            if let Some(cause) = known.get(&node) {
                break match cause {
                    SignatureBlockage::CycleMember(cycle) => {
                        SignatureBlockage::DependsOnCycle(cycle.clone())
                    }
                    other => other.clone(),
                };
            }
            let Some(next) = self.waiting_on(node) else {
                break if self.failed.contains(&node) {
                    SignatureBlockage::Failed(node)
                } else {
                    SignatureBlockage::Pending(node)
                };
            };
            if let Some(start) = active.insert(node, path.len()) {
                let mut closed = path.split_off(start);
                closed.push(node);
                let cycle = Arc::new(SignatureCycle {
                    phase: self.phase,
                    path: closed,
                });
                for &member in &cycle.path[..cycle.path.len() - 1] {
                    known.insert(member, SignatureBlockage::CycleMember(cycle.clone()));
                }
                break SignatureBlockage::DependsOnCycle(cycle);
            }
            path.push(node);
            node = next;
        };
        for symbol in path {
            known.insert(symbol, cause.clone());
        }
        known[&root].clone()
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
    fn failed_provider_is_distinct_from_pending_and_can_be_retried() {
        let d = Definitions::bootstrap(&mut SemanticStore::new());
        let (leaf, mid, bad) = (d.any_class, d.object_class, d.null_class);
        let mut graph = SignatureDependencies::new(SignaturePhase::Members);
        graph.record(leaf, BlockedOn(mid));
        graph.record(mid, BlockedOn(bad));
        assert_eq!(
            graph.blockages([leaf]),
            [(leaf, SignatureBlockage::Pending(bad))]
        );
        graph.record_failure(bad);
        assert_eq!(
            graph.blockages([leaf, mid]),
            [
                (leaf, SignatureBlockage::Failed(bad)),
                (mid, SignatureBlockage::Failed(bad))
            ]
        );
        assert_eq!(graph.record(bad, Complete(d.int)), [mid]);
        assert_eq!(
            graph.blockages([leaf]),
            [(leaf, SignatureBlockage::Pending(mid))]
        );
    }

    #[test]
    fn reports_only_cycle_participants_and_shares_the_closed_path() {
        let d = Definitions::bootstrap(&mut SemanticStore::new());
        let (leaf, a, b) = (d.any_class, d.object_class, d.null_class);
        for phase in [SignaturePhase::Headers, SignaturePhase::Members] {
            let mut graph = SignatureDependencies::new(phase);
            for (symbol, provider) in [(leaf, a), (a, b), (b, a)] {
                graph.record(symbol, BlockedOn(provider));
            }
            let blocked = graph.blockages([leaf, b, a]);
            let [
                (_, SignatureBlockage::DependsOnCycle(cycle)),
                (_, SignatureBlockage::CycleMember(second)),
                (_, SignatureBlockage::CycleMember(first)),
            ] = blocked.as_slice()
            else {
                panic!("{blocked:?}")
            };
            assert_eq!(cycle.phase(), phase);
            assert_eq!(cycle.path(), [a, b, a]);
            assert!(Arc::ptr_eq(cycle, first) && Arc::ptr_eq(first, second));
            graph.record(a, BlockedOn(a));
            let blocked = graph.blockages([a]);
            let SignatureBlockage::CycleMember(cycle) = &blocked[0].1 else {
                panic!("{blocked:?}")
            };
            assert_eq!(cycle.path(), [a, a]);
        }
    }
    #[test]
    fn point_queries_distinguish_the_cycle_and_preserve_retry_order() {
        let d = Definitions::bootstrap(&mut SemanticStore::new());
        let (leaf, a, b, other) = (d.any_class, d.object_class, d.null_class, d.nothing_class);
        let mut graph = SignatureDependencies::new(SignaturePhase::Headers);
        for (id, provider) in [(leaf, a), (a, b), (b, a)] {
            graph.record(id, BlockedOn(provider));
        }
        assert!(
            matches!(graph.blockage(leaf),Some(SignatureBlockage::DependsOnCycle(c)) if c.path()==[a,b,a])
        );
        for (id, path) in [(a, [a, b, a]), (b, [b, a, b])] {
            assert!(
                matches!(graph.blockage(id),Some(SignatureBlockage::CycleMember(c)) if c.path()==path)
            );
        }
        assert_eq!(graph.blockage(other), None);
        assert_eq!(graph.record(a, Complete(d.int)), [leaf, b]);
    }

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
