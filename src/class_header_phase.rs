//! Stateful attempts for the ordinary-class headers in a collection of named sources.
use std::collections::HashMap;

use crate::core::{AstArena, Definitions, Packages, SemanticStore, SymbolId, SymbolInfo, Typed};
use crate::{
    ClassHeaderBindings, ClassHeaderFailure, NamedClassSources, SignatureDependencies,
    SignaturePhase,
};

/// Result of a class header attempt and the declarations released for retry.
pub type ClassHeaderAttempt = crate::SignatureAttempt;

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
    observed: crate::signature_dependencies::ObservedSignatures,
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

    /// Prepares constructors in the same order as [`Self::prepare`] and stops
    /// at the first rejection. Earlier successful constructors remain complete,
    /// and later constructors and header bindings are untouched.
    pub fn try_prepare(
        sources: &'s NamedClassSources<'file>,
        store: &'s mut SemanticStore,
        definitions: Definitions,
        packages: &'s Packages,
        arena: &'s mut AstArena<Typed>,
        bindings: &'s mut ClassHeaderBindings,
    ) -> Result<Self, ClassHeaderFailure> {
        sources
            .prepare_class_constructors(
                sources.classes().map(|(_, class)| class.symbol),
                store,
                definitions,
                packages,
            )
            .try_for_each(|result| result.map(|_| ()))?;
        Ok(Self::new(
            sources,
            store,
            definitions,
            packages,
            arena,
            bindings,
        ))
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
            observed: crate::signature_dependencies::ObservedSignatures::new(
                SignaturePhase::Headers,
            ),
            failures: Vec::new(),
            failure_indices: HashMap::new(),
        }
    }

    fn record_failure(&mut self, failure: ClassHeaderFailure) -> usize {
        let index = self.failures.len();
        self.observed.graph.record_failure(failure.class);
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
        &self.observed.graph
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
            Ok(completion) => Ok(self.observed.record(class, completion, self.store)),
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

    /// Stops the phase and moves out the direct error for one class, if present.
    /// Use after a rejected attempt when stopping at the first error. This skips
    /// classification of the remaining graph. Use [`Self::finish`] to keep all errors.
    pub fn into_failure(mut self, class: SymbolId) -> Option<ClassHeaderFailure> {
        let index = *self.failure_indices.get(&class)?;
        Some(self.failures.swap_remove(index))
    }

    /// Ends this phase and releases its borrows of the session. Classification
    /// reads the canonical store and the recorded graph and runs no further
    /// typing. Direct errors move into the result with their original payloads.
    pub fn finish(self) -> ClassHeaderReport {
        let classes = || self.sources.classes().map(|(_, c)| c.symbol);
        let blockages = self.observed.graph.blockages(classes());
        let mut ready = Vec::new();
        let mut remaining = Vec::new();
        for class in classes() {
            if self.is_ready(class) {
                ready.push(class);
            } else if !self.failure_indices.contains_key(&class)
                && self.observed.graph.waiting_on(class).is_none()
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
