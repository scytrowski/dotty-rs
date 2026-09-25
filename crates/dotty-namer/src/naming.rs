//! Compilation-unit naming entry point and traversal seam.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use dotty_core::ast::{Modifier, Modifiers, Select, UntypedNode, VisibilitySyntax};
use dotty_core::{
    AstArena, Packages, Scope, ScopeId, SemanticStore, SourceId, SourceSpan, Symbol, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, TreeId, TreeKind, TypeName,
    Untyped, Visibility,
};

use crate::SourceSemanticIndex;

/// Internal structural error encountered while indexing a source tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamerError {
    /// A compilation-unit root must be a package definition.
    RootIsNotPackage { tree_index: u32 },
    /// A definition tree was assigned more than one semantic symbol.
    DuplicateSourceTreeSymbol { source: SourceId, tree_index: u32 },
    /// A source tree was assigned more than one derived symbol for one owner.
    DuplicateDerivedSourceTreeSymbol {
        owner: SymbolId,
        source: SourceId,
        tree_index: u32,
    },
    /// A semantic owner was assigned more than one declaration scope.
    DuplicateDeclarationScope { symbol: SymbolId },
    /// A tree did not have the shape required by a naming routine.
    MalformedAstShape {
        tree_index: u32,
        expected: &'static str,
    },
    /// A qualified source visibility does not name an enclosing access boundary.
    InvalidVisibilityQualifier {
        tree_index: u32,
        position: Option<SourceSpan>,
        qualifier: dotty_core::Name,
        protected: bool,
    },
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
            Self::DuplicateDerivedSourceTreeSymbol {
                owner,
                source,
                tree_index,
            } => write!(
                f,
                "source {} tree {tree_index} already has a derived semantic symbol for owner {}",
                source.index(),
                owner.index()
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
            Self::InvalidVisibilityQualifier {
                tree_index,
                protected,
                ..
            } => {
                let visibility = if *protected { "protected" } else { "private" };
                write!(
                    f,
                    "tree {tree_index} has an invalid qualified {visibility} visibility"
                )
            }
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

struct PackageStatPartition {
    top_stats: Vec<TreeId<Untyped>>,
    wrapped_stats: Vec<TreeId<Untyped>>,
}

#[derive(Clone, Copy)]
struct SymbolSpec {
    kind: SymbolKind,
    flags: SymbolFlags,
    visibility: Visibility,
}

#[derive(Clone, Copy)]
struct SourceModifierMapping {
    flags: SymbolFlags,
    visibility: Visibility,
}

/// A declaration identity entered before scanning its nested declarations.
enum EnteredHeader {
    Package {
        tree: TreeId<Untyped>,
        context: NamingContext,
    },
    SourcePackageWrapper {
        stats: Vec<TreeId<Untyped>>,
        context: NamingContext,
        package_symbol: SymbolId,
    },
    ClassLike {
        tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
        package_path: Vec<String>,
    },
    ModuleClass {
        tree: TreeId<Untyped>,
        template: TreeId<Untyped>,
        object_symbol: SymbolId,
        enclosing_scope: ScopeId,
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
/// The pass enters or reuses package symbols and scopes, partitions package
/// declarations, and places supported top-level values, methods, aliases,
/// pattern binders, and extension methods in synthetic source package
/// wrappers. Ordinary classes and objects remain package members. The
/// returned index maps supported source symbol-producing trees to their semantic symbols and
/// scopes; synthetic wrapper identities have no source-tree mapping. These
/// symbols remain incomplete until later compiler phases provide their
/// semantic information.
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
    let (result, scope_insertions, companion_links) = {
        let mut namer = Namer {
            arena,
            source,
            source_file_name,
            store,
            packages,
            root,
            index: SourceSemanticIndex::new(),
            scope_insertions: Vec::new(),
            companion_links: Vec::new(),
            package_private_boundary: None,
            source_package_wrappers: HashMap::new(),
        };
        let result = namer.index(root).map(|()| namer.index);
        (result, namer.scope_insertions, namer.companion_links)
    };
    match result {
        Ok(index) => {
            for (class, object) in companion_links {
                store.symbols.get_mut(class).links.companion = Some(object);
                store.symbols.get_mut(object).links.companion = Some(class);
            }
            Ok(index)
        }
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
    source_file_name: &'a str,
    store: &'a mut SemanticStore,
    packages: &'a mut Packages,
    root: TreeId<Untyped>,
    index: SourceSemanticIndex,
    scope_insertions: Vec<(ScopeId, SymbolId)>,
    companion_links: Vec<(SymbolId, SymbolId)>,
    package_private_boundary: Option<(SymbolId, SymbolId)>,
    source_package_wrappers: HashMap<SymbolId, (SymbolId, ScopeId)>,
}

impl Namer<'_> {
    fn index(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        self.expand(tree, &[], true)
    }

    /// Source expansion hook. Package headers perform their stats partition
    /// and create the synthetic wrapper before entering any declarations.
    fn expand(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<(), NamerError> {
        self.index_expanded(tree, enclosing_package, source_root)
    }

    fn partition_package_stats(
        &self,
        stats: &[TreeId<Untyped>],
        package_path: &[String],
    ) -> Result<PackageStatPartition, NamerError> {
        let wrapped_type_names = self.wrapped_type_names_for_package_path(package_path)?;

        let mut top_stats = Vec::new();
        let mut wrapped_stats = Vec::new();
        for tree in stats {
            let wrapped = match &self.arena.get(*tree).kind {
                TreeKind::PackageDef(_) => false,
                TreeKind::ValDef(_) | TreeKind::DefDef(_) | TreeKind::Export(_) => true,
                TreeKind::TypeDef(definition) => {
                    let class_like =
                        matches!(self.arena.get(definition.rhs).kind, TreeKind::Template(_));
                    let given_or_implicit =
                        definition.metadata.modifiers.iter().any(|modifier| {
                            matches!(modifier, Modifier::Given | Modifier::Implicit)
                        });
                    !class_like || given_or_implicit
                }
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                    let given_or_implicit =
                        definition.metadata.modifiers.iter().any(|modifier| {
                            matches!(modifier, Modifier::Given | Modifier::Implicit)
                        });
                    given_or_implicit
                        || wrapped_type_names
                            .contains(self.store.names.resolve(definition.name.as_name().text()))
                }
                TreeKind::PhaseSpecific(
                    UntypedNode::PatDef(_) | UntypedNode::ExtensionMethods(_),
                ) => true,
                TreeKind::Import(_) => continue,
                _ => continue,
            };
            if wrapped {
                wrapped_stats.push(*tree);
            } else {
                top_stats.push(*tree);
            }
        }
        Ok(PackageStatPartition {
            top_stats,
            wrapped_stats,
        })
    }

    fn wrapped_type_names_for_package_path(
        &self,
        package_path: &[String],
    ) -> Result<HashSet<String>, NamerError> {
        fn collect(
            namer: &Namer<'_>,
            tree: TreeId<Untyped>,
            enclosing_package: &[String],
            source_root: bool,
            package_path: &[String],
            names: &mut HashSet<String>,
        ) -> Result<(), NamerError> {
            let TreeKind::PackageDef(package) = &namer.arena.get(tree).kind else {
                return Ok(());
            };
            let segments = if source_root && namer.is_empty_package_sentinel(package.name) {
                Vec::new()
            } else {
                namer.flatten_package_name(package.name)?
            };
            let mut current_path = enclosing_package.to_vec();
            current_path.extend(segments);

            if current_path == package_path {
                for stat in &package.stats {
                    if let TreeKind::TypeDef(definition) = &namer.arena.get(*stat).kind {
                        let class_like =
                            matches!(namer.arena.get(definition.rhs).kind, TreeKind::Template(_));
                        let given_or_implicit =
                            definition.metadata.modifiers.iter().any(|modifier| {
                                matches!(modifier, Modifier::Given | Modifier::Implicit)
                            });
                        if !class_like || given_or_implicit {
                            names.insert(
                                namer
                                    .store
                                    .names
                                    .resolve(definition.name.as_name().text())
                                    .to_owned(),
                            );
                        }
                    }
                }
            }

            for stat in &package.stats {
                if matches!(namer.arena.get(*stat).kind, TreeKind::PackageDef(_)) {
                    collect(namer, *stat, &current_path, false, package_path, names)?;
                }
            }
            Ok(())
        }

        let mut names = HashSet::new();
        collect(self, self.root, &[], true, package_path, &mut names)?;
        Ok(names)
    }

    fn enter_source_package_wrapper(
        &mut self,
        package_context: &NamingContext,
        stats: Vec<TreeId<Untyped>>,
    ) -> Result<EnteredHeader, NamerError> {
        if let Some((module_class, scope)) = self
            .source_package_wrappers
            .get(&package_context.owner)
            .copied()
        {
            return Ok(EnteredHeader::SourcePackageWrapper {
                stats,
                context: NamingContext {
                    owner: module_class,
                    scope,
                    package_path: package_context.package_path.clone(),
                },
                package_symbol: package_context.owner,
            });
        }
        let stem = self
            .source_file_name
            .rsplit_once('.')
            .map_or(self.source_file_name, |(stem, _)| stem);
        let object_text = format!("{stem}$package");
        let module_class_text = format!("{object_text}$");
        let object_name =
            *dotty_core::TermName::new(self.store.names.intern(&object_text)).as_name();
        let module_class_name =
            *TypeName::new(self.store.names.intern(&module_class_text)).as_name();
        let object_symbol = self.store.symbols.alloc(Symbol {
            name: object_name,
            owner: Some(package_context.owner),
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.store
            .scopes
            .get_mut(package_context.scope)
            .enter(object_name, object_symbol);
        self.scope_insertions
            .push((package_context.scope, object_symbol));

        let module_class = self.store.symbols.alloc(Symbol {
            name: module_class_name,
            owner: Some(package_context.owner),
            kind: SymbolKind::ModuleClass,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.store
            .scopes
            .get_mut(package_context.scope)
            .enter(module_class_name, module_class);
        self.scope_insertions
            .push((package_context.scope, module_class));
        let scope = self.store.scopes.alloc(Scope::new(Some(module_class)));
        self.index.record_scope(module_class, scope)?;
        self.source_package_wrappers
            .insert(package_context.owner, (module_class, scope));

        Ok(EnteredHeader::SourcePackageWrapper {
            stats,
            context: NamingContext {
                owner: module_class,
                scope,
                package_path: package_context.package_path.clone(),
            },
            package_symbol: package_context.owner,
        })
    }

    fn scan_source_package_wrapper(
        &mut self,
        stats: &[TreeId<Untyped>],
        context: &NamingContext,
        package_symbol: SymbolId,
    ) -> Result<(), NamerError> {
        let previous_boundary = self.package_private_boundary;
        self.package_private_boundary = Some((context.owner, package_symbol));
        let mut headers = Vec::new();
        for tree in stats {
            match self.enter_wrapped_stat_headers(*tree, context) {
                Ok(entered) => headers.extend(entered),
                Err(error) => {
                    self.package_private_boundary = previous_boundary;
                    return Err(error);
                }
            }
        }
        for header in headers {
            if let Err(error) = self.scan_entered_header(header) {
                self.package_private_boundary = previous_boundary;
                return Err(error);
            }
        }
        self.package_private_boundary = previous_boundary;
        Ok(())
    }

    fn enter_wrapped_stat_headers(
        &mut self,
        tree: TreeId<Untyped>,
        context: &NamingContext,
    ) -> Result<Vec<EnteredHeader>, NamerError> {
        match &self.arena.get(tree).kind {
            TreeKind::TypeDef(definition) => {
                if matches!(self.arena.get(definition.rhs).kind, TreeKind::Template(_)) {
                    Ok(self
                        .enter_class_or_trait_header(tree, context)?
                        .into_iter()
                        .collect())
                } else {
                    let definition = definition.clone();
                    let spec = self.source_symbol_spec(
                        tree,
                        &definition.metadata,
                        context.owner,
                        SymbolKind::TypeAlias,
                    )?;
                    let symbol = self.enter_symbol(
                        tree,
                        *definition.name.as_name(),
                        context.owner,
                        context.scope,
                        spec,
                    )?;
                    Ok(vec![EnteredHeader::TypeAlias { tree, symbol }])
                }
            }
            TreeKind::ValDef(definition) => {
                let definition = definition.clone();
                let spec = self.source_symbol_spec(
                    tree,
                    &definition.metadata,
                    context.owner,
                    SymbolKind::Field,
                )?;
                let symbol = self.enter_symbol(
                    tree,
                    *definition.name.as_name(),
                    context.owner,
                    context.scope,
                    spec,
                )?;
                Ok(vec![EnteredHeader::Field { tree, symbol }])
            }
            TreeKind::DefDef(definition) => self
                .enter_method_header(tree, definition, context.owner, context.scope)
                .map(|header| vec![header]),
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => Ok(self
                .enter_module_header(tree, context)?
                .into_iter()
                .collect()),
            // These raw source forms are assigned to the package wrapper by
            // partitioning. Their lowered declarations are handled by later
            // naming work, so they do not introduce identities here.
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                let definition = definition.clone();
                let mut headers = Vec::new();
                for (binding_tree, binding_name) in
                    self.collect_pattern_bindings(&definition.patterns)
                {
                    let name = *dotty_core::TermName::new(binding_name.text()).as_name();
                    let spec = self.source_symbol_spec(
                        tree,
                        &definition.modifiers,
                        context.owner,
                        SymbolKind::Field,
                    )?;
                    let symbol =
                        self.enter_symbol(binding_tree, name, context.owner, context.scope, spec)?;
                    headers.push(EnteredHeader::Field {
                        tree: binding_tree,
                        symbol,
                    });
                }
                Ok(headers)
            }
            TreeKind::Export(_) => Ok(Vec::new()),
            TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => {
                let mut headers = Vec::new();
                for method in &extension.methods {
                    let TreeKind::DefDef(definition) = &self.arena.get(*method).kind else {
                        return Err(NamerError::MalformedAstShape {
                            tree_index: method.index(),
                            expected: "DefDef extension method",
                        });
                    };
                    headers.push(self.enter_method_header(
                        *method,
                        definition,
                        context.owner,
                        context.scope,
                    )?);
                }
                Ok(headers)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn collect_pattern_bindings(
        &self,
        patterns: &[TreeId<Untyped>],
    ) -> Vec<(TreeId<Untyped>, dotty_core::Name)> {
        let mut bindings = Vec::new();
        let mut names = HashSet::new();
        let mut pending = patterns.iter().rev().copied().collect::<Vec<_>>();
        while let Some(tree) = pending.pop() {
            match &self.arena.get(tree).kind {
                TreeKind::Ident(ident) => {
                    let text = self.store.names.resolve(ident.name.text());
                    if text != "_" && names.insert(text.to_owned()) {
                        bindings.push((tree, ident.name));
                    }
                }
                TreeKind::Bind(binding) => {
                    let text = self.store.names.resolve(binding.name.text());
                    if text != "_" && names.insert(text.to_owned()) {
                        bindings.push((tree, binding.name));
                    }
                    pending.push(binding.body);
                }
                TreeKind::Typed(typed) => pending.push(typed.expr),
                TreeKind::Apply(application) => {
                    pending.extend(application.args.iter().rev().copied());
                }
                TreeKind::Alternative(alternative) => {
                    if let Some(first) = alternative.alternatives.first() {
                        pending.push(*first);
                    }
                }
                TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
                TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                    pending.extend(tuple.elements.iter().rev().copied());
                }
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                    pending.push(infix.right);
                    pending.push(infix.left);
                }
                TreeKind::UnApply(unapply) => {
                    pending.extend(unapply.patterns.iter().rev().copied());
                }
                _ => {}
            }
        }
        bindings
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
        let mapped = self.map_source_modifiers(tree, &definition.metadata, owner_context.owner)?;
        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(owner_context.owner),
            kind,
            flags: mapped.flags,
            visibility: mapped.visibility,
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

    fn enter_module_header(
        &mut self,
        tree: TreeId<Untyped>,
        owner_context: &NamingContext,
    ) -> Result<Option<EnteredHeader>, NamerError> {
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) =
            &self.arena.get(tree).kind
        else {
            return Ok(None);
        };
        let definition = definition.clone();
        if !matches!(
            self.arena.get(definition.template).kind,
            TreeKind::Template(_)
        ) {
            return Err(NamerError::MalformedAstShape {
                tree_index: definition.template.index(),
                expected: "Template object body",
            });
        }

        let object_name = *definition.name.as_name();
        let module_class_text = format!("{}$", self.store.names.resolve(object_name.text()));
        let module_class_name =
            *TypeName::new(self.store.names.intern(&module_class_text)).as_name();
        let object_spec = self.source_symbol_spec(
            tree,
            &definition.metadata,
            owner_context.owner,
            SymbolKind::Object,
        )?;
        let module_class_spec = self.source_symbol_spec(
            tree,
            &definition.metadata,
            owner_context.owner,
            SymbolKind::ModuleClass,
        )?;

        let object_symbol = self.enter_symbol(
            tree,
            object_name,
            owner_context.owner,
            owner_context.scope,
            object_spec,
        )?;
        self.scope_insertions
            .push((owner_context.scope, object_symbol));
        let symbol = self.store.symbols.alloc(Symbol {
            name: module_class_name,
            owner: Some(owner_context.owner),
            kind: module_class_spec.kind,
            flags: module_class_spec.flags,
            visibility: module_class_spec.visibility,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: self.arena.get(tree).position,
            links: SymbolLinks::default(),
        });
        self.store
            .scopes
            .get_mut(owner_context.scope)
            .enter(module_class_name, symbol);
        self.scope_insertions.push((owner_context.scope, symbol));
        let scope = self.store.scopes.alloc(Scope::new(Some(symbol)));
        self.index.record_scope(symbol, scope)?;

        Ok(Some(EnteredHeader::ModuleClass {
            tree,
            template: definition.template,
            object_symbol,
            enclosing_scope: owner_context.scope,
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
                let partition =
                    self.partition_package_stats(&package.stats, &context.package_path)?;
                let mut headers = Vec::new();
                for stat in partition.top_stats {
                    match self.arena.get(stat).kind {
                        TreeKind::PackageDef(_) => {
                            if let Some(header) =
                                self.enter_package_header(stat, &context.package_path, false)?
                            {
                                headers.push(header);
                            }
                        }
                        TreeKind::TypeDef(_) => {
                            if let Some(header) =
                                self.enter_class_or_trait_header(stat, &context)?
                            {
                                headers.push(header);
                            }
                        }
                        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => {
                            if let Some(header) = self.enter_module_header(stat, &context)? {
                                headers.push(header);
                            }
                        }
                        _ => {}
                    }
                }
                if !partition.wrapped_stats.is_empty() {
                    headers.push(
                        self.enter_source_package_wrapper(&context, partition.wrapped_stats)?,
                    );
                }
                for header in headers {
                    self.scan_entered_header(header)?;
                }
                return Ok(());
            }
            EnteredHeader::SourcePackageWrapper {
                stats,
                context,
                package_symbol,
            } => return self.scan_source_package_wrapper(&stats, &context, package_symbol),
            EnteredHeader::ClassLike {
                tree,
                symbol,
                scope,
                package_path,
            } => (tree, symbol, scope, package_path),
            EnteredHeader::ModuleClass {
                tree,
                template,
                object_symbol,
                enclosing_scope,
                symbol,
                scope,
                package_path,
            } => {
                if let Some(class) = self.source_companion_class(tree, enclosing_scope) {
                    self.companion_links.push((class, object_symbol));
                }
                return self.scan_template_body(template, symbol, scope, package_path);
            }
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
        self.scan_template_body(definition.rhs, symbol, scope, package_path)
    }

    fn source_companion_class(
        &self,
        object_tree: TreeId<Untyped>,
        enclosing_scope: ScopeId,
    ) -> Option<SymbolId> {
        let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) =
            &self.arena.get(object_tree).kind
        else {
            return None;
        };
        let name = *TypeName::new(module.name.as_name().text()).as_name();
        let mut candidates = self
            .store
            .scopes
            .get(enclosing_scope)
            .lookup_all(&name)
            .iter()
            .copied()
            .filter(|symbol| {
                let symbol = self.store.symbols.get(*symbol);
                matches!(symbol.kind, SymbolKind::Class | SymbolKind::Trait)
                    && matches!(symbol.origin, SymbolOrigin::Source(_))
            });
        let candidate = candidates.next()?;
        candidates.next().is_none().then_some(candidate)
    }

    fn scan_template_body(
        &mut self,
        template_tree: TreeId<Untyped>,
        symbol: SymbolId,
        scope: ScopeId,
        package_path: Vec<String>,
    ) -> Result<(), NamerError> {
        let TreeKind::Template(template) = &self.arena.get(template_tree).kind else {
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
            let mut spec = self.source_symbol_spec(
                *parameter_tree,
                &parameter.metadata,
                symbol,
                SymbolKind::TypeParameter,
            )?;
            if parameter
                .metadata
                .modifiers
                .contains(&Modifier::PrivateLocal)
            {
                spec.visibility = Visibility::Private;
            }
            self.enter_symbol(
                *parameter_tree,
                *parameter.name.as_name(),
                symbol,
                scope,
                spec,
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
                let private_local = parameter
                    .metadata
                    .modifiers
                    .contains(&Modifier::PrivateLocal);
                let mut spec = self.source_symbol_spec(
                    *parameter_tree,
                    &parameter.metadata,
                    symbol,
                    if private_local {
                        SymbolKind::Parameter
                    } else {
                        SymbolKind::Field
                    },
                )?;
                if private_local {
                    spec.visibility = Visibility::Private;
                    self.record_unscoped_symbol(
                        *parameter_tree,
                        *parameter.name.as_name(),
                        symbol,
                        spec,
                    )?;
                } else {
                    self.enter_symbol(
                        *parameter_tree,
                        *parameter.name.as_name(),
                        symbol,
                        scope,
                        spec,
                    )?;
                }
            }
        }
        let constructor_spec = self.source_symbol_spec(
            template.constructor,
            &constructor.metadata,
            symbol,
            SymbolKind::Constructor,
        )?;
        let constructor_symbol = self.enter_symbol(
            template.constructor,
            *constructor.name.as_name(),
            symbol,
            scope,
            constructor_spec,
        )?;
        let constructor_scope = self
            .store
            .scopes
            .alloc(Scope::new(Some(constructor_symbol)));
        self.index
            .record_scope(constructor_symbol, constructor_scope)?;

        for parameter_tree in &constructor.type_params {
            let TreeKind::TypeDef(parameter) = &self.arena.get(*parameter_tree).kind else {
                return Err(NamerError::MalformedAstShape {
                    tree_index: parameter_tree.index(),
                    expected: "TypeDef class type parameter",
                });
            };
            let spec = self.source_symbol_spec(
                *parameter_tree,
                &parameter.metadata,
                constructor_symbol,
                SymbolKind::TypeParameter,
            )?;
            self.enter_derived_symbol(
                *parameter_tree,
                *parameter.name.as_name(),
                constructor_symbol,
                constructor_scope,
                spec,
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
                let mapped = self.map_source_modifiers(
                    *parameter_tree,
                    &parameter.metadata,
                    constructor_symbol,
                )?;
                let spec = SymbolSpec {
                    kind: SymbolKind::Parameter,
                    flags: mapped.flags
                        & (SymbolFlags::GIVEN | SymbolFlags::IMPLICIT | SymbolFlags::ERASED),
                    visibility: mapped.visibility,
                };
                self.enter_derived_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    constructor_symbol,
                    constructor_scope,
                    spec,
                )?;
            }
        }

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
                        let spec = self.source_symbol_spec(
                            *member,
                            &definition.metadata,
                            symbol,
                            SymbolKind::TypeAlias,
                        )?;
                        let alias = self.enter_symbol(
                            *member,
                            *definition.name.as_name(),
                            symbol,
                            scope,
                            spec,
                        )?;
                        nested_headers.push(EnteredHeader::TypeAlias {
                            tree: *member,
                            symbol: alias,
                        });
                    }
                }
                TreeKind::ValDef(definition) => {
                    let definition = definition.clone();
                    let spec = self.source_symbol_spec(
                        *member,
                        &definition.metadata,
                        symbol,
                        SymbolKind::Field,
                    )?;
                    let field = self.enter_symbol(
                        *member,
                        *definition.name.as_name(),
                        symbol,
                        scope,
                        spec,
                    )?;
                    nested_headers.push(EnteredHeader::Field {
                        tree: *member,
                        symbol: field,
                    });
                }
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => {
                    if let Some(header) = self.enter_module_header(*member, &class_context)? {
                        nested_headers.push(header);
                    }
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

        let spec =
            self.source_symbol_spec(tree, &definition.metadata, owner, SymbolKind::Method)?;
        let method =
            self.enter_symbol(tree, *definition.name.as_name(), owner, class_scope, spec)?;
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

        let spec =
            self.source_symbol_spec(tree, &definition.metadata, owner, SymbolKind::Constructor)?;
        let constructor =
            self.enter_symbol(tree, *definition.name.as_name(), owner, class_scope, spec)?;
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
            let spec = self.source_symbol_spec(
                *parameter_tree,
                &parameter.metadata,
                method,
                SymbolKind::TypeParameter,
            )?;
            self.enter_symbol(
                *parameter_tree,
                *parameter.name.as_name(),
                method,
                method_scope,
                spec,
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
                let spec = self.source_symbol_spec(
                    *parameter_tree,
                    &parameter.metadata,
                    method,
                    SymbolKind::Parameter,
                )?;
                self.enter_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    method,
                    method_scope,
                    spec,
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
                let spec = self.source_symbol_spec(
                    *parameter_tree,
                    &parameter.metadata,
                    constructor,
                    SymbolKind::Parameter,
                )?;
                self.enter_symbol(
                    *parameter_tree,
                    *parameter.name.as_name(),
                    constructor,
                    constructor_scope,
                    spec,
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
                    Modifier::Extension => SymbolFlags::EXTENSION,
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

    fn record_unscoped_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        name: dotty_core::Name,
        owner: SymbolId,
        spec: SymbolSpec,
    ) -> Result<SymbolId, NamerError> {
        let symbol = self.allocate_source_symbol(tree, name, owner, spec);
        self.index.record_symbol(self.source, tree, symbol)?;
        Ok(symbol)
    }

    fn allocate_source_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        name: dotty_core::Name,
        owner: SymbolId,
        spec: SymbolSpec,
    ) -> SymbolId {
        self.store.symbols.alloc(Symbol {
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
        })
    }

    fn enter_derived_symbol(
        &mut self,
        tree: TreeId<Untyped>,
        name: dotty_core::Name,
        owner: SymbolId,
        scope: ScopeId,
        spec: SymbolSpec,
    ) -> Result<SymbolId, NamerError> {
        let symbol = self.allocate_source_symbol(tree, name, owner, spec);
        self.store.scopes.get_mut(scope).enter(name, symbol);
        self.scope_insertions.push((scope, symbol));
        self.index
            .record_derived_symbol(owner, self.source, tree, symbol)?;
        Ok(symbol)
    }

    fn is_empty_package_sentinel(&self, name: TreeId<Untyped>) -> bool {
        matches!(
            &self.arena.get(name).kind,
            TreeKind::Ident(ident)
                if !ident.backquoted && self.store.names.resolve(ident.name.text()) == "<empty>"
        )
    }

    fn source_visibility_for_owner(
        &self,
        tree: TreeId<Untyped>,
        visibility: &Option<VisibilitySyntax>,
        owner: Option<SymbolId>,
    ) -> Result<Visibility, NamerError> {
        let supported_this_qualifier = |qualifier: Option<dotty_core::Name>| {
            qualifier.is_some_and(|name| self.store.names.resolve(name.text()) == "this")
        };
        let owner = self
            .package_private_boundary
            .filter(|(wrapper, _)| Some(*wrapper) == owner)
            .map_or(owner, |(_, package)| Some(package));
        match visibility {
            None => Ok(Visibility::Public),
            Some(VisibilitySyntax::Private { qualifier: None }) => Ok(owner
                .filter(|owner| self.store.symbols.get(*owner).kind == SymbolKind::Package)
                .map_or(Visibility::Private, Visibility::PrivateWithin)),
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }) if supported_this_qualifier(Some(*qualifier)) => Ok(Visibility::Private),
            Some(VisibilitySyntax::Protected { qualifier: None }) => Ok(Visibility::Protected),
            Some(VisibilitySyntax::Protected {
                qualifier: Some(qualifier),
            }) if supported_this_qualifier(Some(*qualifier)) => Ok(Visibility::Protected),
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }) => self
                .resolve_visibility_boundary(owner, *qualifier)
                .map(Visibility::PrivateWithin)
                .ok_or_else(|| self.invalid_visibility_qualifier(tree, *qualifier, false)),
            Some(VisibilitySyntax::Protected {
                qualifier: Some(qualifier),
            }) => self
                .resolve_visibility_boundary(owner, *qualifier)
                .map(Visibility::ProtectedWithin)
                .ok_or_else(|| self.invalid_visibility_qualifier(tree, *qualifier, true)),
        }
    }

    fn invalid_visibility_qualifier(
        &self,
        tree: TreeId<Untyped>,
        qualifier: dotty_core::Name,
        protected: bool,
    ) -> NamerError {
        NamerError::InvalidVisibilityQualifier {
            tree_index: tree.index(),
            position: self.arena.get(tree).position,
            qualifier,
            protected,
        }
    }

    /// Finds a qualified visibility boundary by walking semantic owners only.
    fn resolve_visibility_boundary(
        &self,
        mut owner: Option<SymbolId>,
        qualifier: dotty_core::Name,
    ) -> Option<SymbolId> {
        let qualifier = self.store.names.resolve(qualifier.text());
        while let Some(symbol) = owner {
            let current = self.store.symbols.get(symbol);
            let source_name = match current.kind {
                SymbolKind::Package => {
                    let name = self.store.names.resolve(current.name.text());
                    (!name.is_empty()).then_some(name)
                }
                SymbolKind::Class | SymbolKind::Trait => {
                    Some(self.store.names.resolve(current.name.text()))
                }
                SymbolKind::ModuleClass => self
                    .store
                    .names
                    .resolve(current.name.text())
                    .strip_suffix('$'),
                _ => None,
            };
            if source_name == Some(qualifier) {
                return Some(symbol);
            }
            owner = current.owner;
        }
        None
    }

    fn map_source_modifiers(
        &self,
        tree: TreeId<Untyped>,
        metadata: &Modifiers,
        owner: SymbolId,
    ) -> Result<SourceModifierMapping, NamerError> {
        Ok(SourceModifierMapping {
            flags: Self::source_flags(&metadata.modifiers),
            visibility: self.source_visibility_for_owner(
                tree,
                &metadata.visibility,
                Some(owner),
            )?,
        })
    }

    fn source_symbol_spec(
        &self,
        tree: TreeId<Untyped>,
        metadata: &Modifiers,
        owner: SymbolId,
        kind: SymbolKind,
    ) -> Result<SymbolSpec, NamerError> {
        let mapped = self.map_source_modifiers(tree, metadata, owner)?;
        Ok(SymbolSpec {
            kind,
            flags: mapped.flags,
            visibility: mapped.visibility,
        })
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
        DefDef, Export, Ident, Literal, Modifier, Modifiers, PackageDef, Template, TypeDef,
        TypeTree, UntypedTemplateMetadata, ValDef, VisibilitySyntax,
    };
    use dotty_core::{
        AstArena, Packages, SemanticStore, SourceId, SourceSpan, Span, SymbolFlags, SymbolInfo,
        SymbolKind, SymbolOrigin, TextRange, Tree, TreeKind, TypeName, Untyped, Visibility,
    };

    use super::*;

    #[test]
    fn each_source_flag_modifier_maps_to_its_semantic_flag() {
        let cases = [
            (Modifier::Abstract, SymbolFlags::ABSTRACT),
            (Modifier::Final, SymbolFlags::FINAL),
            (Modifier::Sealed, SymbolFlags::SEALED),
            (Modifier::Case, SymbolFlags::CASE),
            (Modifier::Implicit, SymbolFlags::IMPLICIT),
            (Modifier::Given, SymbolFlags::GIVEN),
            (Modifier::Lazy, SymbolFlags::LAZY),
            (Modifier::Var, SymbolFlags::MUTABLE),
            (Modifier::Override, SymbolFlags::OVERRIDE),
            (Modifier::Inline, SymbolFlags::INLINE),
            (Modifier::Transparent, SymbolFlags::TRANSPARENT),
            (Modifier::Opaque, SymbolFlags::OPAQUE),
            (Modifier::Extension, SymbolFlags::EXTENSION),
            (Modifier::Erased, SymbolFlags::ERASED),
        ];

        for (modifier, expected) in cases {
            assert_eq!(Namer::source_flags(&[modifier]), expected, "{modifier:?}");
        }
    }

    #[test]
    fn source_flag_mapping_uses_order_independent_union_semantics() {
        let forward = [Modifier::Abstract, Modifier::Inline, Modifier::Extension];
        let reverse = [Modifier::Extension, Modifier::Inline, Modifier::Abstract];

        assert_eq!(Namer::source_flags(&forward), Namer::source_flags(&reverse));
        assert_eq!(
            Namer::source_flags(&forward),
            SymbolFlags::ABSTRACT | SymbolFlags::INLINE | SymbolFlags::EXTENSION
        );
    }

    #[test]
    fn parser_role_markers_do_not_become_semantic_flags() {
        let role_markers = [
            Modifier::Trait,
            Modifier::Enum,
            Modifier::EnumCase,
            Modifier::ParamAccessor,
            Modifier::Param,
            Modifier::PrivateLocal,
        ];

        for marker in role_markers {
            assert!(Namer::source_flags(&[marker]).is_empty(), "{marker:?}");
        }
    }

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

    fn module_definition(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        name: &str,
        modifiers: Vec<Modifier>,
        visibility: Option<VisibilitySyntax>,
        body: Vec<TreeId<Untyped>>,
        position: Option<SourceSpan>,
    ) -> (TreeId<Untyped>, TreeId<Untyped>, TreeId<Untyped>) {
        let constructor = constructor(arena, store, vec![], vec![], None);
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
        let module = arena.alloc(Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::ModuleDef(dotty_core::ast::ModuleDef {
                name: dotty_core::TermName::new(name_id),
                template,
                metadata: Modifiers {
                    visibility,
                    modifiers,
                    ..Modifiers::default()
                },
            })),
            position,
            ty: (),
        });
        (module, template, constructor)
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

        let index = name_package(&arena, root, 23, &mut store, &mut packages).unwrap();
        let owner = packages.get(&["flags"]).unwrap();
        let lookup_scope = type_symbol(&mut store, owner.scope, "Example$package$")
            .and_then(|wrapper| index.scope_of(wrapper))
            .unwrap_or(owner.scope);
        let symbol = type_symbol(&mut store, lookup_scope, "Flagged").unwrap();
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

        assert_eq!(
            store.symbols.get(symbol).visibility,
            Visibility::PrivateWithin(owner.symbol)
        );
    }

    #[test]
    fn private_this_class_visibility_is_private_without_a_this_symbol() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let this_name = *dotty_core::TermName::new(store.names.intern("this")).as_name();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "ThisPrivateClass",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(this_name),
            }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 75, &mut store, &mut packages).unwrap();
        let package = packages.get(&["visibility"]).unwrap();
        let class_symbol = index.symbol_at(SourceId::from_index(75), class).unwrap();

        assert_eq!(
            store.symbols.get(class_symbol).visibility,
            Visibility::Private
        );
        assert_eq!(
            term_symbol(&mut store, package.scope, "this"),
            None,
            "the special qualifier is not resolved as a scope member"
        );
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
    fn protected_this_class_visibility_remains_protected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let this_name = *dotty_core::TermName::new(store.names.intern("this")).as_name();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "ThisProtectedClass",
            vec![],
            Some(VisibilitySyntax::Protected {
                qualifier: Some(this_name),
            }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 84, &mut store, &mut packages).unwrap();
        let symbol = index.symbol_at(SourceId::from_index(84), class).unwrap();

        assert_eq!(store.symbols.get(symbol).visibility, Visibility::Protected);
    }

    #[test]
    fn non_enclosing_qualified_class_visibility_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let source = SourceId::from_index(21);
        let position = Some(SourceSpan::new(
            source,
            Span::without_point(TextRange::new(8, 23).unwrap()),
        ));
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
            position,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 21, &mut store, &mut packages).unwrap_err(),
            NamerError::InvalidVisibilityQualifier {
                tree_index: class.index(),
                position,
                qualifier,
                protected: false,
            }
        );
        assert!(packages.get(&["visibility"]).is_none());
    }

    #[test]
    fn non_enclosing_qualified_protected_visibility_is_rejected() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *dotty_core::TermName::new(store.names.intern("Owner")).as_name();
        let class = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "QualifiedProtected",
            vec![],
            Some(VisibilitySyntax::Protected {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![class]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 76, &mut store, &mut packages).unwrap_err(),
            NamerError::InvalidVisibilityQualifier {
                tree_index: class.index(),
                position: None,
                qualifier,
                protected: true,
            }
        );
    }

    #[test]
    fn private_qualified_visibility_resolves_the_enclosing_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Outer")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let outer = class_definition(&mut arena, &mut store, "Outer", vec![], vec![member], None);
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 77, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(77);
        let outer_symbol = index.symbol_at(source, outer).unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(outer_symbol)
        );
    }

    #[test]
    fn private_qualified_visibility_walks_two_owners_outward() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Outer")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let middle = class_definition(&mut arena, &mut store, "Middle", vec![], vec![member], None);
        let outer = class_definition(&mut arena, &mut store, "Outer", vec![], vec![middle], None);
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 78, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(78);
        let outer_symbol = index.symbol_at(source, outer).unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(outer_symbol)
        );
    }

    #[test]
    fn protected_qualified_visibility_resolves_the_enclosing_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Outer")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Protected {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let outer = class_definition(&mut arena, &mut store, "Outer", vec![], vec![member], None);
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 79, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(79);
        let outer_symbol = index.symbol_at(source, outer).unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::ProtectedWithin(outer_symbol)
        );
    }

    #[test]
    fn qualified_visibility_selects_the_nearest_same_named_owner() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Repeat")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let inner = class_definition(&mut arena, &mut store, "Repeat", vec![], vec![member], None);
        let outer = class_definition(&mut arena, &mut store, "Repeat", vec![], vec![inner], None);
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![outer]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 80, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(80);
        let inner_symbol = index.symbol_at(source, inner).unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(inner_symbol)
        );
    }

    #[test]
    fn qualified_visibility_does_not_resolve_a_sibling_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Sibling")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let outer = class_definition(&mut arena, &mut store, "Outer", vec![], vec![member], None);
        let sibling = class_definition(&mut arena, &mut store, "Sibling", vec![], vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![outer, sibling]);
        let mut packages = Packages::new();

        assert_eq!(
            name_package(&arena, root, 81, &mut store, &mut packages).unwrap_err(),
            NamerError::InvalidVisibilityQualifier {
                tree_index: member.index(),
                position: None,
                qualifier,
                protected: false,
            }
        );
    }

    #[test]
    fn qualified_visibility_maps_object_name_to_module_class_boundary() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Obj")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let (object, _, _) = module_definition(
            &mut arena,
            &mut store,
            "Obj",
            vec![],
            None,
            vec![member],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![object]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 82, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(82);
        let package = packages.get(&["visibility"]).unwrap();
        let module_class = type_symbol(&mut store, package.scope, "Obj$").unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(
            store.symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(module_class)
        );
    }

    #[test]
    fn qualified_visibility_resolves_an_enclosing_package_segment() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("example")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let nested_name = ident(&mut arena, &mut store, "example");
        let nested_package = package(&mut arena, nested_name, vec![member]);
        let root = package_with_stat(&mut arena, &mut store, "com", vec![nested_package]);
        let mut packages = Packages::new();

        name_package(&arena, root, 83, &mut store, &mut packages).unwrap();
        let package = packages.get(&["com", "example"]).unwrap();
        let member_symbol = type_symbol(&mut store, package.scope, "Member").unwrap();

        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(package.symbol)
        );
    }

    #[test]
    fn qualified_visibility_resolves_an_enclosing_trait() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let qualifier = *TypeName::new(store.names.intern("Boundary")).as_name();
        let member = class_definition_with_visibility(
            &mut arena,
            &mut store,
            "Member",
            vec![],
            Some(VisibilitySyntax::Private {
                qualifier: Some(qualifier),
            }),
            vec![],
            None,
        );
        let boundary = class_definition(
            &mut arena,
            &mut store,
            "Boundary",
            vec![Modifier::Trait],
            vec![member],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "visibility", vec![boundary]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 85, &mut store, &mut packages).unwrap();
        let source = SourceId::from_index(85);
        let boundary_symbol = index.symbol_at(source, boundary).unwrap();
        let member_symbol = index.symbol_at(source, member).unwrap();

        assert_eq!(store.symbols.get(boundary_symbol).kind, SymbolKind::Trait);
        assert_eq!(
            store.symbols.get(member_symbol).visibility,
            Visibility::PrivateWithin(boundary_symbol)
        );
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
    fn nested_object_creates_object_and_module_class_identities() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let position = Some(SourceSpan::new(
            SourceId::from_index(77),
            Span::without_point(TextRange::new(10, 28).unwrap()),
        ));
        let (object, _, _) = module_definition(
            &mut arena,
            &mut store,
            "Foo",
            vec![Modifier::Final],
            None,
            vec![],
            position,
        );
        let class = class_definition(&mut arena, &mut store, "Outer", vec![], vec![object], None);
        let root = package_with_stat(&mut arena, &mut store, "objects", vec![class]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(77);
        let index = name_package(&arena, root, 77, &mut store, &mut packages).unwrap();
        let package = packages.get(&["objects"]).unwrap();
        let class_symbol = type_symbol(&mut store, package.scope, "Outer").unwrap();
        let class_scope = index.scope_of(class_symbol).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let module_class = type_symbol(&mut store, class_scope, "Foo$").unwrap();
        let object_name = dotty_core::TermName::new(store.names.intern("Foo"));

        assert_eq!(
            term_symbol(&mut store, class_scope, "Foo"),
            Some(object_symbol)
        );
        assert_eq!(store.symbols.get(object_symbol).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(module_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            store.symbols.get(object_symbol).name,
            *object_name.as_name()
        );
        assert_eq!(store.symbols.get(object_symbol).owner, Some(class_symbol));
        assert_eq!(store.symbols.get(module_class).owner, Some(class_symbol));
        assert_eq!(
            store.symbols.get(object_symbol).origin,
            SymbolOrigin::Source(source)
        );
        assert_eq!(
            store.symbols.get(module_class).origin,
            SymbolOrigin::Source(source)
        );
        assert_eq!(store.symbols.get(module_class).flags, SymbolFlags::FINAL);
        assert_eq!(store.symbols.get(object_symbol).position, position);
        assert_eq!(store.symbols.get(module_class).position, position);
        assert_eq!(store.symbols.get(object_symbol).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        assert_eq!(index.scope_of(object_symbol), None);
        assert!(index.scope_of(module_class).is_some());
        assert_eq!(
            store.symbols.get(object_symbol).links.companion,
            None,
            "the object term is not linked to its module class"
        );
        assert_eq!(store.symbols.get(module_class).links.companion, None);
    }

    #[test]
    fn nested_object_links_to_its_unique_source_class_companion() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "Foo", vec![], vec![], None);
        let (object, _, _) =
            module_definition(&mut arena, &mut store, "Foo", vec![], None, vec![], None);
        let outer = class_definition(
            &mut arena,
            &mut store,
            "Outer",
            vec![],
            vec![class, object],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "companions", vec![outer]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(81);
        let index = name_package(&arena, root, 81, &mut store, &mut packages).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let class_symbol = index.symbol_at(source, class).unwrap();
        let outer = store.symbols.get(class_symbol).owner.unwrap();
        let outer_scope = index.scope_of(outer).unwrap();
        let module_class = type_symbol(&mut store, outer_scope, "Foo$").unwrap();

        assert_eq!(
            store.symbols.get(class_symbol).links.companion,
            Some(object_symbol)
        );
        assert_eq!(
            store.symbols.get(object_symbol).links.companion,
            Some(class_symbol)
        );
        assert_eq!(store.symbols.get(module_class).links.companion, None);
    }

    #[test]
    fn nested_object_does_not_link_when_source_class_companion_is_ambiguous() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let first = class_definition(&mut arena, &mut store, "Foo", vec![], vec![], None);
        let second = class_definition(&mut arena, &mut store, "Foo", vec![], vec![], None);
        let (object, _, _) =
            module_definition(&mut arena, &mut store, "Foo", vec![], None, vec![], None);
        let outer = class_definition(
            &mut arena,
            &mut store,
            "Outer",
            vec![],
            vec![first, second, object],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "ambiguouscompanions", vec![outer]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(82);
        let index = name_package(&arena, root, 82, &mut store, &mut packages).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let first_symbol = index.symbol_at(source, first).unwrap();
        let second_symbol = index.symbol_at(source, second).unwrap();

        assert_eq!(store.symbols.get(first_symbol).links.companion, None);
        assert_eq!(store.symbols.get(second_symbol).links.companion, None);
        assert_eq!(store.symbols.get(object_symbol).links.companion, None);
    }

    #[test]
    fn nested_object_template_members_belong_to_the_module_class() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let method = method_definition(&mut arena, &mut store, "run", vec![], vec![], None);
        let (object, _, constructor) = module_definition(
            &mut arena,
            &mut store,
            "Worker",
            vec![],
            None,
            vec![method],
            None,
        );
        let class = class_definition(&mut arena, &mut store, "Outer", vec![], vec![object], None);
        let root = package_with_stat(&mut arena, &mut store, "objects", vec![class]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(78);
        let index = name_package(&arena, root, 78, &mut store, &mut packages).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let module_class = store
            .symbols
            .get(object_symbol)
            .owner
            .and_then(|outer| {
                let class_scope = index.scope_of(outer)?;
                type_symbol(&mut store, class_scope, "Worker$")
            })
            .unwrap();
        let module_scope = index.scope_of(module_class).unwrap();
        let method_symbol = index.symbol_at(source, method).unwrap();
        let constructor_symbol = index.symbol_at(source, constructor).unwrap();

        assert_eq!(store.symbols.get(method_symbol).owner, Some(module_class));
        assert_eq!(
            store.symbols.get(constructor_symbol).owner,
            Some(module_class)
        );
        assert_eq!(
            term_symbol(&mut store, module_scope, "run"),
            Some(method_symbol)
        );
        assert_eq!(store.scopes.get(module_scope).owner, Some(module_class));
    }

    #[test]
    fn nested_object_headers_precede_object_template_descendants() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let deep = class_definition(&mut arena, &mut store, "Deep", vec![], vec![], None);
        let (object, _, _) =
            module_definition(&mut arena, &mut store, "A", vec![], None, vec![deep], None);
        let sibling = class_definition(&mut arena, &mut store, "B", vec![], vec![], None);
        let outer = class_definition(
            &mut arena,
            &mut store,
            "Outer",
            vec![],
            vec![object, sibling],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "objects", vec![outer]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(79);
        let index = name_package(&arena, root, 79, &mut store, &mut packages).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let sibling_symbol = index.symbol_at(source, sibling).unwrap();
        let deep_symbol = index.symbol_at(source, deep).unwrap();
        let outer_symbol = store.symbols.get(object_symbol).owner.unwrap();
        let outer_scope = index.scope_of(outer_symbol).unwrap();
        let module_class = type_symbol(&mut store, outer_scope, "A$").unwrap();

        assert!(object_symbol.index() < sibling_symbol.index());
        assert!(module_class.index() < sibling_symbol.index());
        assert!(sibling_symbol.index() < deep_symbol.index());
    }

    #[test]
    fn ordinary_package_level_objects_are_owned_by_the_package() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let (object, _, _) = module_definition(
            &mut arena,
            &mut store,
            "TopLevel",
            vec![],
            None,
            vec![],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "objects", vec![object]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 80, &mut store, &mut packages).unwrap();
        let package = packages.get(&["objects"]).unwrap();

        let object_symbol = index.symbol_at(SourceId::from_index(80), object).unwrap();
        let module_class = type_symbol(&mut store, package.scope, "TopLevel$").unwrap();
        assert_eq!(
            term_symbol(&mut store, package.scope, "TopLevel"),
            Some(object_symbol)
        );
        assert_eq!(store.symbols.get(object_symbol).owner, Some(package.symbol));
        assert_eq!(store.symbols.get(module_class).owner, Some(package.symbol));
        assert!(index.scope_of(module_class).is_some());
    }

    #[test]
    fn no_source_package_wrapper_is_created_without_wrapped_stats() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(&mut arena, &mut store, "Direct", vec![], vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "direct", vec![class]);
        let mut packages = Packages::new();

        name_package(&arena, root, 92, &mut store, &mut packages).unwrap();
        let package = packages.get(&["direct"]).unwrap();

        assert_eq!(
            type_symbol(&mut store, package.scope, "Example$package$"),
            None
        );
        assert!(type_symbol(&mut store, package.scope, "Direct").is_some());
    }

    #[test]
    fn top_level_export_stat_causes_source_package_wrapper_creation() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let expr = ident(&mut arena, &mut store, "Api");
        let export = arena.alloc(Tree {
            kind: TreeKind::Export(Export {
                expr,
                selectors: vec![],
            }),
            position: None,
            ty: (),
        });
        let root = package_with_stat(&mut arena, &mut store, "exports", vec![export]);
        let mut packages = Packages::new();

        name_package(&arena, root, 93, &mut store, &mut packages).unwrap();
        let package = packages.get(&["exports"]).unwrap();

        assert!(type_symbol(&mut store, package.scope, "Example$package$").is_some());
    }

    #[test]
    fn given_class_companion_and_given_object_are_wrapped_with_top_level_members() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let class = class_definition(
            &mut arena,
            &mut store,
            "Foo",
            vec![Modifier::Given],
            vec![],
            None,
        );
        let (object, _, _) =
            module_definition(&mut arena, &mut store, "Foo", vec![], None, vec![], None);
        let (given_object, _, _) = module_definition(
            &mut arena,
            &mut store,
            "GivenOnly",
            vec![Modifier::Given],
            None,
            vec![],
            None,
        );
        let root = package_with_stat(
            &mut arena,
            &mut store,
            "givens",
            vec![class, object, given_object],
        );
        let mut packages = Packages::new();

        let source = SourceId::from_index(85);
        let index = name_compilation_unit(
            &arena,
            root,
            source,
            "Givens.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let package = packages.get(&["givens"]).unwrap();
        let wrapper = type_symbol(&mut store, package.scope, "Givens$package$").unwrap();
        let wrapper_scope = index.scope_of(wrapper).unwrap();
        let class_symbol = index.symbol_at(source, class).unwrap();
        let object_symbol = index.symbol_at(source, object).unwrap();
        let given_object_symbol = index.symbol_at(source, given_object).unwrap();
        let given_module_class = type_symbol(&mut store, wrapper_scope, "GivenOnly$").unwrap();

        assert_eq!(store.symbols.get(class_symbol).owner, Some(wrapper));
        assert_eq!(store.symbols.get(object_symbol).owner, Some(wrapper));
        assert_eq!(store.symbols.get(given_object_symbol).owner, Some(wrapper));
        assert_eq!(store.symbols.get(given_module_class).owner, Some(wrapper));
        assert_eq!(
            store.symbols.get(class_symbol).links.companion,
            Some(object_symbol)
        );
        assert_eq!(
            store.symbols.get(object_symbol).links.companion,
            Some(class_symbol)
        );
        assert!(
            store
                .symbols
                .get(class_symbol)
                .flags
                .contains(SymbolFlags::GIVEN)
        );
        assert!(
            store
                .symbols
                .get(given_object_symbol)
                .flags
                .contains(SymbolFlags::GIVEN)
        );
        assert_eq!(type_symbol(&mut store, package.scope, "Foo"), None);
        assert_eq!(term_symbol(&mut store, package.scope, "GivenOnly"), None);
    }

    #[test]
    fn top_level_method_is_owned_by_source_named_synthetic_wrapper() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let method = method_definition(&mut arena, &mut store, "f", vec![], vec![], None);
        let root = package_with_stat(&mut arena, &mut store, "wrapped", vec![method]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(83);
        let index =
            name_compilation_unit(&arena, root, source, "Foo.scala", &mut store, &mut packages)
                .unwrap();
        let package = packages.get(&["wrapped"]).unwrap();
        let wrapper = term_symbol(&mut store, package.scope, "Foo$package").unwrap();
        let wrapper_class = type_symbol(&mut store, package.scope, "Foo$package$").unwrap();
        let wrapper_scope = index.scope_of(wrapper_class).unwrap();
        let method_symbol = index.symbol_at(source, method).unwrap();

        assert_eq!(store.symbols.get(wrapper).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(wrapper_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(store.symbols.get(wrapper).owner, Some(package.symbol));
        assert_eq!(store.symbols.get(wrapper_class).owner, Some(package.symbol));
        assert_eq!(store.symbols.get(wrapper).origin, SymbolOrigin::Synthetic);
        assert_eq!(
            store.symbols.get(wrapper_class).origin,
            SymbolOrigin::Synthetic
        );
        assert_eq!(store.symbols.get(wrapper).position, None);
        assert_eq!(store.symbols.get(wrapper_class).position, None);
        assert_eq!(store.symbols.get(method_symbol).owner, Some(wrapper_class));
        assert_eq!(
            store.symbols.get(method_symbol).origin,
            SymbolOrigin::Source(source)
        );
        assert_eq!(term_symbol(&mut store, package.scope, "f"), None);
        assert_eq!(
            term_symbol(&mut store, wrapper_scope, "f"),
            Some(method_symbol)
        );
    }

    #[test]
    fn private_top_level_method_keeps_the_package_as_its_access_boundary() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let method = method_definition(&mut arena, &mut store, "secret", vec![], vec![], None);
        let TreeKind::DefDef(definition) = &mut arena.get_mut(method).kind else {
            unreachable!();
        };
        definition.metadata.visibility = Some(VisibilitySyntax::Private { qualifier: None });
        let root = package_with_stat(&mut arena, &mut store, "privatewrapped", vec![method]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(84);
        let index = name_compilation_unit(
            &arena,
            root,
            source,
            "Private.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let package = packages.get(&["privatewrapped"]).unwrap();
        let method_symbol = index.symbol_at(source, method).unwrap();

        assert_eq!(
            store.symbols.get(method_symbol).visibility,
            Visibility::PrivateWithin(package.symbol)
        );
    }

    #[test]
    fn repeated_package_clauses_in_one_source_share_the_source_wrapper() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let first_method = method_definition(&mut arena, &mut store, "first", vec![], vec![], None);
        let second_method =
            method_definition(&mut arena, &mut store, "second", vec![], vec![], None);
        let first_package =
            package_with_stat(&mut arena, &mut store, "repeated", vec![first_method]);
        let second_package =
            package_with_stat(&mut arena, &mut store, "repeated", vec![second_method]);
        let root_name = ident(&mut arena, &mut store, "<empty>");
        let root = package(&mut arena, root_name, vec![first_package, second_package]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(91);
        let index = name_compilation_unit(
            &arena,
            root,
            source,
            "Repeated.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let package = packages.get(&["repeated"]).unwrap();
        let wrapper = type_symbol(&mut store, package.scope, "Repeated$package$").unwrap();
        let first = index.symbol_at(source, first_method).unwrap();
        let second = index.symbol_at(source, second_method).unwrap();

        assert_eq!(store.symbols.get(first).owner, Some(wrapper));
        assert_eq!(store.symbols.get(second).owner, Some(wrapper));
        assert_eq!(
            store
                .scopes
                .get(package.scope)
                .lookup_all(TypeName::new(store.names.intern("Repeated$package$")).as_name())
                .len(),
            1
        );
    }

    #[test]
    fn repeated_package_clauses_partition_companions_together() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let given_type = class_definition(
            &mut arena,
            &mut store,
            "Shared",
            vec![Modifier::Given],
            vec![],
            None,
        );
        let (companion, _, _) =
            module_definition(&mut arena, &mut store, "Shared", vec![], None, vec![], None);
        let first_package = package_with_stat(&mut arena, &mut store, "repeated", vec![given_type]);
        let second_package = package_with_stat(&mut arena, &mut store, "repeated", vec![companion]);
        let root_name = ident(&mut arena, &mut store, "<empty>");
        let root = package(&mut arena, root_name, vec![first_package, second_package]);
        let mut packages = Packages::new();

        let source = SourceId::from_index(92);
        let index = name_compilation_unit(
            &arena,
            root,
            source,
            "Repeated.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let package = packages.get(&["repeated"]).unwrap();
        let wrapper = type_symbol(&mut store, package.scope, "Repeated$package$").unwrap();
        let given_symbol = index.symbol_at(source, given_type).unwrap();
        let companion_symbol = index.symbol_at(source, companion).unwrap();

        assert_eq!(store.symbols.get(given_symbol).owner, Some(wrapper));
        assert_eq!(store.symbols.get(companion_symbol).owner, Some(wrapper));
        assert_eq!(
            store.symbols.get(given_symbol).links.companion,
            Some(companion_symbol)
        );
        assert_eq!(
            store.symbols.get(companion_symbol).links.companion,
            Some(given_symbol)
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
    fn constructor_only_parameter_is_not_in_class_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            None,
        );
        let (class, constructor_tree) = class_definition_with_header(
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
        let constructor_symbol = index
            .symbol_at(SourceId::from_index(19), constructor_tree)
            .unwrap();
        let constructor_scope = index.scope_of(constructor_symbol).unwrap();
        let derived_parameter = index
            .derived_symbol_at(constructor_symbol, SourceId::from_index(19), parameter)
            .unwrap();

        assert_eq!(
            store.symbols.get(parameter_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(parameter_symbol).owner,
            Some(class_symbol)
        );
        assert_eq!(term_symbol(&mut store, class_scope, "x"), None);
        assert_eq!(
            store.symbols.get(parameter_symbol).visibility,
            Visibility::Private
        );
        assert_eq!(
            store.symbols.get(derived_parameter).owner,
            Some(constructor_symbol)
        );
        assert_eq!(
            term_symbol(&mut store, constructor_scope, "x"),
            Some(derived_parameter)
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
        definition.metadata.modifiers = vec![
            Modifier::Final,
            Modifier::Override,
            Modifier::Inline,
            Modifier::Extension,
        ];
        definition.metadata.visibility = Some(VisibilitySyntax::Private { qualifier: None });
        let class = class_definition(&mut arena, &mut store, "C", vec![], vec![method], None);
        let root = package_with_stat(&mut arena, &mut store, "members", vec![class]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 44, &mut store, &mut packages).unwrap();
        let method_symbol = index.symbol_at(SourceId::from_index(44), method).unwrap();

        assert_eq!(
            store.symbols.get(method_symbol).flags,
            SymbolFlags::FINAL
                | SymbolFlags::OVERRIDE
                | SymbolFlags::INLINE
                | SymbolFlags::EXTENSION
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
    fn case_class_later_private_local_parameter_is_unscoped_and_not_a_field() {
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
        let (class, constructor_tree) = class_definition_with_header(
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
        let constructor_symbol = index
            .symbol_at(SourceId::from_index(22), constructor_tree)
            .unwrap();
        let constructor_scope = index.scope_of(constructor_symbol).unwrap();
        let first_symbol = index.symbol_at(SourceId::from_index(22), first).unwrap();
        let later_symbol = index.symbol_at(SourceId::from_index(22), later).unwrap();
        let later_constructor_symbol = index
            .derived_symbol_at(constructor_symbol, SourceId::from_index(22), later)
            .unwrap();

        assert_eq!(store.symbols.get(first_symbol).kind, SymbolKind::Field);
        assert_eq!(
            term_symbol(&mut store, class_scope, "x"),
            Some(first_symbol)
        );
        assert_eq!(store.symbols.get(later_symbol).kind, SymbolKind::Parameter);
        assert_eq!(
            store.symbols.get(later_symbol).visibility,
            Visibility::Private
        );
        assert_eq!(term_symbol(&mut store, class_scope, "y"), None);
        assert_eq!(
            store.symbols.get(later_constructor_symbol).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            store.symbols.get(later_constructor_symbol).owner,
            Some(constructor_symbol)
        );
        assert_eq!(
            term_symbol(&mut store, constructor_scope, "y"),
            Some(later_constructor_symbol)
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
        let TreeKind::DefDef(definition) = &mut arena.get_mut(constructor).kind else {
            unreachable!("class_definition_with_header constructs a DefDef constructor");
        };
        definition.metadata.modifiers.push(Modifier::Erased);
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
            store.symbols.get(constructor_symbol).flags,
            SymbolFlags::ERASED
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

    #[test]
    fn failed_class_member_scan_rolls_back_constructor_parameter_copies() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let before = store.checkpoint();

        let mut arena = AstArena::<Untyped>::new();
        let parameter = value_parameter(
            &mut arena,
            &mut store,
            "x",
            vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
            None,
        );
        let malformed_parameter = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let method = method_definition(
            &mut arena,
            &mut store,
            "broken",
            vec![],
            vec![vec![malformed_parameter]],
            None,
        );
        let (class, _) = class_definition_with_header(
            &mut arena,
            &mut store,
            "C",
            vec![],
            vec![method],
            None,
            vec![],
            vec![vec![parameter]],
            None,
        );
        let root = package_with_stat(&mut arena, &mut store, "rollback", vec![class]);
        let source = SourceId::from_index(101);

        assert_eq!(
            name_compilation_unit(
                &arena,
                root,
                source,
                "Rollback.scala",
                &mut store,
                &mut packages,
            )
            .unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: malformed_parameter.index(),
                expected: "ValDef method value parameter",
            }
        );

        assert_eq!(store.checkpoint(), before);
        assert!(packages.is_empty());
    }

    #[test]
    fn failed_wrapped_stat_indexing_rolls_back_synthetic_wrapper_symbols() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        packages.enter(&mut store, SymbolOrigin::Builtin, &["existing"]);
        let before = store.checkpoint();

        let mut arena = AstArena::<Untyped>::new();
        let method = method_definition(&mut arena, &mut store, "f", vec![], vec![], None);
        let malformed_class = class_definition(
            &mut arena,
            &mut store,
            "GivenClass",
            vec![Modifier::Given],
            vec![],
            None,
        );
        let TreeKind::TypeDef(definition) = &arena.get(malformed_class).kind else {
            unreachable!();
        };
        let template = definition.rhs;
        let malformed_constructor = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let TreeKind::Template(template) = &mut arena.get_mut(template).kind else {
            unreachable!();
        };
        template.constructor = malformed_constructor;
        let root = package_with_stat(
            &mut arena,
            &mut store,
            "existing",
            vec![method, malformed_class],
        );
        let existing = packages.get(&["existing"]).unwrap();

        assert!(matches!(
            name_compilation_unit(
                &arena,
                root,
                SourceId::from_index(94),
                "Rollback.scala",
                &mut store,
                &mut packages,
            ),
            Err(NamerError::MalformedAstShape {
                expected: "DefDef primary constructor",
                ..
            })
        ));

        assert_eq!(store.checkpoint(), before);
        assert_eq!(packages.get(&["existing"]).unwrap(), existing);
        assert_eq!(
            term_symbol(&mut store, existing.scope, "Rollback$package"),
            None
        );
        assert_eq!(
            type_symbol(&mut store, existing.scope, "Rollback$package$"),
            None
        );
        assert_eq!(term_symbol(&mut store, existing.scope, "f"), None);
    }
}
