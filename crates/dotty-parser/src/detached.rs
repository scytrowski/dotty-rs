//! Parse one file with worker-local names, then bind it to a semantic session.

use dotty_core::ast::{Modifiers, UntypedNode, VisibilitySyntax};
use dotty_core::{
    Constant, Name, NameId, NameInterner, SourceId, SourceText, TermName, TokenSource, TreeKind,
    TypeName, Untyped,
};

use crate::{ParseResult, Parser, ParserFeatures};

/// A parsed compilation unit whose tree and diagnostic names still belong
/// to a private interner. A worker can return this value to its caller.
pub struct DetachedParse {
    result: ParseResult,
    names: NameInterner,
}

impl DetachedParse {
    /// Parses one source without borrowing the session's semantic store.
    pub fn parse<S: TokenSource>(
        source: SourceText<'_>,
        id: SourceId,
        tokens: S,
        features: ParserFeatures,
    ) -> Self {
        let mut names = NameInterner::new();
        let result = Parser::new(source, id, tokens, &mut names)
            .with_features(features)
            .source_compilation_unit();
        Self { result, names }
    }

    /// Interns this unit's spellings in their original insertion order and
    /// translates every local name in place. Tree IDs, source spans and
    /// diagnostics survive the transfer. Bind files in stable input order
    /// to obtain the same session name IDs as sequential parsing.
    pub fn into_session(mut self, session: &mut NameInterner) -> ParseResult {
        let names = SessionNames(self.names.spellings().map(|s| session.intern(s)).collect());
        for node in self.result.ast.iter_mut() {
            names.remap_tree(&mut node.kind);
        }
        for diagnostic in &mut self.result.diagnostics {
            diagnostic.remap_names(|name| names.name(name));
        }
        self.result
    }
}

struct SessionNames(Vec<NameId>);

impl SessionNames {
    fn id(&self, local: NameId) -> NameId {
        self.0[local.index() as usize]
    }

    fn name(&self, local: Name) -> Name {
        Name::new(self.id(local.text()), local.namespace())
    }

    fn term(&self, local: TermName) -> TermName {
        TermName::new(self.id(local.as_name().text()))
    }

    fn ty(&self, local: TypeName) -> TypeName {
        TypeName::new(self.id(local.as_name().text()))
    }

    fn modifiers(&self, modifiers: &mut Modifiers) {
        if let Some(
            VisibilitySyntax::Private { qualifier } | VisibilitySyntax::Protected { qualifier },
        ) = &mut modifiers.visibility
        {
            *qualifier = qualifier.map(|name| self.name(name));
        }
    }

    // Visit each arena slot exactly once, including unreachable recovery nodes.
    // Child TreeIds already refer to their final slot in this file.
    fn remap_tree(&self, kind: &mut TreeKind<Untyped>) {
        use TreeKind::*;
        match kind {
            Ident(node) => node.name = self.name(node.name),
            Select(node) => node.name = self.name(node.name),
            This(node) => node.qual = node.qual.map(|name| self.name(name)),
            Super(node) => node.mix = node.mix.map(|name| self.name(name)),
            Literal(node) => {
                if let Constant::String(id) = &mut node.value {
                    *id = self.id(*id);
                }
            }
            NamedArg(node) => node.name = self.name(node.name),
            Bind(node) => node.name = self.name(node.name),
            ValDef(node) => {
                node.name = self.term(node.name);
                self.modifiers(&mut node.metadata);
            }
            DefDef(node) => {
                node.name = self.term(node.name);
                self.modifiers(&mut node.metadata);
            }
            TypeDef(node) => {
                node.name = self.ty(node.name);
                self.modifiers(&mut node.metadata);
            }
            Import(dotty_core::ast::Import { selectors, .. })
            | Export(dotty_core::ast::Export { selectors, .. }) => {
                for selector in selectors {
                    selector.imported = self.name(selector.imported);
                }
            }
            PhaseSpecific(node) => self.remap_surface(node),
            Apply(_) | TypeApply(_) | New(_) | Typed(_) | Assign(_) | Block(_) | If(_)
            | Match(_) | CaseDef(_) | Return(_) | While(_) | Try(_) | Closure(_) | Template(_)
            | PackageDef(_) | TypeTree(_) | SingletonTypeTree(_) | AppliedTypeTree(_)
            | RefinedTypeTree(_) | LambdaTypeTree(_) | MatchTypeTree(_) | ByNameTypeTree(_)
            | TypeBoundsTree(_) | Alternative(_) | UnApply(_) | Annotated(_) | Quote(_)
            | Splice(_) | QuotePattern(_) | SplicePattern(_) | Inlined(_) => {}
        }
    }

    fn remap_surface(&self, node: &mut UntypedNode) {
        use UntypedNode::*;
        match node {
            ModuleDef(node) => {
                node.name = self.term(node.name);
                self.modifiers(&mut node.metadata);
            }
            FunctionWithMods(node) => self.modifiers(&mut node.modifiers),
            PatDef(node) => self.modifiers(&mut node.modifiers),
            InfixOp(node) => node.op = self.name(node.op),
            PrefixOp(node) => node.op = self.name(node.op),
            PostfixOp(node) => node.op = self.name(node.op),
            InterpolatedString(node) => node.prefix = self.name(node.prefix),
            ContextBoundTypeTree(node) => {
                node.parameter = self.ty(node.parameter);
                node.name = node.name.map(|name| self.term(name));
            }
            Number(node) => node.text = self.id(node.text),
            Error(_) | Function(_) | CapturesAndResult(_) | PolyFunction(_) | Parens(_)
            | Tuple(_) | ForYield(_) | ForDo(_) | GenFrom(_) | GenAlias(_)
            | ExtensionMethods(_) | ContextBounds(_) | Throw(_) | InlineIf(_) | InlineMatch(_)
            | MacroTree(_) | ParsedTry(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExpressionIssue, ModifierIssue, ParseDiagnostic, ParseIssue, PatternIssue, TypeIssue,
    };
    use dotty_core::{Namespace, SourceSpan, Span, TextRange};

    #[test]
    fn operator_and_modifier_issues_retain_their_spelling_namespace_and_span() {
        let mut local = NameInterner::new();
        let mut session = NameInterner::new();
        session.intern("unrelated");
        let issues = |names: &mut NameInterner| {
            let left = Name::new(names.intern("+"), Namespace::Term);
            let right = Name::new(names.intern("+:"), Namespace::Term);
            [
                ParseIssue::Expression(ExpressionIssue::MixedAssociativityOperators {
                    left,
                    right,
                }),
                ParseIssue::Pattern(PatternIssue::MixedAssociativityOperators { left, right }),
                ParseIssue::Type(TypeIssue::MixedAssociativityTypeOperators {
                    left: Name::new(left.text(), Namespace::Type),
                    right: Name::new(right.text(), Namespace::Type),
                }),
                ParseIssue::Modifier(ModifierIssue::UnsupportedContextualModifier { name: left }),
            ]
        };
        let local_issues = issues(&mut local);
        let span = SourceSpan::new(
            SourceId::from_index(7),
            Span::without_point(TextRange::new(2, 4).unwrap()),
        );
        let names = SessionNames(local.spellings().map(|s| session.intern(s)).collect());
        for (issue, expected) in local_issues.into_iter().zip(issues(&mut session)) {
            let mut diagnostic = ParseDiagnostic::with_issue(span, issue);
            diagnostic.remap_names(|name| names.name(name));
            assert_eq!(diagnostic, ParseDiagnostic::with_issue(span, expected));
        }
    }
}
