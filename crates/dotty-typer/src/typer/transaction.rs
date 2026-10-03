//! Rollback boundaries for typer-owned mutable semantic state.

use super::*;

impl SourceTyper<'_> {
    pub(super) fn run_atomic<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, &mut Vec<(SymbolId, SymbolInfo)>) -> Result<T, TyperError>,
    ) -> Result<T, TyperError> {
        let checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let cache_checkpoint = self.type_index.checkpoint();
        let typed_ast_checkpoint = self.typed_arena.checkpoint();
        let typed_index_checkpoint = self.typed_index.clone();
        let local_symbols_checkpoint = self.local_symbols.clone();
        let pattern_bindings_checkpoint = self.pattern_bindings.clone();
        let local_methods_checkpoint = self.local_methods.clone();
        let initializing_local_symbols_checkpoint = self.initializing_local_symbols.clone();
        let inferred_method_results_checkpoint = self.inferred_method_results_in_progress.clone();
        let expression_scope_checkpoint = self.expression_scopes.clone();
        let mut info_journal = Vec::new();
        let result = operation(self, &mut info_journal);
        if result.is_err() {
            for (changed, previous) in info_journal.into_iter().rev() {
                if self.store.symbols.contains(changed) {
                    self.store.symbols.set_info(changed, previous);
                }
            }
            self.resolver.rollback_to(self.store, resolver_checkpoint);
            self.store.rollback_to(checkpoint);
            self.type_index.restore(cache_checkpoint);
            self.typed_arena.rollback_to(typed_ast_checkpoint);
            self.typed_index = typed_index_checkpoint;
            self.local_symbols = local_symbols_checkpoint;
            self.pattern_bindings = pattern_bindings_checkpoint;
            self.local_methods = local_methods_checkpoint;
            self.initializing_local_symbols = initializing_local_symbols_checkpoint;
            self.inferred_method_results_in_progress = inferred_method_results_checkpoint;
            self.expression_scopes = expression_scope_checkpoint;
        }
        result
    }

    pub(super) fn run_expression_transaction<T>(
        &mut self,
        operation: impl FnOnce(
            &mut Self,
            &mut Vec<(SymbolId, SymbolInfo)>,
            &mut Vec<(SourceId, TreeId<Untyped>)>,
        ) -> Result<T, TyperError>,
    ) -> Result<T, TyperError> {
        let ast_checkpoint = self.typed_arena.checkpoint();
        let typed_index_checkpoint = self.typed_index.clone();
        let store_checkpoint = self.store.checkpoint();
        let resolver_checkpoint = self.resolver.checkpoint();
        let type_index_checkpoint = self.type_index.checkpoint();
        let local_symbols_checkpoint = self.local_symbols.clone();
        let pattern_bindings_checkpoint = self.pattern_bindings.clone();
        let local_methods_checkpoint = self.local_methods.clone();
        let initializing_local_symbols_checkpoint = self.initializing_local_symbols.clone();
        let expression_scope_checkpoint = self.expression_scopes.clone();
        let mut info_journal = Vec::new();
        let mut new_mappings = Vec::new();
        let result = operation(self, &mut info_journal, &mut new_mappings);
        match result {
            Ok(typed) => Ok(typed),
            Err(error) => {
                for (symbol, previous) in info_journal.into_iter().rev() {
                    if self.store.symbols.contains(symbol) {
                        self.store.symbols.set_info(symbol, previous);
                    }
                }
                self.typed_arena.rollback_to(ast_checkpoint);
                self.resolver.rollback_to(self.store, resolver_checkpoint);
                self.store.rollback_to(store_checkpoint);
                self.type_index.restore(type_index_checkpoint);
                self.local_symbols = local_symbols_checkpoint;
                self.pattern_bindings = pattern_bindings_checkpoint;
                self.local_methods = local_methods_checkpoint;
                self.initializing_local_symbols = initializing_local_symbols_checkpoint;
                self.expression_scopes = expression_scope_checkpoint;
                // Nested completion can type an inferred method body with its
                // own mapping journal. Restore the full index so those
                // successful nested mappings cannot outlive rolled-back AST
                // nodes when this outer expression fails.
                self.typed_index = typed_index_checkpoint;
                Err(error)
            }
        }
    }
}
