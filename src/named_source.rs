//! One session-bound compilation unit after successful source naming.
use crate::core::{SourceId, SourceSemanticIndex};
use crate::namer::NamerDiagnostic;
use crate::parser::ParseResult;

/// The original parse and the semantic index built from its exact tree and source.
/// Symbol and name IDs belong to the store used to bind and name this file.
#[derive(Debug)]
pub struct NamedSource {
    syntax: ParseResult,
    index: Box<SourceSemanticIndex>,
    diagnostics: Vec<NamerDiagnostic>,
}

impl NamedSource {
    /// Source ID captured by the parser, including for a file without calls.
    pub fn source(&self) -> SourceId {
        self.syntax.call_names.source()
    }

    /// Original arena, root, call-name tokens and effective parser features.
    pub fn syntax(&self) -> &ParseResult {
        &self.syntax
    }

    /// Naming bindings for this file's original tree IDs.
    pub fn index(&self) -> &SourceSemanticIndex {
        &self.index
    }

    /// Diagnostics committed by the namer on success, in emission order.
    pub fn diagnostics(&self) -> &[NamerDiagnostic] {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Packages, SemanticStore, TreeKind};
    use crate::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};

    #[test]
    fn same_slot_fields_retain_their_source_class_and_symbol() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let mut fields = Vec::new();
        for (i, name) in [(3, "A"), (7, "B")] {
            let source = SourceId::from_index(i);
            let syntax = crate::parse_source(
                source,
                &format!("class {name} {{ val seed = 1 }}"),
                Default::default(),
            )
            .unwrap()
            .bind(&mut store.names)
            .syntax;
            let NamingOutcome::Named { index, diagnostics } =
                name_compilation_unit_with_diagnostics(
                    &syntax.ast,
                    syntax.root,
                    source,
                    &format!("{name}.scala"),
                    SourceParseStatus::Clean,
                    &mut store,
                    &mut packages,
                )
                .unwrap()
            else {
                panic!("expected a named class")
            };
            let unit = NamedSource {
                syntax,
                index,
                diagnostics,
            };
            assert_eq!(unit.source(), source);
            let (tree, _) = unit
                .syntax()
                .ast
                .iter()
                .find(|(_, n)| matches!(n.kind, TreeKind::ValDef(_)))
                .unwrap();
            let symbol = unit.index().symbol_at(unit.source(), tree).unwrap();
            let class = store.symbols.owner(symbol).unwrap();
            assert_eq!(
                store.names.resolve(store.symbols.get(class).name.text()),
                name
            );
            assert_eq!(store.symbols.get(symbol).position.unwrap().source(), source);
            fields.push((tree, symbol));
        }
        assert_eq!(fields[0].0, fields[1].0);
        assert_ne!(fields[0].1, fields[1].1);
    }
}
