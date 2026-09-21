//! Method completion (Milestone 5c): the info of an ordinary `DEFDEF`.
//!
//! Scala 3.9's `TreeUnpickler` reads a `DEFDEF` as
//!
//! ```scala
//! val paramDefss = readParamss()
//! val tpt = readTpt()
//! sym.info = methodType(paramDefss.nestedMap(_.symbol), tpt.tpe)
//! ```
//!
//! and never looks at the body. This module does the same: it does not read the
//! right-hand side, and it builds no typed tree.
//!
//! ## Clauses
//!
//! `readParamss` groups the header exactly like this (the structural decoder's
//! `header_items` keep the wire order; the grouped `parameters` are not used):
//!
//! | header | clause |
//! |--------|--------|
//! | consecutive `TYPEPARAM`s | one type clause |
//! | consecutive `PARAM`s | one term clause |
//! | a tag change | the next clause |
//! | `SPLITCLAUSE` | an explicit boundary (two term clauses stay two) |
//! | `EMPTYCLAUSE` | an explicit empty term clause |
//!
//! The parameters' *addresses* come from the AST index, never from the
//! structural decoder (its offsets are node-relative). Clause markers are bare
//! tags, not nodes, so the index lists the parameter nodes and then the result
//! tree; the two are aligned by tag and a disagreement is
//! `MalformedDefinition`.
//!
//! ## The info
//!
//! `methodType(paramss, result)`: no clause at all is `ByName { result }`
//! (`def f: T`, Dotty's `ExprType`), while `def f(): T` is a `Method` with no
//! parameters; a type clause is a `Poly`, a term clause a `Method` whose kind
//! is decided by its *first* parameter (`GIVEN` is `Contextual`, else
//! `IMPLICIT` is `Implicit`, else `Plain`, from the entered flags, never from
//! names). The clauses are built from the last to the first, each by
//! [`method_type_from_symbols`] / [`poly_type_from_symbols`], so a reference to
//! a parameter symbol in a later clause or the result becomes a `ParamRef` of
//! the exact binder, and an inner binder mentioning an outer parameter is
//! copied and rebound (nothing stored is mutated). The parameter symbols' own
//! infos keep naming the symbols: symbol identity is source identity, the
//! `ParamRef` is identity inside the binder.
//!
//! Every parameter is completed first, in clause order, by the ordinary
//! completion; a failure (an unresolved class, a tree still deferred) is the
//! method's failure and leaves it `Missing`. Completion never forces another
//! method: a reference through a term still `Missing` is
//! `UnsupportedResolutionPrefix`, so there is no completion cycle to track; the
//! only nested completions are this method's own (and lambda) parameters.
//!
//! ## Parameters
//!
//! * `MethodParam.erased` is the entered `ERASED` flag of the parameter, which
//!   is where Scala 3.9's DEFDEF carries it (upstream turns the flag into an
//!   `ErasedParam` annotation of the info in `fromSymbols`; this model keeps
//!   the boolean, as it does for `METHODtype`'s annotation). Nothing is guessed
//!   from a name or a position.
//! * `MethodParam.varargs` is `false`: it is JVM `ACC_VARARGS`, which TASTy
//!   does not have (a Scala repeated parameter is the `<repeated>` type).
//! * A method type parameter has `declared_variance: None`.
//! * `INLINE`, `TRACKED` and `INTO` on a parameter make `fromSymbols` add an
//!   annotation (or, for `TRACKED`, a result refinement) that `dotty-core`
//!   cannot represent without the annotation classes: the method is
//!   `UnsupportedMethodParameterSemantics`, not completed with it dropped.
//! * Constructors are `ConstructorCompletionDeferred`: their info is the owner
//!   class's effective result type, not the serialized return tree.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{TermName, TypeName};
use dotty_core::symbols::{SymbolFlags, SymbolInfo};
use dotty_core::types::{MethodKind, Type};
use dotty_core::{
    MethodParamSpec, TypeParamSpec, method_type_from_symbols, poly_type_from_symbols,
};
use dotty_tasty::tasty::{
    DefDefHeaderItem, DefinitionTail, EMPTYCLAUSE_TAG, INLINE_TAG, INTO_TAG, TRACKED_TAG,
    TYPEPARAM_TAG,
};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

/// One parameter clause of a method, as addresses of its parameter nodes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Clause {
    /// `[A, B]`.
    Type(Vec<u32>),
    /// `(x: A, y: B)`, never empty.
    Term(Vec<u32>),
    /// `()`.
    EmptyTerm,
}

/// Groups a `DEFDEF` header into clauses the way `readParamss` does.
/// `parameters` are the absolute addresses of the parameter nodes, in wire
/// order; the items must agree with them by tag.
pub(crate) fn clauses_of(
    at: u32,
    items: &[DefDefHeaderItem<'_>],
    parameters: &[(u32, u8)],
) -> Result<Vec<Clause>, UnpickleError> {
    let malformed = |reason| UnpickleError::MalformedDefinition {
        address: at,
        reason,
    };
    let mut clauses: Vec<Clause> = Vec::new();
    let mut next = parameters.iter();
    let mut open: Option<u8> = None;
    for item in items {
        match item {
            DefDefHeaderItem::Parameter(parameter) => {
                let tag = parameter.tag();
                let Some((param_at, node_tag)) = next.next() else {
                    return Err(malformed("more header parameters than parameter nodes"));
                };
                if *node_tag != tag {
                    return Err(malformed("a header parameter disagrees with its node"));
                }
                match (open, clauses.last_mut()) {
                    (Some(current), Some(Clause::Type(group) | Clause::Term(group)))
                        if current == tag =>
                    {
                        group.push(*param_at);
                    }
                    _ => clauses.push(if tag == TYPEPARAM_TAG {
                        Clause::Type(vec![*param_at])
                    } else {
                        Clause::Term(vec![*param_at])
                    }),
                }
                open = Some(tag);
            }
            DefDefHeaderItem::Clause(EMPTYCLAUSE_TAG) => {
                clauses.push(Clause::EmptyTerm);
                open = None;
            }
            DefDefHeaderItem::Clause(_) => open = None,
        }
    }
    if next.next().is_some() {
        return Err(malformed("more parameter nodes than header parameters"));
    }
    Ok(clauses)
}

impl TastyUnpickler<'_, '_, '_> {
    /// The info of the ordinary method `symbol`, the `DEFDEF` at `at`. Sets
    /// `SymbolInfo::Complete`; not atomic by itself (the public entry points
    /// roll back).
    pub(crate) fn complete_method(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        symbol: SymbolId,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let body = ast.node(at)?.decode_defdef_body()?;
        let kids: Vec<(u32, u8)> = ast
            .children(at)
            .iter()
            .map(|child| (address(child.offset), child.tag))
            .collect();
        let count = body
            .header_items
            .iter()
            .filter(|item| matches!(item, DefDefHeaderItem::Parameter(_)))
            .count();
        let Some(&(result_at, _)) = kids.get(count) else {
            return Err(UnpickleError::MalformedDefinition {
                address: at,
                reason: "a method has a result type after its parameters",
            });
        };
        let clauses = clauses_of(at, &body.header_items, &kids[..count])?;

        // A parameter modifier whose adaptation is not modeled is refused
        // before anything is completed. The parameter items and the parameter
        // nodes were aligned by `clauses_of`, so they zip.
        let items = body
            .header_items
            .iter()
            .filter_map(|item| match item {
                DefDefHeaderItem::Parameter(parameter) => Some(parameter),
                DefDefHeaderItem::Clause(_) => None,
            })
            .zip(&kids[..count]);
        for (parameter, (param_at, _)) in items {
            for entry in &parameter.decode_body()?.tail {
                if let DefinitionTail::Modifier(tag @ (INLINE_TAG | TRACKED_TAG | INTO_TAG)) = entry
                {
                    return Err(UnpickleError::UnsupportedMethodParameterSemantics {
                        address: *param_at,
                        tag: *tag,
                    });
                }
            }
        }

        // Parameters first, in clause order: a later one may depend on an
        // earlier one's completed type.
        for clause in &clauses {
            if let Clause::Type(params) | Clause::Term(params) = clause {
                for param in params {
                    self.complete_in(ast, *param, depth)?;
                }
            }
        }
        let result = self.type_of_tpt(ast, result_at, at, depth)?;

        let mut current = result;
        for clause in clauses.iter().rev() {
            current = self.build_clause(at, clause, current)?;
        }
        if clauses.is_empty() {
            // `paramss.isEmpty`: `def f: T`.
            current = self.store.types.alloc(Type::ByName { result: current });
        }
        self.set_symbol_info(symbol, SymbolInfo::Complete(current));
        Ok(current)
    }

    /// One clause around `inner`, by symbol abstraction.
    fn build_clause(
        &mut self,
        at: u32,
        clause: &Clause,
        inner: TypeId,
    ) -> Result<TypeId, UnpickleError> {
        let abstraction = |error| UnpickleError::ParameterAbstraction { address: at, error };
        match clause {
            Clause::EmptyTerm => {
                method_type_from_symbols(self.store, &[], inner, MethodKind::Plain)
                    .map_err(abstraction)
            }
            Clause::Type(params) => {
                let mut specs = Vec::with_capacity(params.len());
                for param in params {
                    let (symbol, info) = self.parameter_symbol(*param)?;
                    specs.push(TypeParamSpec {
                        symbol,
                        name: TypeName::new(self.store.symbols.get(symbol).name.text()),
                        bounds: info,
                        // A method type parameter is a `PolyType` parameter,
                        // not a variance-bearing lambda parameter.
                        declared_variance: None,
                    });
                }
                poly_type_from_symbols(self.store, &specs, inner).map_err(abstraction)
            }
            Clause::Term(params) => {
                let mut specs = Vec::with_capacity(params.len());
                let mut kind = MethodKind::Plain;
                for (position, param) in params.iter().enumerate() {
                    let (symbol, info) = self.parameter_symbol(*param)?;
                    let flags = self.store.symbols.get(symbol).flags;
                    if position == 0 {
                        // `NamerOps.methodType`: the first parameter decides.
                        kind = if flags.contains(SymbolFlags::GIVEN) {
                            MethodKind::Contextual
                        } else if flags.contains(SymbolFlags::IMPLICIT) {
                            MethodKind::Implicit
                        } else {
                            MethodKind::Plain
                        };
                    }
                    specs.push(MethodParamSpec {
                        symbol,
                        name: TermName::new(self.store.symbols.get(symbol).name.text()),
                        ty: info,
                        erased: flags.contains(SymbolFlags::ERASED),
                        varargs: false,
                    });
                }
                method_type_from_symbols(self.store, &specs, inner, kind).map_err(abstraction)
            }
        }
    }

    /// The entered symbol of the parameter node at `param` and its completed
    /// info.
    fn parameter_symbol(&self, param: u32) -> Result<(SymbolId, TypeId), UnpickleError> {
        let Some(symbol) = self.index.symbol_at(param) else {
            return Err(UnpickleError::MissingEnteredSymbol { address: param });
        };
        match self.store.symbols.get(symbol).info {
            SymbolInfo::Complete(info) => Ok((symbol, info)),
            _ => Err(UnpickleError::SymbolCompletionDeferred { address: param }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::{PARAM_TAG, ParameterNode, SPLITCLAUSE_TAG};

    fn item(tag: u8) -> DefDefHeaderItem<'static> {
        match tag {
            TYPEPARAM_TAG => DefDefHeaderItem::Parameter(ParameterNode::TypeParam {
                name: 0,
                body: &[],
                payload: &[],
                offset: 0,
            }),
            PARAM_TAG => DefDefHeaderItem::Parameter(ParameterNode::TermParam {
                name: 0,
                body: &[],
                payload: &[],
                offset: 0,
            }),
            marker => DefDefHeaderItem::Clause(marker),
        }
    }

    const T: u8 = TYPEPARAM_TAG;
    const P: u8 = PARAM_TAG;
    const EMPTY: u8 = EMPTYCLAUSE_TAG;
    const SPLIT: u8 = SPLITCLAUSE_TAG;

    /// The clauses of a header written as tags; parameter `i` is at address
    /// `100 + i`.
    fn clauses(header: &[u8]) -> Result<Vec<Clause>, UnpickleError> {
        let items: Vec<DefDefHeaderItem<'static>> = header.iter().map(|tag| item(*tag)).collect();
        let parameters: Vec<(u32, u8)> = header
            .iter()
            .filter(|tag| matches!(**tag, T | P))
            .enumerate()
            .map(|(position, tag)| (100 + u32::try_from(position).unwrap(), *tag))
            .collect();
        clauses_of(7, &items, &parameters)
    }

    #[test]
    fn consecutive_parameters_of_one_tag_are_one_clause_and_a_tag_change_starts_the_next() {
        assert_eq!(
            clauses(&[T, T, P, P, P]).unwrap(),
            vec![
                Clause::Type(vec![100, 101]),
                Clause::Term(vec![102, 103, 104])
            ]
        );
        // A term clause, then a type clause, then a term clause again.
        assert_eq!(
            clauses(&[P, T, P]).unwrap(),
            vec![
                Clause::Term(vec![100]),
                Clause::Type(vec![101]),
                Clause::Term(vec![102])
            ]
        );
    }

    #[test]
    fn a_split_clause_is_a_boundary_between_two_term_clauses() {
        assert_eq!(
            clauses(&[P, SPLIT, P, P]).unwrap(),
            vec![Clause::Term(vec![100]), Clause::Term(vec![101, 102])]
        );
        // Without the marker the same parameters are one clause.
        assert_eq!(
            clauses(&[P, P, P]).unwrap(),
            vec![Clause::Term(vec![100, 101, 102])]
        );
    }

    #[test]
    fn an_empty_clause_is_an_empty_term_clause_in_its_place() {
        assert_eq!(
            clauses(&[EMPTY, P]).unwrap(),
            vec![Clause::EmptyTerm, Clause::Term(vec![100])]
        );
        assert_eq!(
            clauses(&[T, EMPTY]).unwrap(),
            vec![Clause::Type(vec![100]), Clause::EmptyTerm]
        );
        // Two empty clauses stay two.
        assert_eq!(
            clauses(&[EMPTY, EMPTY]).unwrap(),
            vec![Clause::EmptyTerm, Clause::EmptyTerm]
        );
    }

    #[test]
    fn no_header_is_no_clause() {
        assert_eq!(clauses(&[]).unwrap(), vec![]);
    }

    #[test]
    fn a_header_that_disagrees_with_the_parameter_nodes_is_malformed() {
        let items = vec![item(P), item(P)];
        let malformed = |parameters: &[(u32, u8)]| clauses_of(7, &items, parameters);
        // Fewer nodes than header parameters.
        assert!(matches!(
            malformed(&[(100, P)]),
            Err(UnpickleError::MalformedDefinition { address: 7, .. })
        ));
        // More nodes than header parameters.
        assert!(matches!(
            malformed(&[(100, P), (101, P), (102, P)]),
            Err(UnpickleError::MalformedDefinition { address: 7, .. })
        ));
        // A node of another tag.
        assert!(matches!(
            malformed(&[(100, P), (101, T)]),
            Err(UnpickleError::MalformedDefinition { address: 7, .. })
        ));
    }
}
