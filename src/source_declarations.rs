//! Source-ordered declarations of ordinary top-level classes after naming.

use crate::core::{
    AstArena, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SymbolFlags,
    SymbolId, SymbolKind, SymbolOrigin, Template, Tree, TreeId, TreeKind, Untyped, Visibility,
};
use std::fmt;

/// A canonical declaration tree and its symbol in the naming session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub symbol: SymbolId,
    pub tree: TreeId<Untyped>,
}

/// An initialized field or concrete method in the class body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassMember {
    Field(Declaration),
    Method(Declaration),
}

/// IDs from a named class, in source order. All IDs refer to the caller's session.
#[derive(Debug)]
pub struct SourceClass {
    pub symbol: SymbolId,
    pub tree: TreeId<Untyped>,
    /// Primary constructor symbol.
    pub constructor: SymbolId,
    /// Class-owned field symbols from primary constructor parameter clauses,
    /// in clause and parameter order. Constructor-owned formals have separate identities.
    pub constructor_fields: Vec<SymbolId>,
    /// Original parent trees, including constructor applications.
    pub parents: Vec<TreeId<Untyped>>,
    /// Body declarations, preserving interleaved field and method order.
    pub members: Vec<ClassMember>,
}

impl SourceClass {
    /// Checks the caller's body view against the original named template before typing.
    pub(crate) fn body_source(
        &self,
        ast: &AstArena<Untyped>,
        index: &SourceSemanticIndex,
        store: &SemanticStore,
    ) -> Option<SourceId> {
        let SourceDefinition::Canonical { source, tree } = index.definition_of(self.symbol)? else {
            return None;
        };
        if tree != self.tree {
            return None;
        }
        let (reader, template) =
            named_class_template(self.symbol, tree, ast, source, index, store)?;
        (template.body.len() == self.members.len()
            && template
                .body
                .iter()
                .zip(&self.members)
                .all(|(&tree, &member)| reader.body_member(tree, self.symbol) == Ok(member)))
        .then_some(source)
    }

    /// Initialized body fields in declaration order.
    pub fn fields(&self) -> impl Iterator<Item = Declaration> + '_ {
        self.members.iter().filter_map(|member| match member {
            ClassMember::Field(field) => Some(*field),
            ClassMember::Method(_) => None,
        })
    }

    /// Concrete source methods in declaration order.
    pub fn methods(&self) -> impl Iterator<Item = Declaration> + '_ {
        self.members.iter().filter_map(|member| match member {
            ClassMember::Method(method) => Some(*method),
            ClassMember::Field(_) => None,
        })
    }
}

/// The first unsupported source construct or inconsistent naming input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationError {
    /// A declaration or statement outside the supported source subset.
    Unsupported {
        source: SourceId,
        tree: TreeId<Untyped>,
    },
    /// A tree, symbol or owner is inconsistent with the naming inputs.
    InvalidBinding {
        source: SourceId,
        tree: TreeId<Untyped>,
    },
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot read source declarations: {self:?}")
    }
}
impl std::error::Error for DeclarationError {}

/// Reads one named package root containing ordinary concrete classes with initialized
/// fields and concrete methods. Other package statements and class-body statements
/// produce an error at the offending tree. Parents are retained as untyped trees.
/// Pass the AST, index and store from the same naming session. The returned IDs
/// can be kept while the typer extends that store. Field and method declarations
/// point to their definition trees, as required by the source typer.
pub fn source_classes(
    ast: &AstArena<Untyped>,
    root: TreeId<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Result<Vec<SourceClass>, DeclarationError> {
    let reader = Reader {
        ast,
        source,
        index,
        store,
    };
    let TreeKind::PackageDef(package) = &reader.node(root)?.kind else {
        return Err(DeclarationError::Unsupported { source, tree: root });
    };
    // Packages can be shared between sources and have no canonical definition tree.
    let owner = index
        .symbol_at(source, root)
        .filter(|&id| {
            store.symbols.contains(id) && store.symbols.get(id).kind == SymbolKind::Package
        })
        .ok_or(DeclarationError::InvalidBinding { source, tree: root })?;
    package
        .stats
        .iter()
        .map(|&tree| reader.class(tree, owner))
        .collect()
}

// The same identity check is used when a JVM planner receives individual inputs.
pub(crate) fn canonical_symbol(
    store: &SemanticStore,
    index: &SourceSemanticIndex,
    source: SourceId,
    tree: TreeId<Untyped>,
    owner: SymbolId,
) -> Option<SymbolId> {
    let id = index.symbol_at(source, tree)?;
    if !store.symbols.contains(id)
        || index.definition_of(id) != Some(SourceDefinition::Canonical { source, tree })
    {
        return None;
    }
    let symbol = store.symbols.get(id);
    (symbol.owner == Some(owner) && symbol.origin == SymbolOrigin::Source(source)).then_some(id)
}

pub(crate) fn named_class_template<'a>(
    class: SymbolId,
    tree: TreeId<Untyped>,
    ast: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    store: &'a SemanticStore,
) -> Option<(Reader<'a>, &'a Template<Untyped>)> {
    if !store.symbols.contains(class) {
        return None;
    }
    let owner = store.symbols.owner(class)?;
    if !store.symbols.contains(owner) || store.symbols.get(owner).kind != SymbolKind::Package {
        return None;
    }
    let reader = Reader {
        ast,
        source,
        index,
        store,
    };
    let (symbol, template) = reader.class_template(tree, owner).ok()?;
    (symbol == class).then_some((reader, template))
}

/// Collects the canonical constructor fields and public body fields in declaration order.
pub(crate) fn signature_fields(
    class: SymbolId,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Option<(SourceId, Vec<SymbolId>)> {
    let SourceDefinition::Canonical { source, tree } = index.definition_of(class)? else {
        return None;
    };
    let (reader, template) = named_class_template(class, tree, ast, source, index, store)?;
    let (_, mut fields) = reader.constructor(class, template.constructor).ok()?;
    for &tree in &template.body {
        if let ClassMember::Field(field) = reader.body_member(tree, class).ok()?
            && store.symbols.get(field.symbol).visibility == Visibility::Public
        {
            fields.push(field.symbol);
        }
    }
    Some((source, fields))
}

pub(crate) struct Reader<'a> {
    ast: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    store: &'a SemanticStore,
}

impl<'a> Reader<'a> {
    fn node(&self, tree: TreeId<Untyped>) -> Result<&'a Tree<Untyped>, DeclarationError> {
        self.ast
            .try_get(tree)
            .filter(|node| node.position.is_none_or(|p| p.source() == self.source))
            .ok_or(DeclarationError::InvalidBinding {
                source: self.source,
                tree,
            })
    }

    fn declaration(
        &self,
        tree: TreeId<Untyped>,
        owner: SymbolId,
    ) -> Result<Declaration, DeclarationError> {
        let source = self.source;
        let symbol = canonical_symbol(self.store, self.index, source, tree, owner)
            .ok_or(DeclarationError::InvalidBinding { source, tree })?;
        Ok(Declaration { symbol, tree })
    }

    fn class_template(
        &self,
        tree: TreeId<Untyped>,
        owner: SymbolId,
    ) -> Result<(SymbolId, &'a Template<Untyped>), DeclarationError> {
        let source = self.source;
        let TreeKind::TypeDef(def) = &self.node(tree)?.kind else {
            return Err(DeclarationError::Unsupported { source, tree });
        };
        let TreeKind::Template(template) = &self.node(def.rhs)?.kind else {
            return Err(DeclarationError::Unsupported { source, tree });
        };
        let class = self.declaration(tree, owner)?.symbol;
        let symbol = self.store.symbols.get(class);
        if symbol.kind != SymbolKind::Class
            || !symbol
                .flags
                .intersection(
                    SymbolFlags::ABSTRACT
                        | SymbolFlags::CASE
                        | SymbolFlags::ENUM
                        | SymbolFlags::GIVEN,
                )
                .is_empty()
        {
            return Err(DeclarationError::Unsupported { source, tree });
        }
        Ok((class, template))
    }

    fn class(
        &self,
        tree: TreeId<Untyped>,
        owner: SymbolId,
    ) -> Result<SourceClass, DeclarationError> {
        let (class, template) = self.class_template(tree, owner)?;
        let (constructor, constructor_fields) = self.constructor(class, template.constructor)?;
        for &parent in &template.parents {
            self.node(parent)?;
        }
        let mut members = Vec::with_capacity(template.body.len());
        for &tree in &template.body {
            members.push(self.body_member(tree, class)?);
        }
        Ok(SourceClass {
            symbol: class,
            tree,
            constructor,
            constructor_fields,
            parents: template.parents.clone(),
            members,
        })
    }

    fn constructor(
        &self,
        class: SymbolId,
        ctor_tree: TreeId<Untyped>,
    ) -> Result<(SymbolId, Vec<SymbolId>), DeclarationError> {
        let source = self.source;
        let ctor_error = DeclarationError::InvalidBinding {
            source,
            tree: ctor_tree,
        };
        let TreeKind::DefDef(ctor) = &self.node(ctor_tree)?.kind else {
            return Err(ctor_error);
        };
        let constructor = self.declaration(ctor_tree, class)?.symbol;
        if self.store.symbols.get(constructor).kind != SymbolKind::Constructor {
            return Err(ctor_error);
        }

        let mut constructor_fields = Vec::new();
        for &tree in ctor.value_param_clauses.iter().flatten() {
            if !matches!(self.node(tree)?.kind, TreeKind::ValDef(_)) {
                return Err(DeclarationError::InvalidBinding { source, tree });
            }
            let id = self.declaration(tree, class)?.symbol;
            match self.store.symbols.get(id).kind {
                SymbolKind::Field => constructor_fields.push(id),
                SymbolKind::Parameter => {}
                _ => return Err(DeclarationError::InvalidBinding { source, tree }),
            }
        }
        Ok((constructor, constructor_fields))
    }

    fn body_member(
        &self,
        tree: TreeId<Untyped>,
        class: SymbolId,
    ) -> Result<ClassMember, DeclarationError> {
        let source = self.source;
        Ok(match &self.node(tree)?.kind {
            TreeKind::ValDef(field) if field.rhs.is_some() => {
                let decl = self.declaration(tree, class)?;
                if self.store.symbols.get(decl.symbol).kind != SymbolKind::Field {
                    return Err(DeclarationError::InvalidBinding { source, tree });
                }
                ClassMember::Field(decl)
            }
            TreeKind::DefDef(method) if method.rhs.is_some() => {
                let decl = self.declaration(tree, class)?;
                match self.store.symbols.get(decl.symbol).kind {
                    SymbolKind::Method => ClassMember::Method(decl),
                    SymbolKind::Constructor => {
                        return Err(DeclarationError::Unsupported { source, tree });
                    }
                    _ => return Err(DeclarationError::InvalidBinding { source, tree }),
                }
            }
            _ => return Err(DeclarationError::Unsupported { source, tree }),
        })
    }
}
