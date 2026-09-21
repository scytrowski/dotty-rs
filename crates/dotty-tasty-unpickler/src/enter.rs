//! Pass 1 for definitions: entering symbols for packages' members, classes,
//! their members and parameters.
//!
//! The traversal follows the AST address index rather than nested payloads:
//! only the index reports *absolute* addresses (a node decoded out of a
//! parent's payload knows only its offset inside that payload), and address
//! is the identity of a definition. Owners are known top-down, so a symbol is
//! always allocated after its owner and no forward reference is involved.
//!
//! Every definition gets a symbol. Only packages and classes get a
//! declaration scope; a definition is entered into its owner's scope when
//! the owner has one and the definition is a member of it. Parameters of a
//! method, and the parameters of a class's constructor node, are owned by
//! that method but are not members of any scope. Definitions inside method
//! bodies (locals) are not entered.
//!
//! ## Type parameters of a `LAMBDAtpt` (Milestone 5c)
//!
//! A type tree `[X <: B] =>> body` declares type parameters that exist before
//! any type is projected, so pass 1 enters them too: while a definition is
//! entered, its *declared type trees* (a `VALDEF`'s or parameter's type, a
//! `TYPEPARAM`'s bounds, a type alias's right-hand side, a `DEFDEF`'s result
//! type) are scanned for `LAMBDAtpt`. Only tree forms that can hold one are
//! walked (`APPLIEDtpt`, `BYNAMEtpt`, `EXPLICITtpt`, `TYPEBOUNDStpt`,
//! `ANNOTATEDtpt`'s base, a `SHAREDterm` link and nested `LAMBDAtpt`); method
//! bodies and other terms never are. Each parameter is owned by the
//! *enclosing definition or parameter symbol* (the lambda is not a symbol),
//! is not a member of any scope, and all the immediate parameters of a lambda
//! are entered before the walk continues into their bounds and the body. A
//! lambda reached through a `SHAREDterm` from a second owner keeps its first
//! owner; the conflict is recorded (`has_lambda_owner_conflict`) and refused
//! when the tree is projected.
//!
//! ## Lambdas in parents and self types (Milestone 5d1)
//!
//! A class's template parents and its `SELFDEF` type tree are scanned too, with
//! the class as the owner. A parent is scanned only along the paths
//! `type_of_parent` reads (a call's function, a type application's function
//! and type arguments, a constructor selection's `NEW` type), never through a
//! constructor argument. Upstream reads parents in a compiler-internal dummy
//! context; that owner is not a declaration, so the parameters are owned by the
//! class, and, being entered as non-members, never reach its declaration scope.

use dotty_core::ids::SymbolId;
use dotty_core::names::Name;
use dotty_core::symbols::{Scope, Symbol, SymbolInfo, SymbolKind, SymbolLinks, Visibility};
use dotty_tasty::tasty::{
    ANNOTATEDTPT_TAG, APPLIEDTPT_TAG, APPLY_TAG, BLOCK_TAG, BYNAMETPT_TAG, DEFDEF_TAG,
    DefinitionBody, EMPTYCLAUSE_TAG, EXPLICITTPT_TAG, LAMBDATPT_TAG, NEW_TAG, PACKAGE_TAG,
    PARAM_TAG, ParameterNode, RawTree, SELECTIN_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG,
    SPLITCLAUSE_TAG, StructuredNode, TEMPLATE_TAG, TERMREFPKG_TAG, TYPEAPPLY_TAG,
    TYPEBOUNDSTPT_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TermValue,
    VALDEF_TAG,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::class::template_parts;
use crate::error::UnpickleError;
use crate::mapping::{
    DeclaredModifiers, QualifiedAccess, QualifierRef, def_def_kind, namespace_of, term_param_kind,
    type_def_kind, type_param_kind, val_def_kind,
};
use crate::names::{package_segments, wire_name};
use crate::packages::enter_in_scope;
use crate::unpickler::TastyUnpickler;

/// How deep a type tree may nest before pass 1 refuses it.
const MAX_TREE_DEPTH: usize = 256;

/// What is known about a definition when its symbol is allocated.
struct Declaration<'a> {
    at: u32,
    tag: u8,
    name_ref: u32,
    kind: SymbolKind,
    modifiers: &'a DeclaredModifiers,
    owner: SymbolId,
    /// Whether the symbol is a member of its owner's declaration scope.
    member: bool,
}

/// The name reference of a `PACKAGE` node's path, which is a direct
/// `TERMREFpkg` or a `SHAREDtype` link to one.
///
/// The compiler writes a repeated subtree once, so a nested package whose
/// path has already been written elsewhere (for example inside an import)
/// refers to it by address. The link is followed with [`AstView::tree_at`],
/// which rejects an address that is not the start of a node, and a chain
/// longer than [`MAX_SHARED_DEPTH`] is treated as a cycle. Any other path form
/// is `UnsupportedPackagePath`.
fn package_path_name(ast: &AstView<'_>, path: &RawTree<'_>, at: u32) -> Result<u32, UnpickleError> {
    let unsupported = UnpickleError::UnsupportedPackagePath { address: at };
    let RawTree::Leaf(term) = path else {
        return Err(unsupported);
    };
    let mut target = match (term.tag, &term.value) {
        (TERMREFPKG_TAG, TermValue::NameRef(name)) => return Ok(*name),
        (SHAREDTYPE_TAG, TermValue::AstRef(target)) => *target,
        _ => return Err(unsupported),
    };

    for _ in 0..=MAX_SHARED_DEPTH {
        let RawTree::Leaf(term) = ast.tree_at(target, at)? else {
            return Err(unsupported);
        };
        match (term.tag, term.value) {
            (TERMREFPKG_TAG, TermValue::NameRef(name)) => return Ok(name),
            (SHAREDTYPE_TAG, TermValue::AstRef(next)) => target = next,
            _ => return Err(unsupported),
        }
    }
    Err(UnpickleError::InvalidReferenceTarget {
        from: at,
        to: target,
    })
}

impl TastyUnpickler<'_, '_, '_> {
    /// Enters a `PACKAGE` node: its package symbol, then its members.
    pub(crate) fn enter_package(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
    ) -> Result<(), UnpickleError> {
        let package = ast.node(at)?.decode_package()?;
        let path_name = package_path_name(ast, &package.path, at)?;
        // A unit in the default package has the empty path: the session root.
        let path = package_segments(self.file.names(), path_name)?;
        let chain = self.packages.enter(self.store, self.origin, &path);
        for package in &chain {
            self.index.share_scope(package.symbol, package.scope);
        }
        let symbol = chain
            .last()
            .map(|package| package.symbol)
            .unwrap_or_else(|| unreachable!("non-empty path"));
        self.index.insert_symbol(at, symbol)?;

        for child in ast.children(at) {
            let child_at = address(child.offset);
            match child.tag {
                PACKAGE_TAG => self.enter_package(ast, child_at)?,
                TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG => {
                    self.enter_definition(ast, child_at, child.tag, symbol)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Enters a `TYPEDEF`, `VALDEF` or `DEFDEF` owned by `owner`, and the
    /// definitions nested in it.
    fn enter_definition(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        tag: u8,
        owner: SymbolId,
    ) -> Result<(), UnpickleError> {
        let node = ast.node(at)?;
        let owner_kind = self.store.symbols.get(owner).kind;

        match node.decode_structured()? {
            StructuredNode::TypeDef(DefinitionBody::TypeDef {
                name,
                type_or_template,
                tail,
            }) => {
                let modifiers = DeclaredModifiers::from_tail(&tail)?;
                let has_template = matches!(
                    &type_or_template,
                    RawTree::LengthNode(template) if template.tag == TEMPLATE_TAG
                );
                let kind = type_def_kind(&modifiers, has_template);
                let symbol = self.enter_symbol(
                    ast,
                    Declaration {
                        at,
                        tag,
                        name_ref: name,
                        kind,
                        modifiers: &modifiers,
                        owner,
                        member: true,
                    },
                )?;
                if has_template {
                    self.enter_class_scope(symbol)?;
                    self.enter_template(ast, at, symbol)?;
                } else {
                    self.enter_declared_lambdas(ast, at, symbol)?;
                }
            }
            StructuredNode::ValDef(DefinitionBody::ValDef { name, tail, .. }) => {
                let modifiers = DeclaredModifiers::from_tail(&tail)?;
                let kind = val_def_kind(&modifiers, owner_kind);
                let symbol = self.enter_symbol(
                    ast,
                    Declaration {
                        at,
                        tag,
                        name_ref: name,
                        kind,
                        modifiers: &modifiers,
                        owner,
                        member: true,
                    },
                )?;
                self.enter_declared_lambdas(ast, at, symbol)?;
            }
            StructuredNode::DefDef(body) => {
                let modifiers = DeclaredModifiers::from_tail(&body.tail)?;
                let kind = def_def_kind(&wire_name(self.file.names(), body.name)?);
                let method = self.enter_symbol(
                    ast,
                    Declaration {
                        at,
                        tag,
                        name_ref: body.name,
                        kind,
                        modifiers: &modifiers,
                        owner,
                        member: true,
                    },
                )?;
                self.enter_parameters(ast, at, method, false)?;
                // The result type is the first child that is not a parameter
                // or a clause marker.
                let result = ast.children(at).iter().find(|child| {
                    !matches!(
                        child.tag,
                        TYPEPARAM_TAG | PARAM_TAG | EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG
                    )
                });
                if let Some(result) = result {
                    self.enter_lambdas_in(ast, address(result.offset), method, 0)?;
                }
            }
            _ => return Err(UnpickleError::MissingDefinition { address: at }),
        }
        Ok(())
    }

    /// Enters the members of the class whose node is at `class_at`: the
    /// class's own type parameters and constructor parameters (its template
    /// header), then its statements.
    fn enter_template(
        &mut self,
        ast: &AstView<'_>,
        class_at: u32,
        class: SymbolId,
    ) -> Result<(), UnpickleError> {
        let Some(template_node) = ast
            .children(class_at)
            .iter()
            .find(|child| child.tag == TEMPLATE_TAG)
        else {
            return Err(UnpickleError::MissingDefinition { address: class_at });
        };
        let template_at = address(template_node.offset);
        self.enter_parameters(ast, template_at, class, true)?;
        for child in ast.children(template_at) {
            if matches!(child.tag, TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG) {
                self.enter_definition(ast, address(child.offset), child.tag, class)?;
            }
            // Parents, the self definition and non-definition statements
            // declare no symbol of their own; a lambda type inside a parent
            // or the self type does (below).
        }
        let parts = template_parts(ast, template_at);
        for parent in parts.parents {
            self.enter_parent_lambdas(ast, parent, class, 0)?;
        }
        if let Some(self_def) = parts.self_def
            && let Some(tree) = ast.children(self_def).first()
        {
            self.enter_lambdas_in(ast, address(tree.offset), class, 0)?;
        }
        Ok(())
    }

    /// Enters the `LAMBDAtpt` parameters in the parent tree at `at`, owned by
    /// `class`, following exactly the child paths
    /// [`type_of_parent`](TastyUnpickler::type_of_parent) reads: a call's
    /// function only (never its arguments or statements), a type application's
    /// function and type arguments, a constructor selection's `NEW` type.
    ///
    /// Upstream reads parents in a compiler-internal `localDummy` context; that
    /// owner is not a semantic declaration, so a parent's lambda parameters are
    /// owned by the class. They are entered as non-members, so they never reach
    /// the class scope.
    fn enter_parent_lambdas(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        class: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        if depth > MAX_TREE_DEPTH {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "a parent tree nests too deeply",
            });
        }
        // A link that cannot be followed is reported when the parent is
        // projected, not when symbols are entered.
        let Ok(at) = ast.resolve_shared_term(at, at) else {
            return Ok(());
        };
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        match ast.tag_at(at) {
            Some(APPLY_TAG | BLOCK_TAG) => match children.first() {
                Some(function) => self.enter_parent_lambdas(ast, *function, class, depth + 1),
                None => Ok(()),
            },
            Some(TYPEAPPLY_TAG) => {
                let Some((function, arguments)) = children.split_first() else {
                    return Ok(());
                };
                self.enter_parent_lambdas(ast, *function, class, depth + 1)?;
                for argument in arguments {
                    self.enter_lambdas_in(ast, *argument, class, depth + 1)?;
                }
                Ok(())
            }
            Some(SELECTIN_TAG) => match children.first() {
                Some(new) if ast.tag_at(*new) == Some(NEW_TAG) => {
                    for tpt in ast.children(*new) {
                        self.enter_lambdas_in(ast, address(tpt.offset), class, depth + 1)?;
                    }
                    Ok(())
                }
                _ => Ok(()),
            },
            _ => self.enter_lambdas_in(ast, at, class, depth + 1),
        }
    }

    /// Enters the `TYPEPARAM`/`PARAM` nodes directly under `parent_at`, in
    /// wire order, decoding each from its own index node.
    fn enter_parameters(
        &mut self,
        ast: &AstView<'_>,
        parent_at: u32,
        owner: SymbolId,
        in_class: bool,
    ) -> Result<(), UnpickleError> {
        for child in ast.children(parent_at) {
            if !matches!(child.tag, TYPEPARAM_TAG | PARAM_TAG) {
                continue;
            }
            let at = address(child.offset);
            let parameter = ast.node(at)?.decode_parameter()?;
            self.enter_parameter(ast, at, &parameter, owner, in_class)?;
        }
        Ok(())
    }

    /// Enters one parameter. In a class header (`in_class`) a parameter that
    /// is a member is also declared in the class scope.
    fn enter_parameter(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        parameter: &ParameterNode<'_>,
        owner: SymbolId,
        in_class: bool,
    ) -> Result<(), UnpickleError> {
        let tag = parameter.tag();
        let modifiers = DeclaredModifiers::from_tail(&parameter.decode_body()?.tail)?;
        let kind = if tag == TYPEPARAM_TAG {
            type_param_kind()
        } else {
            term_param_kind(&modifiers, in_class)
        };
        let is_member = in_class && matches!(kind, SymbolKind::TypeParameter | SymbolKind::Field);
        let symbol = self.enter_symbol(
            ast,
            Declaration {
                at,
                tag,
                name_ref: parameter.name(),
                kind,
                modifiers: &modifiers,
                owner,
                member: is_member,
            },
        )?;
        self.enter_declared_lambdas(ast, at, symbol)?;
        Ok(())
    }

    /// Scans the declared type tree of the definition at `at` (its first
    /// child: a type, bounds or right-hand side) for `LAMBDAtpt` parameters
    /// owned by `owner`.
    fn enter_declared_lambdas(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        owner: SymbolId,
    ) -> Result<(), UnpickleError> {
        if let Some(tree) = ast.children(at).first() {
            self.enter_lambdas_in(ast, address(tree.offset), owner, 0)?;
        }
        Ok(())
    }

    /// Walks the type tree at `tree`, entering the type parameters of every
    /// `LAMBDAtpt` in it for `owner`. `depth` bounds the nesting, links and
    /// lambdas alike.
    fn enter_lambdas_in(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        if depth > MAX_TREE_DEPTH {
            return Err(UnpickleError::MalformedType {
                address: tree,
                reason: "a type tree nests too deeply",
            });
        }
        let Some(tag) = ast.tag_at(tree) else {
            return Ok(());
        };
        if !matches!(
            tag,
            SHAREDTERM_TAG
                | LAMBDATPT_TAG
                | APPLIEDTPT_TAG
                | BYNAMETPT_TAG
                | EXPLICITTPT_TAG
                | TYPEBOUNDSTPT_TAG
                | ANNOTATEDTPT_TAG
        ) || !self.index.first_lambda_scan(tree, owner)
        {
            return Ok(());
        }
        let children: Vec<u32> = ast
            .children(tree)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        match tag {
            SHAREDTERM_TAG => {
                // A link to a tree written earlier: walked as if it were here.
                // A chain of links is bounded like every shared chain.
                let target = ast.resolve_shared_term(tree, tree)?;
                self.enter_lambdas_in(ast, target, owner, depth + 1)
            }
            LAMBDATPT_TAG => {
                match self.index.lambda_owner(tree) {
                    Some(first) if first == owner => return Ok(()),
                    Some(_) => {
                        self.index.mark_lambda_conflict(tree);
                        return Ok(());
                    }
                    None => self.index.insert_lambda_owner(tree, owner),
                }
                ast.node(tree)?.decode_lambda_tpt()?;
                let Some((body, params)) = children.split_last() else {
                    return Err(UnpickleError::MalformedType {
                        address: tree,
                        reason: "a lambda type tree has type parameters and a body",
                    });
                };
                // Every parameter is entered before any bounds or the body
                // are walked, so they can refer to one another.
                for param in params {
                    if ast.tag_at(*param) != Some(TYPEPARAM_TAG) {
                        return Err(UnpickleError::MalformedType {
                            address: tree,
                            reason: "a lambda type tree parameter is not a type parameter",
                        });
                    }
                    let parameter = ast.node(*param)?.decode_parameter()?;
                    self.enter_parameter(ast, *param, &parameter, owner, false)?;
                }
                self.enter_lambdas_in(ast, *body, owner, depth + 1)
            }
            // A type tree with the annotation to be ignored: only the base.
            ANNOTATEDTPT_TAG => match children.first() {
                Some(base) => self.enter_lambdas_in(ast, *base, owner, depth + 1),
                None => Ok(()),
            },
            _ => {
                for child in children {
                    self.enter_lambdas_in(ast, child, owner, depth + 1)?;
                }
                Ok(())
            }
        }
    }

    /// Allocates the symbol for a definition, records its address, and
    /// declares it in its owner's scope when it is a member and the owner has
    /// one.
    fn enter_symbol(
        &mut self,
        ast: &AstView<'_>,
        declaration: Declaration<'_>,
    ) -> Result<SymbolId, UnpickleError> {
        let Declaration {
            at,
            tag,
            name_ref,
            kind,
            modifiers,
            owner,
            member,
        } = declaration;
        let namespace =
            namespace_of(tag).ok_or(UnpickleError::MissingDefinition { address: at })?;
        let text = wire_name(self.file.names(), name_ref)?;
        let name = Name::new(self.store.names.intern(&text), namespace);

        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(owner),
            kind,
            flags: modifiers.flags,
            visibility: modifiers.visibility,
            info: SymbolInfo::Missing,
            origin: self.origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.index.insert_symbol(at, symbol)?;

        if member && let Some(scope) = self.index.scope_of(owner) {
            enter_in_scope(self.store, &mut self.scope_journal, scope, name, symbol);
        }
        if let Some(access) = modifiers.qualified {
            self.apply_qualified_access(ast, at, symbol, access)?;
        }
        Ok(symbol)
    }

    /// Sets the visibility of a `private[Q]` / `protected[Q]` definition.
    ///
    /// This runs after the symbol is allocated and indexed, because the
    /// qualifier may be the definition itself, and an enclosing definition
    /// is always entered before its members.
    fn apply_qualified_access(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        symbol: SymbolId,
        access: QualifiedAccess,
    ) -> Result<(), UnpickleError> {
        let qualifier = self.resolve_qualifier(ast, at, symbol, access.qualifier, 0)?;
        self.store.symbols.get_mut(symbol).visibility = if access.protected {
            Visibility::ProtectedWithin(qualifier)
        } else {
            Visibility::PrivateWithin(qualifier)
        };
        Ok(())
    }

    /// Resolves the qualifier of the definition `symbol` (at address `at`).
    ///
    /// The qualifier is a package (by name) or an entered definition (by
    /// address), reached through any `SHAREDtype` links. Either way it must be
    /// a package, class, trait or object that encloses `symbol`, or is
    /// `symbol` itself; anything else is `InvalidQualifier`. A package is
    /// found among the owners of `symbol`, never entered, so an untrusted name
    /// cannot create a package symbol.
    pub(crate) fn resolve_qualifier(
        &self,
        ast: &AstView<'_>,
        at: u32,
        symbol: SymbolId,
        qualifier: QualifierRef,
        depth: usize,
    ) -> Result<SymbolId, UnpickleError> {
        let invalid = UnpickleError::InvalidQualifier { definition: at };
        // The definition itself, then each enclosing owner, innermost first.
        let mut chain = vec![symbol];
        while let Some(owner) = self
            .store
            .symbols
            .get(*chain.last().unwrap_or(&symbol))
            .owner
        {
            chain.push(owner);
        }

        match qualifier {
            QualifierRef::Package(name) => {
                let path = package_segments(self.file.names(), name)?;
                chain
                    .into_iter()
                    .find(|candidate| {
                        self.store.symbols.get(*candidate).kind == SymbolKind::Package
                            && self.package_path(*candidate) == path
                    })
                    .ok_or(invalid)
            }
            QualifierRef::Symbol(definition) => {
                let target = self.index.symbol_at(definition).ok_or(
                    UnpickleError::InvalidReferenceTarget {
                        from: at,
                        to: definition,
                    },
                )?;
                let eligible = matches!(
                    self.store.symbols.get(target).kind,
                    SymbolKind::Package
                        | SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::Object
                        | SymbolKind::ModuleClass
                );
                if eligible && chain.contains(&target) {
                    Ok(target)
                } else {
                    Err(invalid)
                }
            }
            QualifierRef::Shared(target) => {
                // A chain of shared links is at most a few deep; a longer one
                // is a cycle.
                if depth > MAX_SHARED_DEPTH {
                    return Err(UnpickleError::InvalidReferenceTarget {
                        from: at,
                        to: target,
                    });
                }
                let next = match ast.tree_at(target, at)? {
                    RawTree::Leaf(term) => match (term.tag, term.value) {
                        (TYPEREFPKG_TAG, TermValue::NameRef(name)) => QualifierRef::Package(name),
                        (SHAREDTYPE_TAG, TermValue::AstRef(next)) => QualifierRef::Shared(next),
                        (tag, _) => return Err(UnpickleError::UnsupportedQualifier { tag }),
                    },
                    RawTree::NatAst {
                        tag: TYPEREFSYMBOL_TAG,
                        value: definition,
                        ..
                    } => QualifierRef::Symbol(definition),
                    RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => {
                        return Err(UnpickleError::UnsupportedQualifier { tag });
                    }
                    RawTree::LengthNode(node) => {
                        return Err(UnpickleError::UnsupportedQualifier { tag: node.tag });
                    }
                };
                self.resolve_qualifier(ast, at, symbol, next, depth + 1)
            }
        }
    }

    /// The path of a package symbol, outermost segment first.
    fn package_path(&self, package: SymbolId) -> Vec<String> {
        let mut path = Vec::new();
        let mut current = Some(package);
        while let Some(symbol) = current {
            let symbol = self.store.symbols.get(symbol);
            // The root package, which has no owner, contributes no segment.
            if symbol.kind != SymbolKind::Package || symbol.owner.is_none() {
                break;
            }
            path.push(self.store.names.resolve(symbol.name.text()).to_owned());
            current = symbol.owner;
        }
        path.reverse();
        path
    }

    fn enter_class_scope(&mut self, class: SymbolId) -> Result<(), UnpickleError> {
        let scope = self.store.scopes.alloc(Scope::new(Some(class)));
        self.index.insert_scope(class, scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::Definitions;
    use dotty_core::store::SemanticStore;

    const FOO: &[u8] = include_bytes!("../tests/fixtures/semantic/Foo.tasty");
    const OUTER: &[u8] = include_bytes!("../tests/fixtures/semantic/Outer.tasty");

    // Foo.tasty (absolute addresses, pinned by `tests/semantic_fixture.rs`):
    // 4 = class Foo, 24 = its parameter `x` (a member field), 70 = method
    // `bar`, 27 = a type node whose payload starts at 28 (not a node start).
    const FOO_CLASS: u32 = 4;
    const FOO_X: u32 = 24;
    const FOO_BAR: u32 = 70;
    const INSIDE_A_NODE: u32 = 28;

    /// Runs `check` on an unpickler that has entered the file's symbols.
    fn with_entered<R>(
        bytes: &[u8],
        check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, &AstView<'_>) -> R,
    ) -> R {
        let file = dotty_tasty::tasty::TastyFile::parse_scala_3_9(bytes).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler.enter_symbols().unwrap();
        let ast = AstView::new(&file).expect("the AST view builds");
        check(&mut unpickler, &ast)
    }

    fn resolve(
        unpickler: &TastyUnpickler<'_, '_, '_>,
        ast: &AstView<'_>,
        definition: u32,
        qualifier: QualifierRef,
    ) -> Result<SymbolId, UnpickleError> {
        let symbol = unpickler.index.symbol_at(definition).expect("definition");
        unpickler.resolve_qualifier(ast, definition, symbol, qualifier, 0)
    }

    /// The address of the definition with tag `tag` named `text` in the file.
    fn definition_named(bytes: &[u8], tag: u8, text: &str) -> u32 {
        let file = dotty_tasty::tasty::TastyFile::parse_scala_3_9(bytes).unwrap();
        let index = file.ast_address_index().unwrap();
        index
            .iter()
            .filter(|node| node.tag == tag)
            .find(|node| {
                let name = node.decode_definition().unwrap().name();
                wire_name(file.names(), name).is_ok_and(|found| found == text)
            })
            .map(|node| u32::try_from(node.offset).unwrap())
            .unwrap_or_else(|| panic!("no definition named {text}"))
    }

    // --- shared-reference targets are validated before decoding ---

    #[test]
    fn a_shared_qualifier_pointing_into_the_middle_of_a_node_is_an_invalid_target() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(
                unpickler,
                ast,
                FOO_CLASS,
                QualifierRef::Shared(INSIDE_A_NODE),
            )
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidReferenceTarget {
                from: FOO_CLASS,
                to: INSIDE_A_NODE
            })
        );
    }

    #[test]
    fn a_shared_qualifier_pointing_past_the_ast_section_is_an_invalid_target() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_CLASS, QualifierRef::Shared(1_000_000))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidReferenceTarget {
                from: FOO_CLASS,
                to: 1_000_000
            })
        );
    }

    #[test]
    fn a_shared_qualifier_at_the_largest_address_is_an_invalid_target_not_an_overflow() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_CLASS, QualifierRef::Shared(u32::MAX))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidReferenceTarget {
                from: FOO_CLASS,
                to: u32::MAX
            })
        );
    }

    // --- a symbol qualifier must be an enclosing class, trait, object or package ---

    #[test]
    fn a_symbol_qualifier_may_be_the_enclosing_class() {
        let (resolved, class) = with_entered(FOO, |unpickler, ast| {
            (
                resolve(unpickler, ast, FOO_X, QualifierRef::Symbol(FOO_CLASS)),
                unpickler.index.symbol_at(FOO_CLASS).unwrap(),
            )
        });

        assert_eq!(resolved, Ok(class));
    }

    #[test]
    fn a_symbol_qualifier_may_be_the_definition_itself() {
        let (resolved, class) = with_entered(FOO, |unpickler, ast| {
            (
                resolve(unpickler, ast, FOO_CLASS, QualifierRef::Symbol(FOO_CLASS)),
                unpickler.index.symbol_at(FOO_CLASS).unwrap(),
            )
        });

        assert_eq!(resolved, Ok(class));
    }

    #[test]
    fn a_symbol_qualifier_that_is_a_method_is_rejected() {
        // `bar` is a definition with a symbol, but not a possible qualifier.
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_X, QualifierRef::Symbol(FOO_BAR))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidQualifier { definition: FOO_X })
        );
    }

    #[test]
    fn a_symbol_qualifier_that_is_the_definition_itself_but_not_class_like_is_rejected() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_BAR, QualifierRef::Symbol(FOO_BAR))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidQualifier {
                definition: FOO_BAR
            })
        );
    }

    #[test]
    fn a_symbol_qualifier_naming_an_address_with_no_symbol_is_an_invalid_target() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_X, QualifierRef::Symbol(INSIDE_A_NODE))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidReferenceTarget {
                from: FOO_X,
                to: INSIDE_A_NODE
            })
        );
    }

    #[test]
    fn a_nested_class_does_not_enclose_the_class_it_is_nested_in() {
        // `class Outer { class Inner ... }`: `Inner` is inside `Outer`, so it
        // cannot qualify `Outer`.
        let (outer, inner) = (
            definition_named(OUTER, TYPEDEF_TAG, "Outer"),
            definition_named(OUTER, TYPEDEF_TAG, "Inner"),
        );

        let result = with_entered(OUTER, |unpickler, ast| {
            resolve(unpickler, ast, outer, QualifierRef::Symbol(inner))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidQualifier { definition: outer })
        );
    }

    #[test]
    fn a_class_two_levels_out_may_qualify_a_nested_definition() {
        // `withinOuter` is in `Inner`, which is in `Outer`.
        let (outer, member) = (
            definition_named(OUTER, TYPEDEF_TAG, "Outer"),
            definition_named(OUTER, DEFDEF_TAG, "withinOuter"),
        );

        let (resolved, class) = with_entered(OUTER, |unpickler, ast| {
            (
                resolve(unpickler, ast, member, QualifierRef::Symbol(outer)),
                unpickler.index.symbol_at(outer).unwrap(),
            )
        });

        assert_eq!(resolved, Ok(class));
    }

    // --- a package qualifier must be an enclosing package, and is never entered ---

    // Name-table indexes in Foo.tasty: 5 = `me.cytrowski.tastyfixtures`,
    // 7 = `me.cytrowski.tastyfixtures.semantic`, 11 = `scala`.
    const OUTER_PACKAGE_NAME: u32 = 5;
    const OWN_PACKAGE_NAME: u32 = 7;
    const SCALA_NAME: u32 = 11;

    #[test]
    fn a_package_qualifier_may_name_the_units_own_package() {
        let (resolved, package) = with_entered(FOO, |unpickler, ast| {
            (
                resolve(
                    unpickler,
                    ast,
                    FOO_CLASS,
                    QualifierRef::Package(OWN_PACKAGE_NAME),
                ),
                unpickler.index.symbol_at(0).unwrap(),
            )
        });

        assert_eq!(resolved, Ok(package));
    }

    #[test]
    fn a_package_qualifier_may_name_an_outer_package() {
        let (resolved, own) = with_entered(FOO, |unpickler, ast| {
            (
                resolve(
                    unpickler,
                    ast,
                    FOO_CLASS,
                    QualifierRef::Package(OUTER_PACKAGE_NAME),
                ),
                unpickler.index.symbol_at(0).unwrap(),
            )
        });

        let outer = resolved.expect("an enclosing package");
        assert_ne!(outer, own);
    }

    #[test]
    fn a_package_that_does_not_enclose_the_definition_is_rejected() {
        let result = with_entered(FOO, |unpickler, ast| {
            resolve(unpickler, ast, FOO_CLASS, QualifierRef::Package(SCALA_NAME))
        });

        assert_eq!(
            result,
            Err(UnpickleError::InvalidQualifier {
                definition: FOO_CLASS
            })
        );
    }
}
