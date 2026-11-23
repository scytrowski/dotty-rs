//! Stateful attempts for the ordinary-class headers in a collection of named sources.
use std::collections::HashMap;

use crate::core::{AstArena, Definitions, Packages, SemanticStore, SymbolId, SymbolInfo, Typed};
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
    /// Previously blocked classes to retry, once each in registration order.
    pub retry: Vec<SymbolId>,
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
            Ok(completion) => Ok(ClassHeaderAttempt {
                completion,
                retry: self.dependencies.record(class, completion),
            }),
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
}
