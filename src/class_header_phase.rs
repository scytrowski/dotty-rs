//! Stateful attempts for the ordinary-class headers in a collection of named sources.
use std::collections::{HashMap, HashSet};

use crate::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SymbolId, SymbolInfo, Typed,
};
use crate::typer::SignatureCompletion;
use crate::{
    ClassHeaderBindings, ClassHeaderFailure, NamedClassSources, SignatureDependencies,
    SignaturePhase,
};

/// Outcome of one successful or blocked header attempt.
#[derive(Debug)]
#[must_use]
pub struct ClassHeaderAttempt {
    /// Actual completion or the provider requested by the typer.
    pub completion: SignatureCompletion,
    /// Unresolved waiters to retry once. Direct waiters come first in registration
    /// order. When the typer completes other providers in the same file, their
    /// waiters follow in provider observation order and then registration order.
    pub retry: Vec<SymbolId>,
}

/// Owned outcome when the caller stops attempting headers.
/// IDs, types and byte ranges refer to the same session and source versions.
#[derive(Debug)]
pub struct ClassHeaderReport {
    /// Usable classes in input file and declaration order, including recursive completions.
    pub ready: Vec<SymbolId>,
    /// Direct rejections in observation order, beginning with constructor failures.
    pub failures: Vec<ClassHeaderFailure>,
    /// Unresolved dependencies in input file and declaration order.
    pub blockages: Vec<(SymbolId, crate::SignatureBlockage)>,
    /// Classes still requiring an initial attempt or an already released retry,
    /// in input file and declaration order.
    pub remaining: Vec<SymbolId>,
}

/// Header completion for one immutable collection of named files in one session.
/// Holds the same store, arena and source-to-typed bindings across all attempts.
/// The caller chooses which class to attempt and when to retry a waiter.
#[derive(Debug)]
pub struct ClassHeaderPhase<'s, 'file> {
    sources: &'s NamedClassSources<'file>,
    store: &'s mut SemanticStore,
    definitions: Definitions,
    packages: &'s Packages,
    arena: &'s mut AstArena<Typed>,
    bindings: &'s mut ClassHeaderBindings,
    dependencies: SignatureDependencies,
    // Only providers actually requested across files can have waiters to wake.
    watched: HashSet<SymbolId>,
    providers: HashMap<SourceId, Vec<SymbolId>>,
    failures: Vec<ClassHeaderFailure>,
    failure_indices: HashMap<SymbolId, usize>,
}

impl<'s, 'file> ClassHeaderPhase<'s, 'file> {
    /// Prepares all indexed constructors in declaration order and retains every
    /// direct failure. Independent constructors continue after a rejection.
    /// Supply the naming store, its definitions and packages, and the arena and
    /// bindings used for this analysis. These remain available after the phase ends.
    pub fn prepare(
        sources: &'s NamedClassSources<'file>,
        store: &'s mut SemanticStore,
        definitions: Definitions,
        packages: &'s Packages,
        arena: &'s mut AstArena<Typed>,
        bindings: &'s mut ClassHeaderBindings,
    ) -> Self {
        let failures = sources
            .prepare_class_constructors(
                sources.classes().map(|(_, c)| c.symbol),
                store,
                definitions,
                packages,
            )
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        let mut phase = Self::new(sources, store, definitions, packages, arena, bindings);
        for failure in failures {
            phase.record_failure(failure);
        }
        phase
    }

    fn new(
        sources: &'s NamedClassSources<'file>,
        store: &'s mut SemanticStore,
        definitions: Definitions,
        packages: &'s Packages,
        arena: &'s mut AstArena<Typed>,
        bindings: &'s mut ClassHeaderBindings,
    ) -> Self {
        Self {
            sources,
            store,
            definitions,
            packages,
            arena,
            bindings,
            dependencies: SignatureDependencies::new(SignaturePhase::Headers),
            providers: HashMap::new(),
            watched: HashSet::new(),
            failures: Vec::new(),
            failure_indices: HashMap::new(),
        }
    }

    fn record_failure(&mut self, failure: ClassHeaderFailure) -> usize {
        let index = self.failures.len();
        self.dependencies.record_failure(failure.class);
        self.failure_indices.insert(failure.class, index);
        self.failures.push(failure);
        index
    }

    /// Whether this indexed class has a usable header in the session store.
    /// Includes headers completed recursively in the same file. A class with a
    /// rejected constructor or header is ineligible even if it has stored info.
    pub fn is_ready(&self, class: SymbolId) -> bool {
        self.sources.class(class).is_some()
            && !self.failure_indices.contains_key(&class)
            && matches!(self.store.symbols.info(class), SymbolInfo::Complete(_))
    }

    /// Recorded dependencies for queries about the current attempt sequence.
    /// Inspect a blocked provider here before following it recursively.
    pub fn dependencies(&self) -> &SignatureDependencies {
        &self.dependencies
    }

    /// Attempts a class and records its outcome. Rejections are retained once
    /// and returned by reference on repeated requests. Blocked and failed
    /// attempts preserve earlier typed trees and bindings. An unindexed class
    /// yields a structural failure without changing the semantic store.
    pub fn attempt(&mut self, class: SymbolId) -> Result<ClassHeaderAttempt, &ClassHeaderFailure> {
        if let Some(&index) = self.failure_indices.get(&class) {
            return Err(&self.failures[index]);
        }
        match self.sources.try_complete_class_header_reported(
            class,
            self.store,
            self.definitions,
            self.packages,
            self.arena,
            self.bindings,
        ) {
            Ok(completion) => {
                let mut retry = self.dependencies.record(class, completion);
                match completion {
                    SignatureCompletion::BlockedOn(provider) => {
                        if self.watched.insert(provider) {
                            // The typer only identifies indexed foreign classes as providers.
                            let (file, _) = self.sources.class(provider).unwrap();
                            self.providers
                                .entry(file.source())
                                .or_default()
                                .push(provider);
                        }
                    }
                    SignatureCompletion::Complete(_) => {
                        let (file, _) = self.sources.class(class).unwrap();
                        if let Some(providers) = self.providers.get_mut(&file.source()) {
                            providers.retain(|provider| {
                                let SymbolInfo::Complete(ty) = *self.store.symbols.info(*provider)
                                else {
                                    return true;
                                };
                                self.watched.remove(provider);
                                if class != *provider
                                    && !self.failure_indices.contains_key(provider)
                                {
                                    retry.extend(
                                        self.dependencies
                                            .record(*provider, SignatureCompletion::Complete(ty)),
                                    );
                                }
                                false
                            });
                        }
                        retry.retain(|&waiter| !self.is_ready(waiter));
                    }
                }
                Ok(ClassHeaderAttempt { completion, retry })
            }
            Err(error) => {
                let source = self.sources.class(class).map(|(file, _)| file.source());
                let index = self.record_failure(ClassHeaderFailure {
                    class,
                    source,
                    error,
                });
                Err(&self.failures[index])
            }
        }
    }

    /// Ends this phase and releases its borrows of the session. Classification
    /// reads the canonical store and the recorded graph and runs no further
    /// typing. Direct errors move into the result with their original payloads.
    pub fn finish(self) -> ClassHeaderReport {
        let classes = || self.sources.classes().map(|(_, c)| c.symbol);
        let blockages = self.dependencies.blockages(classes());
        let mut ready = Vec::new();
        let mut remaining = Vec::new();
        for class in classes() {
            if self.is_ready(class) {
                ready.push(class);
            } else if !self.failure_indices.contains_key(&class)
                && self.dependencies.waiting_on(class).is_none()
            {
                remaining.push(class);
            }
        }
        ClassHeaderReport {
            ready,
            remaining,
            blockages,
            failures: self.failures,
        }
    }
}
