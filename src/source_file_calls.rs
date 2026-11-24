use std::{collections::HashMap, sync::Arc};

use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceSpan,
    SourceTypedIndex, Span, SymbolId, SymbolOrigin, TextRange, TreeId, TreeKind, Typed, Untyped,
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

/// A written direct call resolved to a source method in the same revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSourceCall {
    /// Half-open UTF-8 byte range of the written callee token in the caller.
    pub origin: SourceSpan,
    /// Selected overload in the originating semantic store.
    pub method: SymbolId,
    /// Source ranges of that overload in its defining file.
    pub target: SourceMethodTarget,
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
        self.method_at_with_bindings(offset, ast, typed_ast, store, |owner| {
            check(owner)?.bindings()
        })
    }

    fn method_at_with_bindings<'a>(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        bindings: impl FnOnce(SymbolId) -> Option<&'a SourceTypedIndex>,
    ) -> Option<SymbolId> {
        let entry = self.call_names().call_name_at(offset)?;
        self.method_for_call(entry, ast, typed_ast, store, bindings)
    }

    fn method_for_call<'a>(
        &self,
        (name, call): (TextRange, TreeId<Untyped>),
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        bindings: impl FnOnce(SymbolId) -> Option<&'a SourceTypedIndex>,
    ) -> Option<SymbolId> {
        if ast.try_get(call)?.position?.source() != self.call_names().source() {
            return None;
        }
        let bindings = bindings(self.class_at(name.start())?)?;
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
        self.definition_at_with_bindings(
            offset,
            ast,
            typed_ast,
            store,
            |owner| check(owner)?.bindings(),
            file,
        )
    }

    /// Resolves a direct call after its class report has been consumed. Return the
    /// enclosing class's committed [`crate::CheckedClassBody::bindings`] and the
    /// target file's navigation index from the same session. A failed member or
    /// missing mapping yields `None`. Positions are UTF-8 byte offsets.
    pub fn definition_at_with_bindings<'a>(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        bindings: impl FnOnce(SymbolId) -> Option<&'a SourceTypedIndex>,
        file: impl FnOnce(SourceId) -> Option<&'a SourceFileCalls>,
    ) -> Option<SourceMethodTarget> {
        let (source, method) = self.source_method_at(offset, ast, typed_ast, store, bindings)?;
        file(source)?.target(source, method)
    }

    fn source_method_at<'a>(
        &self,
        offset: u32,
        ast: &AstArena<Untyped>,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
        bindings: impl FnOnce(SymbolId) -> Option<&'a SourceTypedIndex>,
    ) -> Option<(SourceId, SymbolId)> {
        let method = self.method_at_with_bindings(offset, ast, typed_ast, store, bindings)?;
        let SymbolOrigin::Source(source) = store.symbols.get(method).origin else {
            return None;
        };
        Some((source, method))
    }

    fn target(&self, source: SourceId, method: SymbolId) -> Option<SourceMethodTarget> {
        self.targets
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

impl crate::NamedClassSources<'_> {
    /// Resolves a direct parenthesized method call in a committed body member.
    /// Supply a UTF-8 byte offset inside the callee name and the bodies, typed
    /// arena and store of this naming session. The returned ranges belong to
    /// the defining file in the same revision. Either file's navigation index
    /// can return [`DeclarationError`]. Missing files, absent bindings, and
    /// positions outside a supported call (including the token end) yield `Ok(None)`.
    pub fn definition_at(
        &self,
        source: SourceId,
        offset: u32,
        bodies: &crate::CheckedClassBodies,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Result<Option<SourceMethodTarget>, DeclarationError> {
        Ok(self
            .resolved_call_at(source, offset, bodies, typed_ast, store)?
            .map(|call| call.target))
    }

    /// Resolves a byte inside the written name of a direct parenthesized source
    /// call using this session's committed body mappings, including partial bodies.
    /// Returns the caller token, exact overload and target ranges together. Missing
    /// bindings, source declarations or tokens yield `Ok(None)`; indexing either
    /// file can return its original declaration error.
    pub fn resolved_call_at(
        &self,
        source: SourceId,
        offset: u32,
        bodies: &crate::CheckedClassBodies,
        typed_ast: &AstArena<Typed>,
        store: &SemanticStore,
    ) -> Result<Option<ResolvedSourceCall>, DeclarationError> {
        let Some(caller) = self.file(source) else {
            return Ok(None);
        };
        let calls = caller.navigation()?;
        let Some(entry) = calls.call_names().call_name_at(offset) else {
            return Ok(None);
        };
        let Some(method) = calls.method_for_call(entry, caller.ast(), typed_ast, store, |owner| {
            bodies.bindings(owner)
        }) else {
            return Ok(None);
        };
        self.resolved_target(source, entry.0, method, store)
    }

    fn resolved_target(
        &self,
        caller: SourceId,
        name: TextRange,
        method: SymbolId,
        store: &SemanticStore,
    ) -> Result<Option<ResolvedSourceCall>, DeclarationError> {
        let SymbolOrigin::Source(source) = store.symbols.get(method).origin else {
            return Ok(None);
        };
        let Some(provider) = self.file(source) else {
            return Ok(None);
        };
        Ok(provider
            .navigation()?
            .target(source, method)
            .map(|target| ResolvedSourceCall {
                origin: SourceSpan::new(caller, Span::without_point(name)),
                method,
                target,
            }))
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
