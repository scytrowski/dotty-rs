//! Compilation-unit naming entry point and traversal seam.

use std::error::Error;
use std::fmt;

use dotty_core::ast::{Modifier, Select, VisibilitySyntax};
use dotty_core::{
    AstArena, Packages, Scope, ScopeId, SemanticStore, SourceId, Symbol, SymbolFlags, SymbolId,
    SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, TreeId, TreeKind, Untyped, Visibility,
};

use crate::SourceSemanticIndex;

/// Internal structural error encountered while indexing a source tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamerError {
    /// A compilation-unit root must be a package definition.
    RootIsNotPackage { tree_index: u32 },
    /// A definition tree was assigned more than one semantic symbol.
    DuplicateSourceTreeSymbol { source: SourceId, tree_index: u32 },
    /// A semantic owner was assigned more than one declaration scope.
    DuplicateDeclarationScope { symbol: SymbolId },
    /// A tree did not have the shape required by a naming routine.
    MalformedAstShape {
        tree_index: u32,
        expected: &'static str,
    },
    /// A source visibility form whose access boundary is not modeled yet.
    UnsupportedVisibility { tree_index: u32 },
}

impl fmt::Display for NamerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootIsNotPackage { tree_index } => {
                write!(
                    f,
                    "compilation-unit root tree {tree_index} is not a PackageDef"
                )
            }
            Self::DuplicateSourceTreeSymbol { source, tree_index } => write!(
                f,
                "source {} tree {tree_index} already has a semantic symbol",
                source.index()
            ),
            Self::DuplicateDeclarationScope { symbol } => {
                write!(
                    f,
                    "symbol {} already has a declaration scope",
                    symbol.index()
                )
            }
            Self::MalformedAstShape {
                tree_index,
                expected,
            } => {
                write!(
                    f,
                    "tree {tree_index} does not have expected shape: {expected}"
                )
            }
            Self::UnsupportedVisibility { tree_index } => write!(
                f,
                "tree {tree_index} has a qualified visibility the namer does not support"
            ),
        }
    }
}

impl Error for NamerError {}

/// Dynamic traversal state for naming declarations.
///
/// This is intentionally local to a naming traversal and is never stored in
/// [`SemanticStore`].
#[derive(Clone, Debug)]
struct NamingContext {
    owner: SymbolId,
    scope: ScopeId,
    package_path: Vec<String>,
}

#[derive(Clone, Copy)]
struct SymbolSpec {
    kind: SymbolKind,
    flags: SymbolFlags,
    visibility: Visibility,
}

/// A declaration identity entered before scanning its nested declarations.
enum EnteredHeader {
    Package {
        tree: TreeId<Untyped>,
        context: NamingContext,
    },
    ClassLike {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
        package_path: Vec<String>,
    },
    Method {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
    },
    SecondaryConstructor {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
    },
    Field {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    },
    TypeAlias {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
    },
}

/// Runs the source naming pass for one parsed compilation unit.
///
/// The pass enters or reuses package symbols and scopes, then enters symbols
/// and declaration scopes for supported class and trait definitions. The
/// returned index maps the package and class/trait trees to their semantic
/// symbols and scopes. These symbols remain incomplete until later compiler
/// phases provide their semantic information.
///
/// If indexing fails, package registrations, allocated store entries, and
/// scope insertions made by this call are rolled back. The input keeps the
/// source ID and file name alongside arena-relative tree IDs. The file name,
/// including its extension, is retained for later top-level wrapper naming.
pub fn name_compilation_unit(
    arena: &AstArena<Untyped>,
    root: TreeId<Untyped>,
    source: SourceId,
    source_file_name: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Result<SourceSemanticIndex, NamerError> {
    let checkpoint = store.checkpoint();
    let package_mark = packages.mark();
    let (result, scope_insertions) = {
        let mut namer = Namer {
            arena,
            source,
            _source_file_name: source_file_name,
            store,
            packages,
            root,
            index: SourceSemanticIndex::new(),
            scope_insertions: Vec::new(),
        };
        let result = namer.index(root).map(|()| namer.index);
        (result, namer.scope_insertions)
    };
    match result {
        Ok(index) => Ok(index),
        Err(error) => {
            for (scope, symbol) in scope_insertions.into_iter().rev() {
                store.scopes.get_mut(scope).remove(symbol);
            }
            packages.roll_back_to(store, package_mark);
            store.rollback_to(checkpoint);
            Err(error)
        }
    }
}

struct Namer<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    _source_file_name: &'a str,
    store: &'a mut SemanticStore,
    packages: &'a mut Packages,
    root: TreeId<Untyped>,
    index: SourceSemanticIndex,
    scope_insertions: Vec<(ScopeId, SymbolId)>,
}

impl Namer<'_> {
    fn index(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        self.expand(tree, &[], true)
    }

    /// Desugaring hook. It is a no-op until source constructs need expansion.
    fn expand(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<(), NamerError> {
        self.index_expanded(tree, enclosing_package, source_root)
    }

    fn index_expanded(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<(), NamerError> {
        let Some(header) = self.enter_package_header(tree, enclosing_package, source_root)? else {
            return Ok(());
        };
        self.scan_entered_header(header)
    }

    fn enter_package_header(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<Option<EnteredHeader>, NamerError> {
        let TreeKind::PackageDef(package) = &self.arena.get(tree).kind else {
            if tree == self.root {
                return Err(NamerError::RootIsNotPackage {
                    tree_index: tree.index(),
                });
            }
            return Ok(None);
        };

        let package = package.clone();
        let segments = if source_root && self.is_empty_package_sentinel(package.name) {
            Vec::new()
        } else {
            self.flatten_package_name(package.name)?
        };
        let mut package_path = enclosing_package.to_vec();
        package_path.extend(segments);

        let entered =
            self.packages
                .enter(self.store, SymbolOrigin::Source(self.source), &package_path);
        let Some(leaf) = entered.last().copied() else {
            return Err(NamerError::MalformedAstShape {
                tree_index: tree.index(),
                expected: "package registry path result",
            });
        };
        self.index.record_symbol(self.source, tree, leaf.symbol)?;
        self.index.record_scope(leaf.symbol, leaf.scope)?;

        let context = NamingContext {
            owner: leaf.symbol,
            scope: leaf.scope,
            package_path,
        };
        Ok(Some(EnteredHeader::Package { tree, context }))
    }

    fn enter_class_or_trait_header(
        &mut self,
        tree: TreeId<Untyped>,
        owner_context: &NamingContext,
    ) -> Result<Option<EnteredHeader>, NamerError> {
        let TreeKind::TypeDef(definition) = &self.arena.get(tree).kind else {
            return Ok(None);
        };
        let definition = definition.clone();
        let TreeKind::Template(template) = &self.arena.get(definition.rhs).kind else {
            return Ok(None);
        };
        let template = template.clone();
        // Enum identity has its own later naming step. Never misclassify it
        // as a class or require class-header constructor data from this pass.
        if definition.metadata.modifiers.contains(&Modifier::Enum) {
            return Ok(None);
        }

        let TreeKind::DefDef(constructor) = &self.arena.get(template.constructor).kind else {
            return Err(NamerError::MalformedAstShape {
                tree_index: template.constructor.index(),
                expected: "DefDef primary constructor",
            });
        };
        let constructor = constructor.clone();
        for parameter in &constructor.type_params {
            let TreeKind::TypeDef(_) = &self.arena.get(*parameter).kind else {
                return Err(NamerError::MalformedAstShape {
                    tree_index: parameter.index(),
                    expected: "TypeDef class type parameter",
                });
            };
        }
        for clause in &constructor.value_param_clauses {
            for parameter in clause {
                let TreeKind::ValDef(parameter_def) = &self.arena.get(*parameter).kind else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter.index(),
                        expected: "ValDef constructor value parameter",
                    });
                };
                if !parameter_def
                    .metadata
                    .modifiers
                    .contains(&Modifier::ParamAccessor)
                {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter.index(),
                        expected: "constructor ValDef with ParamAccessor metadata",
                    });
                }
            }
        }

        let name = *definition.name.as_name();
        let kind = if definition.metadata.modifiers.contains(&Modifier::Trait) {
            SymbolKind::Trait
        } else {
            SymbolKind::Class
        };
        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(owner_context.owner),
            kind,
            flags: Self::source_flags(&definition.metadata.modifiers),
            visibility: self.source_visibility(tree, &definition.metadata.visibility)?,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: self.arena.get(tree).position,
            links: SymbolLinks::default(),
        });
        self.store
            .scopes
            .get_mut(owner_context.scope)
            .enter(name, symbol);
        self.scope_insertions.push((owner_context.scope, symbol));
        self.index.record_symbol(self.source, tree, symbol)?;

        let scope = self.store.scopes.alloc(Scope::new(Some(symbol)));
        self.index.record_scope(symbol, scope)?;
        Ok(Some(EnteredHeader::ClassLike {
            tree,
            symbol,
            scope,
            package_path: owner_context.package_path.clone(),
        }))
    }

    fn scan_entered_header(&mut self, header: EnteredHeader) -> Result<(), NamerError> {
        let (tree, symbol, scope, package_path) = match header {
            EnteredHeader::Package { tree, context } => {
                let TreeKind::PackageDef(package) = &self.arena.get(tree).kind else {
                    return Ok(());
                };
                let mut headers = Vec::new();
                for stat in &package.stats {
                    match self.arena.get(*stat).kind {
                        TreeKind::PackageDef(_) => {
                            if let Some(header) =
                                self.enter_package_header(*stat, &context.package_path, false)?
                            {
                                headers.push(header);
                            }
                        }
                        TreeKind::TypeDef(_) => {
                            if let Some(header) =
                                self.enter_class_or_trait_header(*stat, &context)?
                            {
                                headers.push(header);
                            }
                        }
                        _ => {}
                    }
                }
                for header in headers {
                    self.scan_entered_header(header)?;
                }
                return Ok(());
            }
            EnteredHeader::ClassLike {
                tree,
                symbol,
                scope,
                package_path,
            } => (tree, symbol, scope, package_path),
            EnteredHeader::Method {
                tree,
                symbol,
                scope,
            } => return self.scan_method_parameters(tree, symbol, scope),
            EnteredHeader::SecondaryConstructor {
                tree,
                symbol,
                scope,
            } => return self.scan_constructor_parameters(tree, symbol, scope),
            EnteredHeader::Field { tree, symbol } | EnteredHeader::TypeAlias { tree, symbol } => {
                let _entered_identity = (tree, symbol);
                return Ok(());
            }
        };
        let TreeKind::TypeDef(definition) = &self.arena.get(tree).kind else {
            return Ok(());
        };
        let TreeKind::Template(template) = &self.arena.get(definition.rhs).kind else {
            return Ok(());
        };
        let TreeKind::DefDef(constructor) = &self.arena.get(template.constructor).kind else {
            return Err(NamerError::MalformedAstShape {
                tree_index: template.constructor.index(),
                expected: "DefDef primary constructor",
            });
        };
        let class_context = NamingContext {
            owner: symbol,
            scope,
            package_path,
        };
        let constructor = constructor.clone();
        for parameter_tree in &constructor.type_params {
            let TreeKind::TypeDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                return Err(NamerError::MalformedAstShape {
                    tree_index: parameter_tree.index(),
                    expected: "TypeDef class type parameter",
                });
            };
            self.enter_symbol(
                *parameter_tree,
                *parameter.name.as_name(),
                symbol,
                scope,
                SymbolSpec {
                    kind: SymbolKind::TypeParameter,
                    flags: SymbolFlags::EMPTY,
                    visibility: Visibility::Public,
                },
            )?;
        }
        for clause in &constructor.value_param_clauses {
            for parameter_tree in clause {
                let TreeKind::ValDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter_tree.index(),
                        expected: "ValDef constructor value parameter",
                    });
                };
                if !parameter
                    .metadata
                    .modifiers
                    .contains(&Modifier::ParamAccessor)
                {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter_tree.index(),
                        expected: "constructor ValDef with ParamAccessor metadata",
                    });
                }
                self.enter_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    symbol,
                    scope,
                    SymbolSpec {
                        kind: if parameter
                            .metadata
                            .modifiers
                            .contains(&Modifier::PrivateLocal)
                        {
                            SymbolKind::Parameter
                        } else {
                            SymbolKind::Field
                        },
                        flags: Self::source_flags(&parameter.metadata.modifiers),
                        visibility: self
                            .source_visibility(*parameter_tree, &parameter.metadata.visibility)?,
                    },
                )?;
            }
        }
        self.enter_symbol(
            template.constructor,
            *constructor.name.as_name(),
            symbol,
            scope,
            SymbolSpec {
                kind: SymbolKind::Constructor,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
            },
        )?;

        let mut nested_headers = Vec::new();
        for member in &template.body {
            if *member == template.constructor {
                continue;
            }
            match &self.arena.get(*member).kind {
                TreeKind::TypeDef(definition) => {
                    let definition = definition.clone();
                    if matches!(self.arena.get(definition.rhs).kind, TreeKind::Template(_)) {
                        if let Some(header) =
                            self.enter_class_or_trait_header(*member, &class_context)?
                        {
                            nested_headers.push(header);
                        }
                    } else {
                        let alias = self.enter_symbol(
                            *member,
                            *definition.name.as_name(),
                            symbol,
                            scope,
                            SymbolSpec {
                                kind: SymbolKind::TypeAlias,
                                flags: Self::source_flags(&definition.metadata.modifiers),
                                visibility: self
                                    .source_visibility(*member, &definition.metadata.visibility)?,
                            },
                        )?;
                        nested_headers.push(EnteredHeader::TypeAlias {
                            tree: *member,
                            symbol: alias,
                        });
                    }
                }
                TreeKind::ValDef(definition) => {
                    let definition = definition.clone();
                    let field = self.enter_symbol(
                        *member,
                        *definition.name.as_name(),
                        symbol,
                        scope,
                        SymbolSpec {
                            kind: SymbolKind::Field,
                            flags: Self::source_flags(&definition.metadata.modifiers),
                            visibility: self
                                .source_visibility(*member, &definition.metadata.visibility)?,
                        },
                    )?;
                    nested_headers.push(EnteredHeader::Field {
                        tree: *member,
                        symbol: field,
                    });
                }
                TreeKind::DefDef(definition) => {
                    let definition = definition.clone();
                    let kind =
                        if self.store.names.resolve(definition.name.as_name().text()) == "<init>" {
                            SymbolKind::Constructor
                        } else {
                            SymbolKind::Method
                        };
                    if kind == SymbolKind::Method {
                        nested_headers.push(self.enter_method_header(
                            *member,
                            &definition,
                            symbol,
                            scope,
                        )?);
                    } else {
                        nested_headers.push(self.enter_secondary_constructor_header(
                            *member,
                            &definition,
                            symbol,
                            scope,
                        )?);
                    }
                }
                _ => {}
            }
        }
        for header in nested_headers {
            self.scan_entered_header(header)?;
        }
        Ok(())
    }

    fn enter_method_header(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        owner: SymbolId,
        class_scope: ScopeId,
    ) -> Result<EnteredHeader, NamerError> {
        for parameter in &definition.type_params {
            let TreeKind::TypeDef(_) = &self.arena.get(*parameter).kind else {
                return Err(NamerError::MalformedAstShape {
                    tree_index: parameter.index(),
                    expected: "TypeDef method type parameter",
                });
            };
        }

        for clause in &definition.value_param_clauses {
            for parameter in clause {
                let TreeKind::ValDef(parameter_definition) = &self.arena.get(*parameter).kind
                else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter.index(),
                        expected: "ValDef method value parameter",
                    });
                };
                if parameter_definition
                    .metadata
                    .modifiers
                    .iter()
                    .any(|modifier| {
                        matches!(modifier, Modifier::ParamAccessor | Modifier::PrivateLocal)
                    })
                {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter.index(),
                        expected: "ordinary method ValDef without constructor-role metadata",
                    });
                }
            }
        }

        let method = self.enter_symbol(
            tree,
            *definition.name.as_name(),
            owner,
            class_scope,
            SymbolSpec {
                kind: SymbolKind::Method,
                flags: Self::source_flags(&definition.metadata.modifiers),
                visibility: self.source_visibility(tree, &definition.metadata.visibility)?,
            },
        )?;
        let method_scope = self.store.scopes.alloc(Scope::new(Some(method)));
        self.index.record_scope(method, method_scope)?;

        Ok(EnteredHeader::Method {
            tree,
            symbol: method,
            scope: method_scope,
        })
    }

    fn enter_secondary_constructor_header(
        &mut self,
        tree: TreeId<Untyped>,
        definition: &dotty_core::ast::DefDef<Untyped>,
        owner: SymbolId,
        class_scope: ScopeId,
    ) -> Result<EnteredHeader, NamerError> {
        for clause in &definition.value_param_clauses {
            for parameter in clause {
                let TreeKind::ValDef(_) = &self.arena.get(*parameter).kind else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter.index(),
                        expected: "ValDef secondary constructor value parameter",
                    });
                };
            }
        }

        let constructor = self.enter_symbol(
            tree,
            *definition.name.as_name(),
            owner,
            class_scope,
            SymbolSpec {
                kind: SymbolKind::Constructor,
                flags: Self::source_flags(&definition.metadata.modifiers),
                visibility: self.source_visibility(tree, &definition.metadata.visibility)?,
            },
        )?;
        let constructor_scope = self.store.scopes.alloc(Scope::new(Some(constructor)));
        self.index.record_scope(constructor, constructor_scope)?;

        Ok(EnteredHeader::SecondaryConstructor {
            tree,
            symbol: constructor,
            scope: constructor_scope,
        })
    }

    fn scan_method_parameters(
        &mut self,
        tree: TreeId<Untyped>,
        method: SymbolId,
        method_scope: ScopeId,
    ) -> Result<(), NamerError> {
        let TreeKind::DefDef(definition) = &self.arena.get(tree).kind else {
            return Ok(());
        };
        for parameter_tree in &definition.type_params {
            let TreeKind::TypeDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                return Err(NamerError::MalformedAstShape {
                    tree_index: parameter_tree.index(),
                    expected: "TypeDef method type parameter",
                });
            };
            self.enter_symbol(
                *parameter_tree,
                *parameter.name.as_name(),
                method,
                method_scope,
                SymbolSpec {
                    kind: SymbolKind::TypeParameter,
                    flags: SymbolFlags::EMPTY,
                    visibility: Visibility::Public,
                },
            )?;
        }
        for clause in &definition.value_param_clauses {
            for parameter_tree in clause {
                let TreeKind::ValDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter_tree.index(),
                        expected: "ValDef method value parameter",
                    });
                };
                if parameter.metadata.modifiers.iter().any(|modifier| {
                    matches!(modifier, Modifier::ParamAccessor | Modifier::PrivateLocal)
                }) {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter_tree.index(),
                        expected: "ordinary method ValDef without constructor-role metadata",
                    });
                }
                self.enter_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    method,
                    method_scope,
                    SymbolSpec {
                        kind: SymbolKind::Parameter,
                        flags: Self::source_flags(&parameter.metadata.modifiers),
                        visibility: Visibility::Public,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn scan_constructor_parameters(
        &mut self,
        tree: TreeId<Untyped>,
        constructor: SymbolId,
        constructor_scope: ScopeId,
    ) -> Result<(), NamerError> {
        let TreeKind::DefDef(definition) = &self.arena.get(tree).kind else {
            return Ok(());
        };
        for clause in &definition.value_param_clauses {
            for parameter_tree in clause {
                let TreeKind::ValDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                    return Err(NamerError::MalformedAstShape {
                        tree_index: parameter_tree.index(),
                        expected: "ValDef secondary constructor value parameter",
                    });
                };
                self.enter_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    constructor,
                    constructor_scope,
                    SymbolSpec {
                        kind: SymbolKind::Parameter,
                        flags: Self::source_flags(&parameter.metadata.modifiers),
                        visibility: Visibility::Public,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn source_flags(modifiers: &[Modifier]) -> SymbolFlags {
        modifiers
            .iter()
            .fold(SymbolFlags::EMPTY, |flags, modifier| {
                let flag = match modifier {
                    Modifier::Abstract => SymbolFlags::ABSTRACT,
                    Modifier::Final => SymbolFlags::FINAL,
                    Modifier::Sealed => SymbolFlags::SEALED,
                    Modifier::Case => SymbolFlags::CASE,
                    Modifier::Implicit => SymbolFlags::IMPLICIT,
                    Modifier::Given => SymbolFlags::GIVEN,
                    Modifier::Lazy => SymbolFlags::LAZY,
                    Modifier::Var => SymbolFlags::MUTABLE,
                    Modifier::Override => SymbolFlags::OVERRIDE,
                    Modifier::Inline => SymbolFlags::INLINE,
                    Modifier::Transparent => SymbolFlags::TRANSPARENT,
                    Modifier::Opaque => SymbolFlags::OPAQUE,
                    Modifier::Erased => SymbolFlags::ERASED,
                    _ => SymbolFlags::EMPTY,
                };
                flags | flag
            })
    }

    fn enter_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        name: dotty_core::Name,
        owner: SymbolId,
        scope: ScopeId,
        spec: SymbolSpec,
    ) -> Result<SymbolId, NamerError> {
        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(owner),
            kind: spec.kind,
            flags: spec.flags,
            visibility: spec.visibility,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: self.arena.get(tree).position,
            links: SymbolLinks::default(),
        });
        self.store.scopes.get_mut(scope).enter(name, symbol);
        self.index.record_symbol(self.source, tree, symbol)?;
        Ok(symbol)
    }

    fn is_empty_package_sentinel(&self, name: TreeId<Untyped>) -> bool {
        matches!(
            &self.arena.get(name).kind,
            TreeKind::Ident(ident)
                if !ident.backquoted && self.store.names.resolve(ident.name.text()) == "<empty>"
        )
    }

    fn source_visibility(
        &self,
        tree: TreeId<Untyped>,
        visibility: &Option<VisibilitySyntax>,
    ) -> Result<Visibility, NamerError> {
        let supported_this_qualifier = |qualifier: Option<dotty_core::Name>| {
            qualifier.is_some_and(|name| self.store.names.resolve(name.text()) == "this")
        };
        match visibility {
            None => Ok(Visibility::Public),
            Some(VisibilitySyntax::Private { qualifier })
                if qualifier.is_none() || supported_this_qualifier(*qualifier) =>
            {
                Ok(Visibility::Private)
            }
            Some(VisibilitySyntax::Protected { qualifier })
                if qualifier.is_none() || supported_this_qualifier(*qualifier) =>
            {
                Ok(Visibility::Protected)
            }
            Some(VisibilitySyntax::Private { .. } | VisibilitySyntax::Protected { .. }) => {
                Err(NamerError::UnsupportedVisibility {
                    tree_index: tree.index(),
                })
            }
        }
    }

    fn flatten_package_name(&self, tree: TreeId<Untyped>) -> Result<Vec<String>, NamerError> {
        let mut segments = Vec::new();
        self.flatten_package_name_into(tree, &mut segments)?;
        Ok(segments)
    }

    fn flatten_package_name_into(
        &self,
        tree: TreeId<Untyped>,
        segments: &mut Vec<String>,
    ) -> Result<(), NamerError> {
        match &self.arena.get(tree).kind {
            TreeKind::Ident(ident) => {
                segments.push(self.store.names.resolve(ident.name.text()).to_owned());
                Ok(())
            }
            TreeKind::Select(Select {
                qualifier, name, ..
            }) => {
                self.flatten_package_name_into(*qualifier, segments)?;
                segments.push(self.store.names.resolve(name.text()).to_owned());
                Ok(())
            }
            _ => Err(NamerError::MalformedAstShape {
                tree_index: tree.index(),
                expected: "Ident or qualified Select package name",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::ast::{
        DefDef, Ident, Literal, Modifier, Modifiers, PackageDef, Template, TypeDef, TypeTree,
        UntypedTemplateMetadata, ValDef, VisibilitySyntax,
    };
    use dotty_core::{
        AstArena, Packages, SemanticStore, SourceId, SourceSpan, Span, SymbolFlags, SymbolInfo,
        SymbolKind, SymbolOrigin, TextRange, Tree, TreeKind, TypeName, Untyped, Visibility,
    };

    use super::*;

    fn ident(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        text: &str,
    ) -> TreeId<Untyped> {
        let name = store.names.intern(text);
        arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *dotty_core::TermName::new(name).as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        })
    }

    fn backquoted_ident(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        text: &str,
    ) -> TreeId<Untyped> {
        let name = store.names.intern(text);
        arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *dotty_core::TermName::new(name).as_name(),
                backquoted: true,
            }),
            position: None,
            ty: (),
        })
    }

    fn select(
        arena: &mut AstArena<Untyped>,
        qualifier: TreeId<Untyped>,
        store: &mut SemanticStore,
        text: &str,
    ) -> TreeId<Untyped> {
        let name = store.names.intern(text);
        arena.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier,
                name: *dotty_core::TermName::new(name).as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        })
    }

    fn package(
        arena: &mut AstArena<Untyped>,
        name: TreeId<Untyped>,
        stats: Vec<TreeId<Untyped>>,
    ) -> TreeId<Untyped> {
        arena.alloc(Tree {
            kind: TreeKind::PackageDef(PackageDef { name, stats }),
            position: None,
            ty: (),
        })
    }

    fn type_tree(arena: &mut AstArena<Untyped>) -> TreeId<Untyped> {
        arena.alloc(Tree {
            kind: TreeKind::TypeTree(TypeTree),
            position: None,
            ty: (),
        })
    }

    fn constructor(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        let tpt = type_tree(arena);
        let name = dotty_core::TermName::new(store.names.intern("<init>"));
        arena.alloc(Tree {
            kind: TreeKind::DefDef(DefDef {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
            position,
            ty: (),
        })
    }

    fn method_definition(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        let tpt = type_tree(arena);
        let name = dotty_core::TermName::new(store.names.intern(name));
        arena.alloc(Tree {
            kind: TreeKind::DefDef(DefDef {
                name,
                type_params,
                value_param_clauses,
                tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
            position,
            ty: (),
        })
    }

    #[allow(clippy::too_many_arguments)] // Test fixture exposes each AST component explicitly.
    fn class_definition_with_header(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        body: Vec<TreeId<Untyped>>,
        position: Option<SourceSpan>,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        constructor_position: Option<SourceSpan>,
    ) -> (TreeId<Untyped>, TreeId<Untyped>) {
        class_definition_with_visibility_header(
            arena,
            store,
            name,
            modifiers,
            None,
            body,
            position,
            type_params,
            value_param_clauses,
            constructor_position,
        )
    }

    fn class_definition_with_visibility(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        visibility: Option<VisibilitySyntax>,
        body: Vec<TreeId<Untyped>>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        class_definition_with_visibility_header(
            arena,
            store,
            name,
            modifiers,
            visibility,
            body,
            position,
            vec![],
            vec![],
            None,
        )
        .0
    }

    #[allow(clippy::too_many_arguments)] // Test fixture exposes each AST component explicitly.
    fn class_definition_with_visibility_header(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        visibility: Option<VisibilitySyntax>,
        body: Vec<TreeId<Untyped>>,
        position: Option<SourceSpan>,
        type_params: Vec<TreeId<Untyped>>,
        value_param_clauses: Vec<Vec<TreeId<Untyped>>>,
        constructor_position: Option<SourceSpan>,
    ) -> (TreeId<Untyped>, TreeId<Untyped>) {
        let constructor = constructor(
            arena,
            store,
            type_params,
            value_param_clauses,
            constructor_position,
        );
        let template = arena.alloc(Tree {
            kind: TreeKind::Template(Template {
                constructor,
                parents: vec![],
                self_val: None,
                body,
                metadata: UntypedTemplateMetadata::default(),
            }),
            position: None,
            ty: (),
        });
        let name_id = store.names.intern(name);
        let definition = arena.alloc(Tree {
            kind: TreeKind::TypeDef(TypeDef {
                name: TypeName::new(name_id),
                rhs: template,
                metadata: Modifiers {
                    visibility,
                    modifiers,
                    ..Modifiers::default()
                },
                variance: None,
            }),
            position,
            ty: (),
        });
        (definition, constructor)
    }

    fn class_definition(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        body: Vec<TreeId<Untyped>>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        class_definition_with_header(
            arena,
            store,
            name,
            modifiers,
            body,
            position,
            vec![],
            vec![],
            None,
        )
        .0
    }

    fn type_parameter(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        let rhs = type_tree(arena);
        let name = TypeName::new(store.names.intern(name));
        arena.alloc(Tree {
            kind: TreeKind::TypeDef(TypeDef {
                name,
                rhs,
                metadata: Modifiers {
                    modifiers: vec![Modifier::Param],
                    ..Modifiers::default()
                },
                variance: None,
            }),
            position,
            ty: (),
        })
    }

    fn value_parameter(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        position: Option<SourceSpan>,
    ) -> TreeId<Untyped> {
        let tpt = type_tree(arena);
        let name = dotty_core::TermName::new(store.names.intern(name));
        arena.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: None,
                metadata: Modifiers {
                    modifiers,
                    ..Modifiers::default()
                },
            }),
            position,
            ty: (),
        })
    }

    fn package_with_stat(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        segment: &str,
        stats: Vec<TreeId<Untyped>>,
    ) -> TreeId<Untyped> {
        let name = ident(arena, store, segment);
        package(arena, name, stats)
    }

    fn type_symbol(
        store: &mut SemanticStore,
        scope: dotty_core::ScopeId,
        text: &str,
    ) -> Option<SymbolId> {
        let name = TypeName::new(store.names.intern(text));
        store.scopes.get(scope).lookup(name.as_name())
    }

    fn term_symbol(
        store: &mut SemanticStore,
        scope: dotty_core::ScopeId,
        text: &str,
    ) -> Option<SymbolId> {
        let name = dotty_core::TermName::new(store.names.intern(text));
        store.scopes.get(scope).lookup(name.as_name())
    }

    fn name_package(
        arena: &AstArena<Untyped>,
        root: TreeId<Untyped>,
        source: u32,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> Result<SourceSemanticIndex, NamerError> {
        name_compilation_unit(
            arena,
            root,
            SourceId::from_index(source),
            "Example.scala",
            store,
            packages,
        )
    }

    fn entered_class_flags(modifiers: Vec<Modifier>) -> SymbolFlags {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "Flagged", modifiers, vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "flags", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 23, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["flags"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "Flagged").unwrap();
        store.symbols.get(symbol).flags
    }

    #[test]
    fn non_package_compilation_unit_root_is_rejected() {
        let mut arena = AstArena::<Untyped>::new();
        let root = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });

        assert_eq!(
            name_compilation_unit(
                &arena,
                root,
                SourceId::from_index(1),
                "Bad.scala",
                &mut SemanticStore::new(),
                &mut Packages::new(),
            )
            .unwrap_err(),
            NamerError::RootIsNotPackage { tree_index: 0 }
        );
    }

    #[test]
    fn synthetic_empty_package_uses_root_and_never_creates_an_empty_segment() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let sentinel = ident(&mut arena, &mut store, "<empty>");
        let root = package(&mut arena, sentinel, vec![]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 1, &mut store, &mut packages).unwrap();
        let root_package = packages.get::<&str>(&[]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(1), root),
            Some(root_package.symbol)
        );
        assert_eq!(
            index.scope_of(root_package.symbol),
            Some(root_package.scope)
        );
        assert!(packages.get(&["<empty>"]).is_none());
        assert_eq!(
            store.symbols.get(root_package.symbol).origin,
            SymbolOrigin::Source(SourceId::from_index(1))
        );
    }

    #[test]
    fn quoted_empty_package_name_is_a_real_package_segment() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let quoted_name = backquoted_ident(&mut arena, &mut store, "<empty>");
        let root = package(&mut arena, quoted_name, vec![]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 18, &mut store, &mut packages).unwrap();
        let named_package = packages.get(&["<empty>"]).unwrap();

        assert_ne!(named_package, packages.get::<&str>(&[]).unwrap());
        assert_eq!(
            index.symbol_at(SourceId::from_index(18), root),
            Some(named_package.symbol)
        );
        assert_eq!(
            store
                .names
                .resolve(store.symbols.get(named_package.symbol).name.text()),
            "<empty>"
        );
    }

    #[test]
    fn qualified_package_name_enters_each_segment_and_maps_the_leaf() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let foo = ident(&mut arena, &mut store, "foo");
        let bar = select(&mut arena, foo, &mut store, "bar");
        let baz = select(&mut arena, bar, &mut store, "baz");
        let root = package(&mut arena, baz, vec![]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 2, &mut store, &mut packages).unwrap();
        let leaf = packages.get(&["foo", "bar", "baz"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(2), root),
            Some(leaf.symbol)
        );
        assert_eq!(index.scope_of(leaf.symbol), Some(leaf.scope));
        assert!(packages.get(&["foo"]).is_some());
        assert!(packages.get(&["foo", "bar"]).is_some());
    }

    #[test]
    fn nested_package_clauses_are_relative_to_the_enclosing_package() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let foo_name = ident(&mut arena, &mut store, "foo");
        let bar_name = ident(&mut arena, &mut store, "bar");
        let nested = package(&mut arena, bar_name, vec![]);
        let root = package(&mut arena, foo_name, vec![nested]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 3, &mut store, &mut packages).unwrap();
        let foo = packages.get(&["foo"]).unwrap();
        let foo_bar = packages.get(&["foo", "bar"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(3), root),
            Some(foo.symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(3), nested),
            Some(foo_bar.symbol)
        );
        assert_eq!(store.symbols.owner(foo_bar.symbol), Some(foo.symbol));
    }

    #[test]
    fn separate_source_units_reuse_the_same_package_symbol_and_scope() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();

        let mut first_arena = AstArena::<Untyped>::new();
        let first_name = ident(&mut first_arena, &mut store, "shared");
        let first_root = package(&mut first_arena, first_name, vec![]);
        let first = name_package(&first_arena, first_root, 4, &mut store, &mut packages).unwrap();

        let mut second_arena = AstArena::<Untyped>::new();
        let second_name = ident(&mut second_arena, &mut store, "shared");
        let second_root = package(&mut second_arena, second_name, vec![]);
        let second =
            name_package(&second_arena, second_root, 5, &mut store, &mut packages).unwrap();
        let package = packages.get(&["shared"]).unwrap();

        assert_eq!(
            first.symbol_at(SourceId::from_index(4), first_root),
            Some(package.symbol)
        );
        assert_eq!(
            second.symbol_at(SourceId::from_index(5), second_root),
            Some(package.symbol)
        );
        assert_eq!(first.scope_of(package.symbol), Some(package.scope));
        assert_eq!(second.scope_of(package.symbol), Some(package.scope));
    }

    #[test]
    fn repeated_package_path_in_one_source_reuses_the_registered_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let outer_name = ident(&mut arena, &mut store, "outer");
        let first_inner_name = ident(&mut arena, &mut store, "inner");
        let second_inner_name = ident(&mut arena, &mut store, "inner");
        let first_inner = package(&mut arena, first_inner_name, vec![]);
        let second_inner = package(&mut arena, second_inner_name, vec![]);
        let root = package(&mut arena, outer_name, vec![first_inner, second_inner]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 10, &mut store, &mut packages).unwrap();
        let inner = packages.get(&["outer", "inner"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(10), first_inner),
            Some(inner.symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(10), second_inner),
            Some(inner.symbol)
        );
        assert_eq!(index.scope_of(inner.symbol), Some(inner.scope));
    }

    #[test]
    fn package_created_by_another_adapter_keeps_its_existing_origin() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let prior_origin = SymbolOrigin::Builtin;
        packages.enter(&mut store, prior_origin, &["existing"]);

        let mut arena = AstArena::<Untyped>::new();
        let name = ident(&mut arena, &mut store, "existing");
        let root = package(&mut arena, name, vec![]);
        let _index = name_package(&arena, root, 6, &mut store, &mut packages).unwrap();
        let package = packages.get(&["existing"]).unwrap();

        assert_eq!(store.symbols.get(package.symbol).origin, prior_origin);
    }

    #[test]
    fn class_in_a_named_package_is_entered_in_the_package_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "foo", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 11, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["foo"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "C").unwrap();

        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Class);
        assert_eq!(store.symbols.get(symbol).owner, Some(owner.symbol));
        assert_eq!(
            index.symbol_at(SourceId::from_index(11), class),
            Some(symbol)
        );
    }

    #[test]
    fn private_class_visibility_is_preserved() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "PrivateClass",
            vec![],
            Some(VisibilitySyntax::Private { qualifier: None }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 19, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["visibility"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "PrivateClass").unwrap();

        assert_eq!(store.symbols.get(symbol).visibility, Visibility::Private);
    }

    #[test]
    fn protected_class_visibility_is_preserved() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "ProtectedClass",
            vec![],
            Some(VisibilitySyntax::Protected { qualifier: None }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 20, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["visibility"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "ProtectedClass").unwrap();

        assert_eq!(store.symbols.get(symbol).visibility, Visibility::Protected);
    }

    #[test]
    fn unsupported_qualified_class_visibility_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier_id = store.names.intern("outer");
        let qualifier = *dotty_core::TermName::new(qualifier_id).as_name();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "QualifiedPrivate",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 21, &mut store, &mut packages).unwrap_err(),
            NamerError::UnsupportedVisibility {
                tree_index: class.index()
            }
        );
        assert!(packages.get(&["visibility"]).is_none());
    }

    #[test]
    fn trait_marker_selects_trait_kind() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let trait_tree = class_definition(
            &mut arena,
            &mut store,
            "Showable",
            vec![Modifier::Trait],
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "traits", vec![trait_tree]);
        let mut packages = Packages::new();

        name_package(&arena, root, 12, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["traits"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "Showable").unwrap();

        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Trait);
    }

    #[test]
    fn case_class_remains_class_kind() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(
            &mut arena,
            &mut store,
            "Point",
            vec![Modifier::Case],
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "geometry", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 13, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["geometry"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "Point").unwrap();

        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Class);
    }

    #[test]
    fn final_class_sets_the_final_symbol_flag() {
        assert_eq!(
            entered_class_flags(vec![Modifier::Final]),
            SymbolFlags::FINAL
        );
    }

    #[test]
    fn sealed_class_sets_the_sealed_symbol_flag() {
        assert_eq!(
            entered_class_flags(vec![Modifier::Sealed]),
            SymbolFlags::SEALED
        );
    }

    #[test]
    fn abstract_class_sets_the_abstract_symbol_flag() {
        assert_eq!(
            entered_class_flags(vec![Modifier::Abstract]),
            SymbolFlags::ABSTRACT
        );
    }

    #[test]
    fn case_class_sets_the_case_symbol_flag() {
        assert_eq!(entered_class_flags(vec![Modifier::Case]), SymbolFlags::CASE);
    }

    #[test]
    fn implicit_class_sets_the_implicit_symbol_flag() {
        assert_eq!(
            entered_class_flags(vec![Modifier::Implicit]),
            SymbolFlags::IMPLICIT
        );
    }

    #[test]
    fn nested_class_and_trait_are_owned_by_the_enclosing_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let nested_class = class_definition(&mut arena, &mut store, "Nested", vec![], vec![], None);
        let nested_trait = class_definition(
            &mut arena,
            &mut store,
            "NestedTrait",
            vec![Modifier::Trait],
            vec![],
            None,
        );
        let outer = class_definition(
            &mut arena,
            &mut store,
            "Outer",
            vec![],
            vec![nested_class, nested_trait],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "nesting", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 14, &mut store, &mut packages).unwrap();
        let package_owner = packages.get(&["nesting"]).unwrap();
        let outer_symbol = type_symbol(&mut store, package_owner.scope, "Outer").unwrap();
        let outer_scope = index.scope_of(outer_symbol).unwrap();
        let nested_symbol = type_symbol(&mut store, outer_scope, "Nested").unwrap();
        let nested_trait_symbol = type_symbol(&mut store, outer_scope, "NestedTrait").unwrap();

        assert_eq!(store.symbols.get(nested_symbol).owner, Some(outer_symbol));
        assert_eq!(
            store.symbols.get(nested_trait_symbol).owner,
            Some(outer_symbol)
        );
        assert_eq!(
            store.symbols.get(nested_trait_symbol).kind,
            SymbolKind::Trait
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(14), nested_class),
            Some(nested_symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(14), nested_trait),
            Some(nested_trait_symbol)
        );
    }

    #[test]
    fn class_scope_belongs_to_class_and_symbol_stays_incomplete() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "Incomplete", vec![], vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "scopecheck", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 15, &mut store, &mut packages).unwrap();
        let package_owner = packages.get(&["scopecheck"]).unwrap();
        let symbol = type_symbol(&mut store, package_owner.scope, "Incomplete").unwrap();
        let scope = index.scope_of(symbol).unwrap();

        assert_eq!(store.scopes.get(scope).owner, Some(symbol));
        assert_eq!(store.symbols.get(symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(store.symbols.get(symbol).visibility, Visibility::Public);
        assert_eq!(
            store.symbols.get(symbol).origin,
            SymbolOrigin::Source(SourceId::from_index(15))
        );
    }

    #[test]
    fn class_source_position_is_preserved() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(16);
        let range = TextRange::new(20, 35).unwrap();
        let position = Some(SourceSpan::new(source, Span::without_point(range)));
        let class = class_definition(&mut arena, &mut store, "Located", vec![], vec![], position);
        let root = package_with_stat(&mut arena, &mut store, "positions", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 16, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["positions"]).unwrap();
        let symbol = type_symbol(&mut store, owner.scope, "Located").unwrap();

        assert_eq!(store.symbols.get(symbol).position, position);
    }

    #[test]
    fn enum_template_is_not_entered_as_an_ordinary_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let enumeration = class_definition(
            &mut arena,
            &mut store,
            "Color",
            vec![Modifier::Enum],
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "enums", vec![enumeration]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 17, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["enums"]).unwrap();

        assert_eq!(type_symbol(&mut store, owner.scope, "Color"), None);
        assert_eq!(index.symbol_at(SourceId::from_index(17), enumeration), None);
    }

    #[test]
    fn class_type_parameters_are_owned_and_entered_in_class_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = type_parameter(&mut arena, &mut store, "A", None);
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "Generic",
            vec![],
            vec![],
            None,
            vec![parameter],
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "generics", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 18, &mut store, &mut packages).unwrap();
        let package = packages.get(&["generics"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "Generic").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let parameter_symbol = type_symbol(&mut store, class_scope, "A").unwrap();

        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::TypeParameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(class_symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(18), parameter),
            Some(parameter_symbol)
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).info,
            SymbolInfo::Missing
        );
    }

    #[test]
    fn constructor_only_parameter_is_available_in_class_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "params", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 19, &mut store, &mut packages).unwrap();
        let package = packages.get(&["params"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let parameter_symbol = index
            .symbol_at(SourceId::from_index(19), parameter)
            .unwrap();

        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(class_symbol)
        );
        assert_eq!(
            term_symbol(&mut store, class_scope, "x"),
            Some(parameter_symbol)
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).visibility,
            Visibility::Public
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).info,
            SymbolInfo::Missing
        );
    }

    #[test]
    fn val_constructor_parameter_is_a_class_field() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "vals", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 20, &mut store, &mut packages).unwrap();
        let package = packages.get(&["vals"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let field_symbol = term_symbol(&mut store, class_scope, "x").unwrap();

        assert_eq!(store.symbols.get(field_symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(field_symbol).owner, Some(class_symbol));
        assert_eq!(store.symbols.get(field_symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(
            index.symbol_at(SourceId::from_index(20), parameter),
            Some(field_symbol)
        );
    }

    #[test]
    fn direct_class_val_is_entered_as_an_incomplete_field() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(30);
        let field = value_parameter(&mut arena, &mut store, "member", vec![], None);
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![field], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 30, &mut store, &mut packages).unwrap();
        let package = packages.get(&["members"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let field_symbol = term_symbol(&mut store, class_scope, "member").unwrap();

        assert_eq!(store.symbols.get(field_symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(field_symbol).owner, Some(class_symbol));
        assert_eq!(store.symbols.get(field_symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(field_symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(
            store.symbols.get(field_symbol).visibility,
            Visibility::Public
        );
        assert_eq!(
            store.symbols.get(field_symbol).origin,
            SymbolOrigin::Source(source)
        );
        assert_eq!(index.symbol_at(source, field), Some(field_symbol));
    }

    #[test]
    fn direct_class_val_preserves_var_flag_and_private_visibility() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let field = value_parameter(&mut arena, &mut store, "count", vec![Modifier::Var], None);
        let TreeKind::ValDef(definition) = &mut arena.get_mut(field).kind else {
            unreachable!("value_parameter constructs a ValDef");
        };
        definition.metadata.visibility = Some(VisibilitySyntax::Private { qualifier: None });
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![field], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 42, &mut store, &mut packages).unwrap();
        let package = packages.get(&["members"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let field_symbol = index.symbol_at(SourceId::from_index(42), field).unwrap();

        assert_eq!(store.symbols.get(field_symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(field_symbol).flags, SymbolFlags::MUTABLE);
        assert_eq!(
            store.symbols.get(field_symbol).visibility,
            Visibility::Private
        );
        assert_eq!(store.symbols.get(field_symbol).owner, Some(class_symbol));
    }

    #[test]
    fn direct_class_lazy_val_preserves_lazy_flag() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let field = value_parameter(&mut arena, &mut store, "cached", vec![Modifier::Lazy], None);
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![field], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 43, &mut store, &mut packages).unwrap();
        let field_symbol = index.symbol_at(SourceId::from_index(43), field).unwrap();

        assert_eq!(store.symbols.get(field_symbol).flags, SymbolFlags::LAZY);
    }

    #[test]
    fn direct_class_method_is_owned_mapped_and_has_a_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(31);
        let method = method_definition(&mut arena, &mut store, "compute", vec![], vec![], None);
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 31, &mut store, &mut packages).unwrap();
        let package = packages.get(&["members"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let method_symbol = term_symbol(&mut store, class_scope, "compute").unwrap();
        let method_scope = index.scope_of(method_symbol).unwrap();

        assert_eq!(store.symbols.get(method_symbol).kind, SymbolKind::Method);
        assert_eq!(store.symbols.get(method_symbol).owner, Some(class_symbol));
        assert_eq!(store.symbols.get(method_symbol).info, SymbolInfo::Missing);
        assert_eq!(
            store.symbols.get(method_symbol).origin,
            SymbolOrigin::Source(source)
        );
        assert_eq!(store.scopes.get(method_scope).owner, Some(method_symbol));
        assert_eq!(index.symbol_at(source, method), Some(method_symbol));
    }

    #[test]
    fn direct_member_headers_precede_method_parameters() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(&mut arena, &mut store, "input", vec![], None);
        let method = method_definition(
            &mut arena,
            &mut store,
            "first",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let field = value_parameter(&mut arena, &mut store, "second", vec![], None);
        let class = class_definition(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![method, field],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "headers", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 71, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(71);
        let method_symbol = index.symbol_at(source, method).unwrap();
        let field_symbol = index.symbol_at(source, field).unwrap();
        let parameter_symbol = index.symbol_at(source, parameter).unwrap();

        assert!(method_symbol.index() < field_symbol.index());
        assert!(field_symbol.index() < parameter_symbol.index());
        assert_eq!(
            store.symbols.get(method_symbol).owner,
            store.symbols.get(field_symbol).owner
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(method_symbol)
        );
    }

    #[test]
    fn sibling_nested_class_headers_precede_descendant_class_headers() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let deep = class_definition(&mut arena, &mut store, "Deep", vec![], vec![], None);
        let first = class_definition(&mut arena, &mut store, "First", vec![], vec![deep], None);
        let sibling = class_definition(&mut arena, &mut store, "Sibling", vec![], vec![], None);
        let outer = class_definition(
            &mut arena,
            &mut store,
            "Outer",
            vec![],
            vec![first, sibling],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "nested_headers", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 72, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(72);
        let first_symbol = index.symbol_at(source, first).unwrap();
        let sibling_symbol = index.symbol_at(source, sibling).unwrap();
        let deep_symbol = index.symbol_at(source, deep).unwrap();

        assert!(first_symbol.index() < sibling_symbol.index());
        assert_eq!(
            store.symbols.get(first_symbol).owner,
            store.symbols.get(sibling_symbol).owner
        );
        assert_eq!(
            store.symbols.get(first_symbol).owner,
            store.symbols.get(sibling_symbol).owner
        );
        assert_eq!(store.symbols.get(deep_symbol).owner, Some(first_symbol));
    }

    #[test]
    fn package_class_headers_precede_class_header_parameters() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "value",
            vec![Modifier::ParamAccessor],
            None,
        );
        let first = class_definition_with_header(
            &mut arena,
            &mut store,
            "First",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        )
        .0;
        let sibling = class_definition(&mut arena, &mut store, "Sibling", vec![], vec![], None);
        let root = package_with_stat(
            &mut arena,
            &mut store,
            "package_headers",
            vec![first, sibling],
        );
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 73, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(73);
        let first_symbol = index.symbol_at(source, first).unwrap();
        let sibling_symbol = index.symbol_at(source, sibling).unwrap();
        let parameter_symbol = index.symbol_at(source, parameter).unwrap();

        assert!(first_symbol.index() < sibling_symbol.index());
        assert!(sibling_symbol.index() < parameter_symbol.index());
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(first_symbol)
        );
    }

    #[test]
    fn sibling_package_headers_precede_descendant_class_headers() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let deep = class_definition(&mut arena, &mut store, "Deep", vec![], vec![], None);
        let first_class =
            class_definition(&mut arena, &mut store, "First", vec![], vec![deep], None);
        let first_package = package_with_stat(&mut arena, &mut store, "first", vec![first_class]);
        let sibling_class =
            class_definition(&mut arena, &mut store, "Sibling", vec![], vec![], None);
        let sibling_package =
            package_with_stat(&mut arena, &mut store, "sibling", vec![sibling_class]);
        let root = package_with_stat(
            &mut arena,
            &mut store,
            "root",
            vec![first_package, sibling_package],
        );
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 74, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(74);
        let deep_symbol = index.symbol_at(source, deep).unwrap();
        let sibling_symbol = index.symbol_at(source, sibling_class).unwrap();
        let first_package_symbol = index.symbol_at(source, first_package).unwrap();
        let sibling_package_symbol = index.symbol_at(source, sibling_package).unwrap();

        assert!(first_package_symbol.index() < sibling_package_symbol.index());
        assert!(sibling_package_symbol.index() < deep_symbol.index());
        assert_eq!(
            store.symbols.get(sibling_symbol).owner,
            Some(sibling_package_symbol)
        );
    }

    #[test]
    fn direct_class_method_preserves_visibility_and_modifier_flags() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let method = method_definition(&mut arena, &mut store, "run", vec![], vec![], None);
        let TreeKind::DefDef(definition) = &mut arena.get_mut(method).kind else {
            unreachable!("method_definition constructs a DefDef");
        };
        definition.metadata.modifiers = vec![Modifier::Final, Modifier::Override, Modifier::Inline];
        definition.metadata.visibility = Some(VisibilitySyntax::Private { qualifier: None });
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 44, &mut store, &mut packages).unwrap();
        let method_symbol = index.symbol_at(SourceId::from_index(44), method).unwrap();

        assert_eq!(
            store.symbols.get(method_symbol).flags,
            SymbolFlags::FINAL | SymbolFlags::OVERRIDE | SymbolFlags::INLINE
        );
        assert_eq!(
            store.symbols.get(method_symbol).visibility,
            Visibility::Private
        );
    }

    #[test]
    fn overloaded_class_methods_keep_each_symbol_in_declaration_order() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let first = method_definition(&mut arena, &mut store, "lookup", vec![], vec![], None);
        let second = method_definition(&mut arena, &mut store, "lookup", vec![], vec![], None);
        let class = class_definition(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![first, second],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "overloads", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 32, &mut store, &mut packages).unwrap();
        let package = packages.get(&["overloads"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let name = dotty_core::TermName::new(store.names.intern("lookup"));
        let overloads = store.scopes.get(class_scope).lookup_all(name.as_name());
        let first_symbol = index.symbol_at(SourceId::from_index(32), first).unwrap();
        let second_symbol = index.symbol_at(SourceId::from_index(32), second).unwrap();

        assert_eq!(overloads, &[first_symbol, second_symbol]);
        assert_ne!(first_symbol, second_symbol);
    }

    #[test]
    fn class_type_alias_is_owned_mapped_and_left_incomplete() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(38);
        let rhs = type_tree(&mut arena);
        let alias = arena.alloc(Tree {
            kind: TreeKind::TypeDef(TypeDef {
                name: TypeName::new(store.names.intern("Alias")),
                rhs,
                metadata: Modifiers::default(),
                variance: None,
            }),
            position: None,
            ty: (),
        });
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![alias], None);
        let root = package_with_stat(&mut arena, &mut store, "aliases", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 38, &mut store, &mut packages).unwrap();
        let package = packages.get(&["aliases"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let alias_symbol = type_symbol(&mut store, class_scope, "Alias").unwrap();

        assert_eq!(store.symbols.get(alias_symbol).kind, SymbolKind::TypeAlias);
        assert_eq!(store.symbols.get(alias_symbol).owner, Some(class_symbol));
        assert_eq!(store.symbols.get(alias_symbol).info, SymbolInfo::Missing);
        assert_eq!(index.symbol_at(source, alias), Some(alias_symbol));
    }

    #[test]
    fn opaque_type_alias_preserves_opaque_flag_and_private_visibility() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let rhs = type_tree(&mut arena);
        let alias = arena.alloc(Tree {
            kind: TreeKind::TypeDef(TypeDef {
                name: TypeName::new(store.names.intern("OpaqueAlias")),
                rhs,
                metadata: Modifiers {
                    visibility: Some(VisibilitySyntax::Private { qualifier: None }),
                    modifiers: vec![Modifier::Opaque],
                    ..Modifiers::default()
                },
                variance: None,
            }),
            position: None,
            ty: (),
        });
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![alias], None);
        let root = package_with_stat(&mut arena, &mut store, "aliases", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 45, &mut store, &mut packages).unwrap();
        let alias_symbol = index.symbol_at(SourceId::from_index(45), alias).unwrap();

        assert_eq!(store.symbols.get(alias_symbol).kind, SymbolKind::TypeAlias);
        assert_eq!(store.symbols.get(alias_symbol).flags, SymbolFlags::OPAQUE);
        assert_eq!(
            store.symbols.get(alias_symbol).visibility,
            Visibility::Private
        );
    }

    #[test]
    fn method_type_and_value_parameters_are_owned_and_scoped() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(34);
        let type_parameter = type_parameter(&mut arena, &mut store, "A", None);
        let first_value = value_parameter(&mut arena, &mut store, "x", vec![], None);
        let second_value = value_parameter(&mut arena, &mut store, "y", vec![], None);
        let method = method_definition(
            &mut arena,
            &mut store,
            "convert",
            vec![type_parameter],
            vec![vec![first_value], vec![second_value]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "methods", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 34, &mut store, &mut packages).unwrap();
        let package = packages.get(&["methods"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let method_symbol = term_symbol(&mut store, class_scope, "convert").unwrap();
        let method_scope = index.scope_of(method_symbol).unwrap();
        let type_symbol = type_symbol(&mut store, method_scope, "A").unwrap();
        let x_symbol = term_symbol(&mut store, method_scope, "x").unwrap();
        let y_symbol = term_symbol(&mut store, method_scope, "y").unwrap();

        assert_eq!(
            store.symbols.get(type_symbol).kind,
            SymbolKind::TypeParameter
        );
        assert_eq!(store.symbols.get(type_symbol).owner, Some(method_symbol));
        assert_eq!(store.symbols.get(x_symbol).kind, SymbolKind::Parameter);
        assert_eq!(store.symbols.get(y_symbol).kind, SymbolKind::Parameter);
        assert_eq!(store.symbols.get(type_symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(x_symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(y_symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(x_symbol).owner, Some(method_symbol));
        assert_eq!(store.symbols.get(y_symbol).owner, Some(method_symbol));
        assert_eq!(store.symbols.get(type_symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(store.symbols.get(x_symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(store.symbols.get(y_symbol).flags, SymbolFlags::EMPTY);
        assert_eq!(term_symbol(&mut store, class_scope, "x"), None);
        assert_eq!(term_symbol(&mut store, class_scope, "y"), None);
        assert_eq!(index.symbol_at(source, type_parameter), Some(type_symbol));
        assert_eq!(index.symbol_at(source, first_value), Some(x_symbol));
        assert_eq!(index.symbol_at(source, second_value), Some(y_symbol));
    }

    #[test]
    fn contextual_method_parameter_preserves_implicit_flag() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "evidence",
            vec![Modifier::Implicit],
            None,
        );
        let method = method_definition(
            &mut arena,
            &mut store,
            "show",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "contextual", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 54, &mut store, &mut packages).unwrap();
        let package = packages.get(&["contextual"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let method_symbol = term_symbol(&mut store, class_scope, "show").unwrap();
        let method_scope = index.scope_of(method_symbol).unwrap();
        let parameter_symbol = term_symbol(&mut store, method_scope, "evidence").unwrap();

        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).flags,
            SymbolFlags::IMPLICIT
        );
    }

    #[test]
    fn erased_method_parameter_preserves_erased_flag() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "token",
            vec![Modifier::Erased],
            None,
        );
        let method = method_definition(
            &mut arena,
            &mut store,
            "run",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "erased", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 55, &mut store, &mut packages).unwrap();
        let package = packages.get(&["erased"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let method_symbol = term_symbol(&mut store, class_scope, "run").unwrap();
        let method_scope = index.scope_of(method_symbol).unwrap();
        let parameter_symbol = term_symbol(&mut store, method_scope, "token").unwrap();

        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).flags,
            SymbolFlags::ERASED
        );
    }

    #[test]
    fn method_with_a_non_type_type_parameter_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let malformed = ident(&mut arena, &mut store, "A");
        let method = method_definition(
            &mut arena,
            &mut store,
            "convert",
            vec![malformed],
            vec![],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "methods", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 35, &mut store, &mut packages).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: malformed.index(),
                expected: "TypeDef method type parameter",
            }
        );
    }

    #[test]
    fn method_with_a_non_val_value_parameter_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let malformed = ident(&mut arena, &mut store, "x");
        let method = method_definition(
            &mut arena,
            &mut store,
            "convert",
            vec![],
            vec![vec![malformed]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "methods", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 36, &mut store, &mut packages).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: malformed.index(),
                expected: "ValDef method value parameter",
            }
        );
    }

    #[test]
    fn method_constructor_role_marker_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor],
            None,
        );
        let method = method_definition(
            &mut arena,
            &mut store,
            "convert",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "methods", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 37, &mut store, &mut packages).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: parameter.index(),
                expected: "ordinary method ValDef without constructor-role metadata",
            }
        );
    }

    #[test]
    fn method_private_local_role_marker_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::PrivateLocal],
            None,
        );
        let method = method_definition(
            &mut arena,
            &mut store,
            "convert",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "methods", vec![class]);

        assert_eq!(
            name_package(&arena, root, 40, &mut store, &mut Packages::new()).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: parameter.index(),
                expected: "ordinary method ValDef without constructor-role metadata",
            }
        );
    }

    #[test]
    fn primary_constructor_in_template_body_is_not_entered_twice() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let (class, primary) = class_definition_with_header(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![],
            None,
            vec![],
            vec![],
            None,
        );
        let TreeKind::TypeDef(definition) = &arena.get(class).kind else {
            unreachable!("class_definition constructs a TypeDef");
        };
        let TreeKind::Template(template) = &mut arena.get_mut(definition.rhs).kind else {
            unreachable!("class_definition constructs a Template");
        };
        template.body.push(primary);
        let root = package_with_stat(&mut arena, &mut store, "constructors", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 33, &mut store, &mut packages).unwrap();
        let package = packages.get(&["constructors"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let init = dotty_core::TermName::new(store.names.intern("<init>"));

        assert_eq!(
            store
                .scopes
                .get(class_scope)
                .lookup_all(init.as_name())
                .len(),
            1
        );
        assert!(index.symbol_at(SourceId::from_index(33), primary).is_some());
    }

    #[test]
    fn secondary_constructor_member_uses_constructor_kind() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(41);
        let secondary = method_definition(&mut arena, &mut store, "<init>", vec![], vec![], None);
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![secondary], None);
        let root = package_with_stat(&mut arena, &mut store, "constructors", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 41, &mut store, &mut packages).unwrap();
        let package = packages.get(&["constructors"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let constructor_symbol = index.symbol_at(source, secondary).unwrap();
        let TreeKind::TypeDef(class_definition) = &arena.get(class).kind else {
            unreachable!("class_definition constructs a TypeDef");
        };
        let TreeKind::Template(template) = &arena.get(class_definition.rhs).kind else {
            unreachable!("class_definition constructs a Template");
        };
        let primary_symbol = index.symbol_at(source, template.constructor).unwrap();
        let init = dotty_core::TermName::new(store.names.intern("<init>"));

        assert_eq!(
            store.symbols.get(constructor_symbol).kind,
            SymbolKind::Constructor
        );
        assert_eq!(
            store.symbols.get(constructor_symbol).owner,
            Some(class_symbol)
        );
        assert_eq!(
            store.scopes.get(class_scope).lookup_all(init.as_name()),
            &[primary_symbol, constructor_symbol]
        );
    }

    #[test]
    fn secondary_constructor_parameters_are_owned_and_scoped_by_constructor() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(56);
        let parameter = value_parameter(&mut arena, &mut store, "x", vec![], None);
        let secondary = method_definition(
            &mut arena,
            &mut store,
            "<init>",
            vec![],
            vec![vec![parameter]],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![secondary], None);
        let root = package_with_stat(&mut arena, &mut store, "constructors", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 56, &mut store, &mut packages).unwrap();
        let package = packages.get(&["constructors"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "C").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let constructor_symbol = index.symbol_at(source, secondary).unwrap();
        let constructor_scope = index.scope_of(constructor_symbol).unwrap();
        let parameter_symbol = index.symbol_at(source, parameter).unwrap();

        assert_eq!(
            store.symbols.get(constructor_symbol).kind,
            SymbolKind::Constructor
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(constructor_symbol)
        );
        assert_eq!(
            store
                .scopes
                .get(constructor_scope)
                .lookup_all(&store.symbols.get(parameter_symbol).name),
            &[parameter_symbol]
        );
        assert!(
            store
                .scopes
                .get(class_scope)
                .lookup_all(&store.symbols.get(parameter_symbol).name)
                .is_empty()
        );
    }

    #[test]
    fn private_val_constructor_parameter_keeps_private_visibility() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "secret",
            vec![Modifier::ParamAccessor],
            None,
        );
        let TreeKind::ValDef(definition) = &mut arena.get_mut(parameter).kind else {
            unreachable!("value_parameter constructs a ValDef");
        };
        definition.metadata.visibility = Some(VisibilitySyntax::Private { qualifier: None });
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "vals", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 29, &mut store, &mut packages).unwrap();
        let symbol = index
            .symbol_at(SourceId::from_index(29), parameter)
            .unwrap();

        assert_eq!(store.symbols.get(symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(symbol).visibility, Visibility::Private);
    }

    #[test]
    fn var_constructor_parameter_is_a_mutable_field() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "count",
            vec![Modifier::ParamAccessor, Modifier::Var],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "Counter",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "vars", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 21, &mut store, &mut packages).unwrap();
        let package = packages.get(&["vars"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "Counter").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let field_symbol = term_symbol(&mut store, class_scope, "count").unwrap();

        assert_eq!(store.symbols.get(field_symbol).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(field_symbol).flags, SymbolFlags::MUTABLE);
    }

    #[test]
    fn case_class_later_private_local_parameter_is_scoped_but_not_a_field() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let first = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor],
            None,
        );
        let later = value_parameter(
            &mut arena,
            &mut store,
            "y",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "Pair",
            vec![Modifier::Case],
            vec![],
            None,
            vec![],
            vec![vec![first], vec![later]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "cases", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 22, &mut store, &mut packages).unwrap();
        let package = packages.get(&["cases"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "Pair").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let first_symbol = index.symbol_at(SourceId::from_index(22), first).unwrap();
        let later_symbol = index.symbol_at(SourceId::from_index(22), later).unwrap();

        assert_eq!(store.symbols.get(first_symbol).kind, SymbolKind::Field);
        assert_eq!(
            term_symbol(&mut store, class_scope, "x"),
            Some(first_symbol)
        );
        assert_eq!(store.symbols.get(later_symbol).kind, SymbolKind::Parameter);
        assert_eq!(
            term_symbol(&mut store, class_scope, "y"),
            Some(later_symbol)
        );
    }

    #[test]
    fn constructor_parameter_source_position_is_preserved() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(28);
        let position = Some(SourceSpan::new(
            source,
            Span::without_point(TextRange::new(12, 20).unwrap()),
        ));
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "located",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            position,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "LocatedParameter",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "positions", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 28, &mut store, &mut packages).unwrap();
        let parameter_symbol = index.symbol_at(source, parameter).unwrap();

        assert_eq!(store.symbols.get(parameter_symbol).position, position);
    }

    #[test]
    fn multiple_constructor_clauses_keep_every_parameter_identity() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let first = value_parameter(
            &mut arena,
            &mut store,
            "a",
            vec![Modifier::ParamAccessor],
            None,
        );
        let second = value_parameter(
            &mut arena,
            &mut store,
            "b",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "Many",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![first], vec![second]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "clauses", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 23, &mut store, &mut packages).unwrap();

        assert!(index.symbol_at(SourceId::from_index(23), first).is_some());
        assert!(index.symbol_at(SourceId::from_index(23), second).is_some());
    }

    #[test]
    fn primary_constructor_is_owned_mapped_and_entered_under_init() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let constructor_position = Some(SourceSpan::new(
            SourceId::from_index(24),
            Span::without_point(TextRange::new(2, 10).unwrap()),
        ));
        let (class, constructor) = class_definition_with_header(
            &mut arena,
            &mut store,
            "WithConstructor",
            vec![],
            vec![],
            None,
            vec![],
            vec![],
            constructor_position,
        );
        let root = package_with_stat(&mut arena, &mut store, "constructors", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 24, &mut store, &mut packages).unwrap();
        let package = packages.get(&["constructors"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "WithConstructor").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let constructor_symbol = term_symbol(&mut store, class_scope, "<init>").unwrap();

        assert_eq!(
            store.symbols.get(constructor_symbol).kind,
            SymbolKind::Constructor
        );
        assert_eq!(
            store.symbols.get(constructor_symbol).owner,
            Some(class_symbol)
        );
        assert_eq!(
            store.symbols.get(constructor_symbol).position,
            constructor_position
        );
        assert_eq!(
            store.symbols.get(constructor_symbol).info,
            SymbolInfo::Missing
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(24), constructor),
            Some(constructor_symbol)
        );
    }

    #[test]
    fn malformed_primary_constructor_shape_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let invalid_constructor = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let template = arena.alloc(Tree {
            kind: TreeKind::Template(Template {
                constructor: invalid_constructor,
                parents: vec![],
                self_val: None,
                body: vec![],
                metadata: UntypedTemplateMetadata::default(),
            }),
            position: None,
            ty: (),
        });
        let class_name = TypeName::new(store.names.intern("Broken"));
        let class = arena.alloc(Tree {
            kind: TreeKind::TypeDef(TypeDef {
                name: class_name,
                rhs: template,
                metadata: Modifiers::default(),
                variance: None,
            }),
            position: None,
            ty: (),
        });
        let root = package_with_stat(&mut arena, &mut store, "malformed", vec![class]);

        assert_eq!(
            name_package(&arena, root, 25, &mut store, &mut Packages::new()).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: invalid_constructor.index(),
                expected: "DefDef primary constructor",
            }
        );
    }

    #[test]
    fn constructor_parameter_without_param_accessor_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(&mut arena, &mut store, "uncategorized", vec![], None);
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "MalformedParameter",
            vec![],
            vec![],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "malformedparams", vec![class]);

        assert_eq!(
            name_package(&arena, root, 26, &mut store, &mut Packages::new()).unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: parameter.index(),
                expected: "constructor ValDef with ParamAccessor metadata",
            }
        );
    }

    #[test]
    fn malformed_package_name_shape_is_rejected() {
        let mut arena = AstArena::<Untyped>::new();
        let name = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let root = package(&mut arena, name, vec![]);

        assert_eq!(
            name_package(
                &arena,
                root,
                7,
                &mut SemanticStore::new(),
                &mut Packages::new(),
            )
            .unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: name.index(),
                expected: "Ident or qualified Select package name",
            }
        );
    }

    #[test]
    fn failed_indexing_rolls_back_packages_symbols_scopes_and_scope_entries() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        packages.enter(&mut store, SymbolOrigin::Builtin, &["existing"]);
        let before = store.checkpoint();

        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "Leaked", vec![], vec![], None);
        let nested_name = ident(&mut arena, &mut store, "created");
        let nested_package = package(&mut arena, nested_name, vec![]);
        let malformed_name = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let malformed_package = package(&mut arena, malformed_name, vec![]);
        let root = package_with_stat(
            &mut arena,
            &mut store,
            "existing",
            vec![class, nested_package, malformed_package],
        );
        let existing = packages.get(&["existing"]).unwrap();

        assert!(matches!(
            name_package(&arena, root, 22, &mut store, &mut packages),
            Err(NamerError::MalformedAstShape { .. })
        ));

        assert_eq!(store.checkpoint(), before);
        assert_eq!(packages.get(&["existing"]).unwrap(), existing);
        assert!(packages.get(&["existing", "created"]).is_none());
        let leaked_name = TypeName::new(store.names.intern("Leaked"));
        assert!(
            store
                .scopes
                .get(existing.scope)
                .lookup_all(leaked_name.as_name())
                .is_empty()
        );
    }
}
