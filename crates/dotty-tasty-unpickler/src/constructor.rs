//! Constructor completion (Milestone 5d2a): `Constructor` `DEFDEF` symbols
//! become `Complete`.
//!
//! Scala 3.9's `TreeUnpickler.readNewDef` reads a constructor exactly like an
//! ordinary method (`method`), except for its result:
//!
//! ```scala
//! val paramDefss = readParamss()
//! val tpt = readTpt()
//! val paramss = paramDefss.nestedMap(_.symbol)
//! val normalizedParamss = normalizeIfConstructor(paramss, name == nme.CONSTRUCTOR)
//! val resType =
//!   if name == nme.CONSTRUCTOR then effectiveResultType(sym, normalizedParamss)
//!   else tpt.tpe
//! sym.info = methodType(normalizedParamss, resType)
//! ```
//!
//! **The serialized return type tree is not the constructor's semantic
//! result.** `readTpt()` still runs (its validation, and any type-tree
//! projection cache it populates, stay real), but its `TypeId` is discarded:
//! the constructed-instance type is built from the owner class instead, never
//! from the wire tree.
//!
//! **Constructor completion does not require, and never triggers, the owner
//! class's `ClassInfo` completion.** The effective result only needs the
//! owner's `SymbolId` and its `no_prefix` canonical reference (the same
//! convention [`class`](crate::class) publishes `ClassInfo.prefix` with), not
//! its parents, self type or declaration scope. A class whose `ClassInfo` is
//! blocked on an external parent can still give its constructor a complete
//! info. If the owner *is* already `Complete(ClassInfo)`, that info is
//! checked for the two invariants class completion always holds
//! (`class == owner`, `prefix == no_prefix`): a mismatch is a semantic-state
//! error, not silently accepted.
//!
//! ## Normalization
//!
//! [`normalize_if_constructor`] mirrors `NamerOps.normalizeIfConstructor`
//! over the already-grouped [`Clause`](crate::method::Clause) sequence, once
//! every clause's parameters are completed (the rule reads parameter flags):
//!
//! * a leading type clause is kept, and the rest is normalized recursively;
//! * otherwise, if the first clause is a non-empty term clause whose first
//!   parameter is `IMPLICIT`, an empty ordinary clause is prepended;
//! * otherwise, if every term clause (type clauses do not disqualify; an
//!   explicit empty clause does, since it already satisfies the invariant
//!   below) is non-empty and `GIVEN`, an empty ordinary clause is appended;
//! * otherwise the clauses are unchanged.
//!
//! This guarantees every constructor ends with at least one non-implicit,
//! non-contextual term clause, exactly as upstream's comment on
//! `normalizeIfConstructor` says. The inserted clause is a real empty
//! `Clause::EmptyTerm`, which [`TastyUnpickler::build_clause`] turns into an
//! ordinary empty `Type::Method` binder like any other: normalization never
//! touches the wire or the AST, only the semantic clause sequence built from
//! it.
//!
//! ## Effective result
//!
//! Before any clause wraps it, the result starts as the owner class applied
//! to its own leading type clause, if the *normalized* clauses start with one
//! (Dotty's `ctor.owner.typeRef.appliedTo(tparams.map(_.typeRef))`); otherwise
//! it is the owner's plain reference. Both are built with
//! [`Definitions::no_prefix`](dotty_core::Definitions), the repository's
//! canonical prefix convention, never a copied `ClassInfo.prefix`, a fresh
//! `ThisType` or a textual lookup.
//!
//! A constructor type-parameter symbol is **not** the class header's type
//! parameter symbol of the same name: TASTy writes them at different
//! addresses, so pass 1 enters two different `SymbolId`s. The effective
//! result names the constructor's own type-parameter symbols (as
//! `TypeRef(no_prefix, constructor_type_param)`); [`TastyUnpickler::build_clause`]'s
//! `poly_type_from_symbols` then abstracts exactly those references into
//! `ParamRef`s of the constructor's own `Poly` binder, the same 5c machinery
//! that abstracts an ordinary method's clauses. No substitution by name and no
//! `SymbolId` equality with the class header's type parameters is ever
//! required or assumed.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::{MethodKind, Type};

use crate::ast_view::AstView;
use crate::error::UnpickleError;
use crate::method::{Clause, defdef_header, guard_parameter_modifiers};
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// Completes the constructor `symbol`, the `DEFDEF` at `at`. Sets
    /// `SymbolInfo::Complete`; not atomic by itself (the public entry points
    /// roll back).
    pub(crate) fn complete_constructor(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        symbol: SymbolId,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let owner = self.constructor_owner(at, symbol)?;

        let header = defdef_header(ast, at)?;
        guard_parameter_modifiers(&header)?;
        self.complete_clause_parameters(ast, &header.clauses, depth)?;

        // `readTpt()` still runs: a malformed or unresolvable serialized
        // return tree is a real completion failure, and any type tree under
        // it (a `LAMBDAtpt`, a selection, ...) is validated exactly as it
        // would be for an ordinary method. The `TypeId` itself is discarded:
        // the effective result below replaces it, and its own type-tree
        // projection cache entry (from `type_of_tpt`) is left as it is,
        // naming a different semantic identity than the constructor's info.
        let _serialized_result = self.type_of_tpt(ast, header.result_at, at, depth)?;

        let normalized = self.normalize_if_constructor(&header.clauses)?;
        let effective_result = self.constructor_effective_result(owner, &normalized)?;
        let current = self.build_signature(at, &normalized, effective_result)?;
        self.set_symbol_info(symbol, SymbolInfo::Complete(current));
        Ok(current)
    }

    /// The constructor's owner, validated class-like (Section 4/26/27): the
    /// entered owner of `symbol`, which must be a `Class`, `Trait` or
    /// `ModuleClass`. If that owner already holds `SymbolInfo::Complete`, its
    /// info is checked to actually be the `ClassInfo` class completion
    /// publishes (naming the owner itself, with the canonical prefix); a
    /// `Missing`, `Deferred` or `Error` owner is not inspected further; its
    /// class-like `SymbolId` is enough.
    fn constructor_owner(&self, at: u32, symbol: SymbolId) -> Result<SymbolId, UnpickleError> {
        let owner = self.store.symbols.get(symbol).owner;
        let class_like = owner.filter(|owner| {
            matches!(
                self.store.symbols.get(*owner).kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            )
        });
        let Some(owner) = class_like else {
            return Err(UnpickleError::ConstructorOwnerNotClassLike { address: at, owner });
        };
        if let SymbolInfo::Complete(info_ty) = self.store.symbols.get(owner).info {
            let malformed = |info_ty| UnpickleError::MalformedOwnerClassInfo {
                address: at,
                owner,
                info: info_ty,
            };
            match self.store.types.get(info_ty) {
                Type::ClassInfo(info)
                    if info.class == owner && info.prefix == self.definitions.no_prefix => {}
                _ => return Err(malformed(info_ty)),
            }
        }
        Ok(owner)
    }

    /// `NamerOps.normalizeIfConstructor`, over the grouped clauses of a
    /// constructor whose parameters are already completed (flags are read
    /// from the entered parameter symbols, never guessed). See the module
    /// doc for the exact rule.
    pub(crate) fn normalize_if_constructor(
        &self,
        clauses: &[Clause],
    ) -> Result<Vec<Clause>, UnpickleError> {
        match clauses.split_first() {
            Some((Clause::Type(params), rest)) => {
                let mut out = vec![Clause::Type(params.clone())];
                out.extend(self.normalize_if_constructor(rest)?);
                Ok(out)
            }
            Some((Clause::Term(params), _))
                if self.clause_kind(params)? == MethodKind::Implicit =>
            {
                let mut out = vec![Clause::EmptyTerm];
                out.extend(clauses.iter().cloned());
                Ok(out)
            }
            _ => {
                let mut all_contextual = true;
                for clause in clauses {
                    match clause {
                        Clause::Term(params) => {
                            if self.clause_kind(params)? != MethodKind::Contextual {
                                all_contextual = false;
                                break;
                            }
                        }
                        // An explicit empty clause already satisfies the
                        // "at least one non-implicit, non-contextual clause"
                        // invariant, so it disqualifies an append exactly as
                        // an ordinary clause would.
                        Clause::EmptyTerm => {
                            all_contextual = false;
                            break;
                        }
                        // A type clause never disqualifies (it is not a term
                        // clause), matching upstream's `forall` over term
                        // clauses only.
                        Clause::Type(_) => {}
                    }
                }
                let mut out = clauses.to_vec();
                if all_contextual {
                    out.push(Clause::EmptyTerm);
                }
                Ok(out)
            }
        }
    }

    /// The result the normalized clauses wrap, before abstraction: the owner
    /// class applied to its own leading type clause's parameter symbols, or
    /// the owner's plain reference if `normalized` has no leading type
    /// clause. Built with the canonical `no_prefix`, never a copied
    /// `ClassInfo.prefix`, a fresh `ThisType`, or a textual lookup.
    fn constructor_effective_result(
        &mut self,
        owner: SymbolId,
        normalized: &[Clause],
    ) -> Result<TypeId, UnpickleError> {
        let owner_ref = self
            .store
            .types
            .alloc(Type::type_ref(self.definitions.no_prefix, owner));
        match normalized.first() {
            Some(Clause::Type(params)) => {
                let mut args = Vec::with_capacity(params.len());
                for param in params {
                    let (symbol, _) = self.parameter_symbol(*param)?;
                    args.push(
                        self.store
                            .types
                            .alloc(Type::type_ref(self.definitions.no_prefix, symbol)),
                    );
                }
                Ok(self.store.types.alloc(Type::Applied {
                    tycon: owner_ref,
                    args,
                }))
            }
            _ => Ok(owner_ref),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Synthetic-wire regressions for the two owner-validation errors
    //! (`constructor_owner`), which real Scala 3.9.0 output cannot produce (a
    //! `<init>` is always owned by a class-like symbol, and class completion
    //! always publishes a well-formed `ClassInfo`): these need direct access
    //! to the unpickler's private state, so they live here rather than in
    //! `tests/constructors.rs`.

    use dotty_core::Definitions;
    use dotty_core::store::SemanticStore;
    use dotty_core::symbols::{Scope, SymbolInfo, SymbolKind};
    use dotty_core::types::{ClassInfo, Type};
    use dotty_tasty::tasty::{
        DEFDEF_TAG, Header, IDENTTPT_TAG, NameTable, PACKAGE_TAG, RawName, Section, SectionTable,
        TEMPLATE_TAG, TERMREFPKG_TAG, TYPEDEF_TAG, TYPEREFPKG_TAG, TastyFile,
    };

    use crate::error::UnpickleError;
    use crate::unpickler::TastyUnpickler;

    const NAMES: [&str; 4] = ["ASTs", "p", "C", "<init>"];

    fn n(text: &str) -> u32 {
        u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
    }

    fn nat(value: u32) -> Vec<u8> {
        let mut groups = vec![u8::try_from(value & 0x7f).unwrap() | 0x80];
        let mut rest = value >> 7;
        while rest > 0 {
            groups.push(u8::try_from(rest & 0x7f).unwrap());
            rest >>= 7;
        }
        groups.reverse();
        groups
    }

    fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend(nat(u32::try_from(payload.len()).unwrap()));
        bytes.extend(payload);
        bytes
    }

    fn leaf(tag: u8, value: u32) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend(nat(value));
        bytes
    }

    /// The type every constructor's result tree names here: the package `p`.
    fn p_type() -> Vec<u8> {
        leaf(TYPEREFPKG_TAG, n("p"))
    }

    /// `IDENTtpt p`: a type tree whose type is `p`, used as a always-resolves
    /// placeholder return tree.
    fn ident() -> Vec<u8> {
        [&[IDENTTPT_TAG][..], &nat(n("p")), &p_type()].concat()
    }

    /// `DEFDEF <init>` with no parameters, no rhs, returning `ident()`.
    fn no_arg_constructor() -> Vec<u8> {
        node(DEFDEF_TAG, &[nat(n("<init>")), ident()].concat())
    }

    fn names() -> NameTable {
        NameTable::from_entries(
            NAMES
                .iter()
                .map(|text| RawName::Utf8((*text).to_owned()))
                .collect(),
        )
        .unwrap()
    }

    fn file_from(ast: &[u8]) -> Vec<u8> {
        TastyFile::from_parts(
            Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names(),
            SectionTable::from_sections(vec![Section::new(0, ast)]),
        )
        .unwrap()
        .encode()
        .unwrap()
    }

    #[test]
    fn a_constructor_directly_under_a_package_has_a_non_class_like_owner() {
        // `<init>` is classified by name alone (`def_def_kind`), so a bare
        // DEFDEF entered as a direct package member (never inside a
        // TEMPLATE) is still a Constructor, owned by the package.
        let ast = node(
            PACKAGE_TAG,
            &[leaf(TERMREFPKG_TAG, n("p")), no_arg_constructor()].concat(),
        );
        let bytes = file_from(&ast);
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler.enter_symbols().unwrap();
        let at = u32::try_from(
            file.ast_address_index()
                .unwrap()
                .iter_nodes()
                .find(|found| found.tag == DEFDEF_TAG)
                .unwrap()
                .offset,
        )
        .unwrap();
        let owner = unpickler.index().symbol_at(0); // the package symbol.

        let result = unpickler.complete_symbol(at);

        assert_eq!(
            result,
            Err(UnpickleError::ConstructorOwnerNotClassLike { address: at, owner })
        );
    }

    #[test]
    fn a_constructor_whose_owner_is_complete_but_not_its_own_class_info_is_malformed() {
        let ast = node(
            PACKAGE_TAG,
            &[
                leaf(TERMREFPKG_TAG, n("p")),
                node(
                    TYPEDEF_TAG,
                    &[nat(n("C")), node(TEMPLATE_TAG, &no_arg_constructor())].concat(),
                ),
            ]
            .concat(),
        );
        let bytes = file_from(&ast);
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler.enter_symbols().unwrap();
        let index = file.ast_address_index().unwrap();
        let class_at = u32::try_from(
            index
                .iter_nodes()
                .find(|found| found.tag == TYPEDEF_TAG)
                .unwrap()
                .offset,
        )
        .unwrap();
        let class = unpickler.index().symbol_at(class_at).expect("the class C");
        assert_eq!(
            unpickler.symbol_state_at(class_at).map(|(kind, _)| kind),
            Some(SymbolKind::Class)
        );

        // Corrupt the class's info directly: `Complete`, but not the
        // `ClassInfo` class completion would ever publish for it (a
        // `ClassInfo` naming a *different* symbol). Real class completion
        // never produces this; it is a defensive check.
        let other_class = unpickler
            .store
            .symbols
            .alloc(unpickler.store.symbols.get(class).clone());
        let scope = unpickler.store.scopes.alloc(Scope::new(Some(other_class)));
        let bogus = unpickler.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: unpickler.definitions.no_prefix,
            class: other_class,
            parents: Vec::new(),
            declarations: scope,
            self_type: None,
        }));
        unpickler.set_symbol_info(class, SymbolInfo::Complete(bogus));

        let ctor_at = {
            // The constructor is the TEMPLATE's only child, right after the
            // TYPEDEF's name and the TEMPLATE's own length header.
            let file_index = file.ast_address_index().unwrap();
            file_index
                .iter_nodes()
                .find(|found| found.tag == DEFDEF_TAG)
                .map(|found| u32::try_from(found.offset).unwrap())
                .unwrap()
        };

        let result = unpickler.complete_symbol(ctor_at);

        assert_eq!(
            result,
            Err(UnpickleError::MalformedOwnerClassInfo {
                address: ctor_at,
                owner: class,
                info: bogus,
            })
        );
        // The corruption survives (complete_symbol only rolls back what it
        // itself touched, and it never reached the point of touching the
        // constructor).
        assert_eq!(
            unpickler.symbol_state_at(ctor_at),
            Some((SymbolKind::Constructor, SymbolInfo::Missing))
        );
    }
}
