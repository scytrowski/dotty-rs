//! Owned application failures from the source typer.
use crate::core::{
    AstArena, Definitions, SourceId, SourceSpan, SymbolId, TreeId, TreeKind, TypeId, Untyped,
};
use crate::typer::TyperError;
use crate::{SourceDiagnostic, typer_error_span_with};

/// Key for a retry of one member's failed call within a single analysis.
/// Keep keys only for the lifetime of one immutable set of parsed files and named symbols.
/// A fresh analysis can reuse every source, tree, symbol and type index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallErrorIdentity {
    member: SymbolId,
    source: SourceId,
    call: TreeId<Untyped>,
    cause: CallCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CallCause {
    Arity {
        expected: usize,
        actual: usize,
    },
    Argument {
        index: usize,
        expected: TypeId,
        actual: TypeId,
    },
}

/// An original typed call failure with the source position resolved during analysis.
/// It can outlive the typer and both arenas. Type IDs inside the error describe
/// the failed attempt and may have been rolled back. Its owned descriptions remain valid.
#[derive(Debug)]
pub struct CallDiagnostic {
    error: TyperError,
    member: Option<SymbolId>,
    call: Option<TreeId<Untyped>>,
    source: SourceId,
    location: Option<SourceSpan>,
}

impl CallDiagnostic {
    /// Takes ownership of an arity or argument type mismatch. Other errors are
    /// returned intact without requesting a tree. Supply the original untyped
    /// arena for the requested source, or `None` to retain an unlocated failure.
    /// Pass the canonical enclosing member when known. A raw phase failure
    /// without that context can still be presented and inspected.
    pub fn from_typer<'a>(
        error: TyperError,
        member: Option<SymbolId>,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Result<Self, TyperError> {
        let (source, tree_index) = match error {
            TyperError::ApplicationArityMismatch {
                source, tree_index, ..
            }
            | TyperError::ApplicationArgumentTypeMismatch {
                source, tree_index, ..
            } => (source, tree_index),
            _ => return Err(error),
        };
        let ast = ast_for(source);
        let location = typer_error_span_with(&error, |_| ast);
        let call = ast
            .and_then(|ast| ast.iter().nth(tree_index as usize))
            .filter(|(_, tree)| matches!(tree.kind, TreeKind::Apply(_)) && location.is_some())
            .map(|(id, _)| id);
        Ok(Self {
            error,
            member,
            call,
            source,
            location,
        })
    }

    /// Canonical enclosing member when supplied by the caller or a phase wrapper.
    pub fn member(&self) -> Option<SymbolId> {
        self.member
    }

    /// Returns a key only for an identifiable explicit call in a known member.
    /// Use the original, unmodified bootstrap definitions from this analysis.
    /// Arity keys preserve both counts. Type keys preserve the argument index and
    /// both canonical builtin types. Other type slots may be reused after rollback,
    /// so keep both reports when this returns `None`.
    pub fn identity(&self, defs: &Definitions) -> Option<CallErrorIdentity> {
        let member = self.member?;
        let call = self.call?;
        let cause = match self.error {
            TyperError::ApplicationArityMismatch {
                actual, expected, ..
            } => CallCause::Arity { expected, actual },
            TyperError::ApplicationArgumentTypeMismatch {
                actual,
                expected,
                argument_index,
                ..
            } => {
                let builtins = [
                    defs.byte,
                    defs.short,
                    defs.char,
                    defs.int,
                    defs.long,
                    defs.float,
                    defs.double,
                    defs.boolean,
                    defs.unit,
                    defs.object_type,
                    defs.any_type,
                    defs.matchable_type,
                    defs.nothing_type,
                    defs.null_type,
                ];
                if !builtins.contains(&expected) || !builtins.contains(&actual) {
                    return None;
                }
                CallCause::Argument {
                    index: argument_index,
                    expected,
                    actual,
                }
            }
            _ => return None,
        };
        Some(CallErrorIdentity {
            member,
            source: self.source,
            call,
            cause,
        })
    }

    /// Original payload, including argument index or arity counts.
    pub fn error(&self) -> &TyperError {
        &self.error
    }

    /// Stable machine-readable category, independent of the argument types and counts.
    pub fn code(&self) -> &'static str {
        self.error
            .call_error_code()
            .expect("validated call failure")
    }

    /// Original file identity, available even without its tree.
    pub fn source(&self) -> SourceId {
        self.source
    }

    /// UTF-8 byte span of the call for arity, or the rejected argument for type mismatch.
    pub fn location(&self) -> Option<SourceSpan> {
        self.location
    }

    /// Formats the default message without querying a source or semantic arena.
    pub fn render(&self) -> SourceDiagnostic {
        SourceDiagnostic::typer_at(&self.error, self.location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Definitions, NameInterner, SemanticStore, TreeKind};

    #[test]
    fn keys_distinguish_source_member_call_and_payload() {
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        let source = SourceId::from_index(0);
        let owner = defs.object_class;
        let mut names = NameInterner::new();
        let files = [0, 1].map(|i| {
            crate::parse_source(
                SourceId::from_index(i),
                "class A { def f() = g(1); def h() = g(1) }",
                Default::default(),
            )
            .unwrap()
            .bind(&mut names)
        });
        let key = |source: SourceId, member, ordinal, expected, actual| {
            let ast = &files[source.index() as usize].syntax.ast;
            let (call, _) = ast
                .iter()
                .filter(|(_, t)| matches!(t.kind, TreeKind::Apply(_)))
                .nth(ordinal)
                .unwrap();
            CallDiagnostic::from_typer(
                TyperError::ApplicationArityMismatch {
                    source,
                    tree_index: call.index(),
                    expected,
                    actual,
                },
                member,
                |_| Some(ast),
            )
            .unwrap()
            .identity(&defs)
        };
        let first = key(source, Some(owner), 0, 0, 1).unwrap();
        assert_eq!(Some(first), key(source, Some(owner), 0, 0, 1));
        for other in [
            key(source, Some(owner), 1, 0, 1),
            key(SourceId::from_index(1), Some(owner), 0, 0, 1),
            key(source, Some(defs.any_class), 0, 0, 1),
            key(source, Some(owner), 0, 2, 1),
            key(source, Some(owner), 0, 0, 2),
        ] {
            assert_ne!(first, other.unwrap());
        }
        assert_eq!(key(source, None, 0, 0, 1), None);
    }

    #[test]
    fn retains_payload_and_source_with_or_without_the_original_tree() {
        let source = SourceId::from_index(3);
        let text = "class A { def f() = only(false) }";
        let parsed = crate::parse_source(source, text, Default::default())
            .unwrap()
            .bind(&mut NameInterner::new());
        let ast = &parsed.syntax.ast;
        let (call, _) = ast
            .iter()
            .find(|(_, tree)| matches!(tree.kind, TreeKind::Apply(_)))
            .unwrap();
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        for available in [true, false] {
            for (error, site) in [
                (
                    TyperError::ApplicationArityMismatch {
                        source,
                        tree_index: call.index(),
                        expected: 0,
                        actual: 1,
                    },
                    "only(false)",
                ),
                (
                    TyperError::ApplicationArgumentTypeMismatch {
                        source,
                        tree_index: call.index(),
                        argument_index: 0,
                        expected: defs.int,
                        actual: defs.boolean,
                        expected_description: "Int".into(),
                        actual_description: "Boolean".into(),
                    },
                    "false",
                ),
            ] {
                let lookup = |id| {
                    assert_eq!(id, source);
                    available.then_some(ast)
                };
                let rendered = SourceDiagnostic::from_typer(&error, lookup);
                let d = CallDiagnostic::from_typer(error, None, lookup).unwrap();
                assert_eq!(d.source(), source);
                assert_eq!(d.error().call_error_code(), Some(d.code()));
                assert_eq!(d.render(), rendered);
                assert_eq!(
                    d.location().map(|span| {
                        assert_eq!(span.source(), source);
                        let range = span.span().range();
                        &text[range.start() as usize..range.end() as usize]
                    }),
                    available.then_some(site)
                );
            }
        }
        assert!(matches!(
            CallDiagnostic::from_typer(
                TyperError::UsingApplicationDeferred {
                    source,
                    tree_index: call.index()
                },
                None,
                |_| panic!("unsupported error queried an AST")
            ),
            Err(TyperError::UsingApplicationDeferred { .. })
        ));
    }
}

#[cfg(test)]
mod member_tests {
    use super::*;
    use crate::core::{NameInterner, SemanticStore, SymbolKind, Type};
    use crate::source_declarations::{ClassMember, Declaration};
    use crate::{ClassBodyError, MemberSignatureError, SourceDeclaration};

    #[test]
    fn phase_retries_match_only_permanent_types_after_rollback() {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(7);
        let parsed = crate::parse_source(
            source,
            "class A { def f() = take(false, false) }",
            Default::default(),
        )
        .unwrap()
        .bind(&mut NameInterner::new());
        let ast = &parsed.syntax.ast;
        let find = |predicate: fn(&TreeKind<Untyped>) -> bool| {
            ast.iter().find(|(_, t)| predicate(&t.kind)).unwrap().0
        };
        let tree = find(|k| matches!(k, TreeKind::DefDef(_)));
        let call = find(|k| matches!(k, TreeKind::Apply(_)));
        let member = defs.object_class;
        let class = defs.any_class;
        let lookup = |id| (id == source).then_some(ast);
        let mismatch = |argument_index, actual, expected, actual_description: &str| {
            TyperError::ApplicationArgumentTypeMismatch {
                source,
                tree_index: call.index(),
                argument_index,
                expected,
                actual,
                actual_description: actual_description.into(),
                expected_description: "Int".into(),
            }
        };
        let signature = |error| {
            MemberSignatureError::Typing {
                member,
                declaration: SourceDeclaration {
                    source,
                    class,
                    kind: SymbolKind::Method,
                },
                error,
            }
            .into_call_diagnostic(lookup)
            .unwrap()
        };
        let body = |error| {
            ClassBodyError::Member {
                class,
                member: ClassMember::Method(Declaration {
                    tree,
                    symbol: member,
                }),
                error,
            }
            .into_call_diagnostic(lookup)
            .unwrap()
        };

        let error = || mismatch(0, defs.boolean, defs.int, "Boolean");
        let first = signature(error());
        let key = first.identity(&defs).unwrap();
        assert_eq!(first.member(), Some(member));
        assert_eq!(body(error()).identity(&defs), Some(key));
        for other in [
            mismatch(1, defs.boolean, defs.int, "Boolean"),
            mismatch(0, defs.byte, defs.int, "Byte"),
            mismatch(0, defs.boolean, defs.long, "Boolean"),
        ] {
            assert_ne!(body(other).identity(&defs).unwrap(), key);
        }

        let before = store.checkpoint();
        let slot = store.types.alloc(Type::JavaArray { element: defs.int });
        let error = mismatch(0, slot, defs.int, "Array[Int]");
        store.rollback_to(before);
        let reused = store.types.alloc(Type::JavaArray { element: defs.byte });
        assert_eq!(slot, reused);
        let original = signature(error);
        let later = body(mismatch(0, reused, defs.int, "Array[Byte]"));
        assert_eq!(original.identity(&defs), None);
        assert_eq!(later.identity(&defs), None);
        drop(store);
        assert_eq!(original.render().message, "Expected Int, found Array[Int]");
        assert_eq!(later.render().message, "Expected Int, found Array[Byte]");
        assert!(
            matches!(ClassBodyError::InvalidClass(class).into_call_diagnostic(|_| unreachable!()), Err(ClassBodyError::InvalidClass(id)) if id == class)
        );
        assert!(
            matches!(MemberSignatureError::InvalidMember(member).into_call_diagnostic(|_| unreachable!()), Err(MemberSignatureError::InvalidMember(id)) if id == member)
        );
    }
}
