//! Ordered completion of source signatures using the active phase's dependency graph.
use std::{error::Error, fmt, sync::Arc};

use crate::core::SymbolId;
use crate::typer::SignatureCompletion;
use crate::{SignatureAttempt, SignatureBlockage, SignatureCycle, SignatureDependencies};

/// First failure reached while completing a caller-ordered set of source signatures.
#[derive(Debug)]
pub enum SignatureRunError<E> {
    /// Original header or member rejection from the attempted declaration.
    Rejected(E),
    /// Closed path of canonical symbols, excluding upstream dependents.
    Cycle(Arc<SignatureCycle>),
}

impl<E: fmt::Display> fmt::Display for SignatureRunError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => error.fmt(f),
            Self::Cycle(cycle) => write!(f, "{}: {:?}", cycle.code(), cycle.path()),
        }
    }
}

impl<E: Error + 'static> Error for SignatureRunError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rejected(error) => Some(error),
            Self::Cycle(_) => None,
        }
    }
}

pub(crate) fn try_complete_roots<P, E>(
    mut phase: P,
    roots: impl IntoIterator<Item = SymbolId>,
    is_ready: impl Fn(&P, SymbolId) -> bool,
    mut attempt: impl FnMut(&mut P, SymbolId) -> Option<SignatureAttempt>,
    dependencies: impl Fn(&P) -> &SignatureDependencies,
    into_failure: impl FnOnce(P, SymbolId) -> Option<E>,
    mut observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<P, SignatureRunError<E>> {
    let mut work = Vec::new();
    for root in roots {
        if is_ready(&phase, root) {
            continue;
        }
        work.push(root);
        while let Some(symbol) = work.pop() {
            let Some(attempt) = attempt(&mut phase, symbol) else {
                return Err(SignatureRunError::Rejected(
                    into_failure(phase, symbol)
                        .expect("a rejected attempt retains its phase error"),
                ));
            };
            observe(symbol, attempt.completion);
            work.extend(attempt.retry.into_iter().rev());
            if let SignatureCompletion::BlockedOn(provider) = attempt.completion {
                match dependencies(&phase).blockage(provider) {
                    Some(
                        SignatureBlockage::CycleMember(cycle)
                        | SignatureBlockage::DependsOnCycle(cycle),
                    ) => return Err(SignatureRunError::Cycle(cycle)),
                    _ => work.push(provider),
                }
            }
        }
    }
    Ok(phase)
}
