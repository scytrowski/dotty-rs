use std::{collections::HashMap, sync::Arc};

use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SymbolId,
    SymbolOrigin, TextRange, TreeKind, Typed, Untyped,
};
use crate::jvm_class_plan::TypedClassBodies;
use crate::parser::{ParseResult, SourceCallNames};
use crate::source_declarations::{DeclarationError, SourceClass};

/// A source definition and the written token to select inside it, in UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMethodTarget {
    /// Identity of the original source file in this session.
    pub source: SourceId,
    /// UTF-8 byte range of the complete source method definition.
    pub declaration: TextRange,
    /// UTF-8 byte range of the written method name token.
    pub name: TextRange,
}

#[derive(Debug)]
enum CallNames {
    Owned(SourceCallNames),
    Parsed(Arc<ParseResult>),
}

impl CallNames {
    fn tokens(&self) -> &SourceCallNames {
        match self {
            Self::Owned(names) => names,
            Self::Parsed(parse) => &parse.call_names,
        }
    }
}

/// Direct call names and the ranges of their top-level classes in one named source file.
#[derive(Debug)]
pub struct SourceFileCalls {
    names: CallNames,
    classes: Vec<(TextRange, SymbolId)>,
    targets: HashMap<SymbolId, SourceMethodTarget>,
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
        Self::build(CallNames::Owned(names), ast, index, classes)
    }

    pub(crate) fn from_parse(
        parse: &Arc<ParseResult>,
        index: &SourceSemanticIndex,
        classes: &[SourceClass],
    ) -> Result<Self, DeclarationError> {
        Self::build(
            CallNames::Parsed(Arc::clone(parse)),
            &parse.ast,
            index,
            classes,
        )
    }

    fn build(
        names: CallNames,
        ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
        classes: &[SourceClass],
    ) -> Result<Self, DeclarationError> {
        let tokens = names.tokens();
        let source = tokens.source();
        let mut ranges = Vec::with_capacity(classes.len());
        let mut targets = HashMap::new();
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
            for method in class.methods() {
                if let Some(name) = tokens.method_name(method.tree) {
                    let tree = method.tree;
                    let invalid = DeclarationError::InvalidBinding { source, tree };
                    if index.definition_of(method.symbol)
                        != Some(SourceDefinition::Canonical { source, tree })
                    {
                        return Err(invalid);
                    }
                    let declaration = ast
                        .try_get(tree)
                        .and_then(|node| node.position)
                        .filter(|p| p.source() == source)
                        .ok_or(invalid)?
                        .span()
                        .range();
                    if name.start() < declaration.start()
                        || name.end() > declaration.end()
                        || declaration.start() < range.start()
                        || declaration.end() > range.end()
                    {
                        return Err(invalid);
                    }
                    targets.insert(
                        method.symbol,
                        SourceMethodTarget {
                            source,
                            declaration,
                            name,
                        },
                    );
                }
            }
            ranges.push((range, class.symbol));
            previous_end = range.end();
        }
        Ok(Self {
            names,
            classes: ranges,
            targets,
        })
    }

    /// The original call names, usable for syntax-only lookup.
    pub fn call_names(&self) -> &SourceCallNames {
        self.names.tokens()
    }

    fn class_at(&self, offset: u32) -> Option<SymbolId> {
        let i = self
            .classes
            .partition_point(|(range, _)| range.end() <= offset);
        let &(range, symbol) = self.classes.get(i)?;
        range.contains(offset).then_some(symbol)
    }

    /// Resolve a call in a successfully checked member even when another member failed.
    /// Supply the current check for the enclosing class from this session.
    pub fn checked_method_at<'a>(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        check: impl FnOnce(SymbolId) -> Option<&'a crate::ClassBodyCheck>,
    ) -> Option<SymbolId> {
        let call = self.call_names().call_at(offset)?;
        if ast.try_get(call)?.position?.source() != self.call_names().source() {
            return None;
        }
        let bindings = check(self.class_at(offset)?)?.bindings()?;
        crate::jvm_class_plan::selected_method(bindings, call, ast, typed_ast, store)
    }

    /// Follows a checked source call to the defining file, declaration and name token.
    /// Both lookups must return data from the caller's current analysis session.
    pub fn definition_at<'a>(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        check: impl FnOnce(SymbolId) -> Option<&'a crate::ClassBodyCheck>,
        file: impl FnOnce(SourceId) -> Option<&'a SourceFileCalls>,
    ) -> Option<SourceMethodTarget> {
        let method = self.checked_method_at(offset, ast, typed_ast, store, check)?;
        let SymbolOrigin::Source(source) = store.symbols.get(method).origin else {
            return None;
        };
        file(source)?
            .targets
            .get(&method)
            .copied()
            .filter(|target| target.source == source)
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
        body.selected_method_at(offset, self.call_names(), ast, typed_ast, store)
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
