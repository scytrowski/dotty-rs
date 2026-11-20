use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceSemanticIndex, SymbolId, TextRange, TreeKind,
    Typed, Untyped,
};
use crate::jvm_class_plan::TypedClassBodies;
use crate::parser::SourceCallNames;
use crate::source_declarations::{DeclarationError, SourceClass};

/// Direct call names and the ranges of their top-level classes in one named source file.
#[derive(Debug)]
pub struct SourceFileCalls {
    names: SourceCallNames,
    classes: Vec<(TextRange, SymbolId)>,
}

impl SourceFileCalls {
    /// Prepare once per parse using its call names, arena, naming index and the classes
    /// returned in source order by [`source_classes`](crate::source_declarations::source_classes).
    /// A class with an inconsistent binding or a missing, empty or overlapping range
    /// yields [`DeclarationError::InvalidBinding`].
    pub fn new(
        names: SourceCallNames,
        ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
        classes: &[SourceClass],
    ) -> Result<Self, DeclarationError> {
        let source = names.source();
        let mut ranges = Vec::with_capacity(classes.len());
        let mut previous_end = 0;
        for class in classes {
            let tree = class.tree;
            let invalid = DeclarationError::InvalidBinding { source, tree };
            if index.definition_of(class.symbol)
                != Some(SourceDefinition::Canonical { source, tree })
            {
                return Err(invalid);
            }
            let node = ast.try_get(tree).ok_or(invalid)?;
            let position = node
                .position
                .filter(|p| p.source() == source)
                .ok_or(invalid)?;
            let range = position.span().range();
            if !matches!(node.kind, TreeKind::TypeDef(_))
                || range.is_empty()
                || range.start() < previous_end
            {
                return Err(invalid);
            }
            ranges.push((range, class.symbol));
            previous_end = range.end();
        }
        Ok(Self {
            names,
            classes: ranges,
        })
    }

    /// The original call names, usable for syntax-only lookup.
    pub fn call_names(&self) -> &SourceCallNames {
        &self.names
    }

    fn class_at(&self, offset: u32) -> Option<SymbolId> {
        let i = self
            .classes
            .partition_point(|(range, _)| range.end() <= offset);
        let &(range, symbol) = self.classes.get(i)?;
        range.contains(offset).then_some(symbol)
    }

    /// Resolve a UTF-8 byte offset on a method name using the current bodies from
    /// the same session. Pass the original parse arena and the bodies' typed arena.
    /// Returns `None` if the enclosing class has no recorded body or the position
    /// does not resolve to a direct method call.
    pub fn selected_method_at(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        bodies: &TypedClassBodies,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Option<SymbolId> {
        let body = bodies.get(self.class_at(offset)?).ok()?;
        body.selected_method_at(offset, &self.names, ast, typed_ast, store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Packages, SourceId, SourceText};
    use crate::source_declarations::source_classes;

    fn named() -> (
        crate::parser::ParseResult,
        SourceSemanticIndex,
        Vec<SourceClass>,
    ) {
        let text = " class A {}  \nclass B {} ";
        let source = SourceId::from_index(3);
        let mut store = SemanticStore::new();
        let parsed = crate::parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            crate::lexer::ContextualScanner::new(text).unwrap(),
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty());
        let index = crate::namer::name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Classes.scala",
            &mut store,
            &mut Packages::new(),
        )
        .unwrap();
        let classes = source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap();
        (parsed, index, classes)
    }

    #[test]
    fn class_ranges_exclude_the_gap_and_their_end_offsets() {
        let (parsed, index, classes) = named();
        let calls = SourceFileCalls::new(parsed.call_names, &parsed.ast, &index, &classes).unwrap();
        for (offset, expected) in [
            (0, None),
            (1, Some(classes[0].symbol)),
            (10, Some(classes[0].symbol)),
            (11, None),
            (13, None),
            (14, Some(classes[1].symbol)),
            (23, Some(classes[1].symbol)),
            (24, None),
            (u32::MAX, None),
        ] {
            assert_eq!(calls.class_at(offset), expected, "offset {offset}");
        }
    }

    #[test]
    fn swapped_class_symbols_are_rejected_during_preparation() {
        let (parsed, index, mut classes) = named();
        classes[0].symbol = classes[1].symbol;
        let expected = DeclarationError::InvalidBinding {
            source: parsed.call_names.source(),
            tree: classes[0].tree,
        };
        assert_eq!(
            SourceFileCalls::new(parsed.call_names, &parsed.ast, &index, &classes).unwrap_err(),
            expected
        );
    }
}
