//! One session-bound compilation unit after successful source naming.
use crate::core::{Packages, SemanticStore, SourceId, SourceSemanticIndex};
use crate::namer::{
    NamerDiagnostic, NamerError, NamingOutcome, SourceParseStatus,
    name_compilation_unit_with_diagnostics,
};
use crate::parser::ParseResult;
use crate::source_declarations::{
    DeclarationDiagnostic, DeclarationError, SourceClass, read_source_classes, source_classes,
};
use crate::{ParsedSource, SourceFileCalls, SyntaxDiagnostic};
use std::sync::{Arc, OnceLock};

/// Typed outcome of a file that could not enter naming successfully.
#[derive(Debug)]
pub enum NameSourceError {
    /// Syntax diagnostics prevented naming. Includes the original recovered AST.
    Syntax(Box<ParsedSource<SyntaxDiagnostic>>),
    /// A confirmed source error. Diagnostics are owned and survive namer rollback.
    Rejected(Vec<NamerDiagnostic>),
    /// A structural naming failure, with the originating file ID.
    Namer {
        /// Source of the attempted compilation unit.
        source: SourceId,
        /// Original failure returned by the namer.
        error: NamerError,
    },
}

impl std::fmt::Display for NameSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(parsed) => write!(
                f,
                "source has {} syntax diagnostics",
                parsed.diagnostics.len()
            ),
            Self::Rejected(issues) => write!(
                f,
                "naming rejected source with {} diagnostics",
                issues.len()
            ),
            Self::Namer { error, .. } => error.fmt(f),
        }
    }
}
impl std::error::Error for NameSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Namer { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl ParsedSource<SyntaxDiagnostic> {
    /// Names the intact result of [`crate::DetachedSource::bind`] in the store
    /// whose interner was used for binding. Supply the original file name
    /// including its extension. A file with syntax diagnostics keeps its AST
    /// and never enters naming. Failed naming rolls back its symbol, scope
    /// and package changes. Names interned before this call remain valid.
    pub fn name(
        self,
        filename: &str,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> Result<NamedSource, NameSourceError> {
        if !self.diagnostics.is_empty() {
            return Err(NameSourceError::Syntax(Box::new(self)));
        }
        let syntax = self.syntax;
        let source = syntax.call_names.source();
        match name_compilation_unit_with_diagnostics(
            &syntax.ast,
            syntax.root,
            source,
            filename,
            SourceParseStatus::Clean,
            store,
            packages,
        ) {
            Ok(NamingOutcome::Named { index, diagnostics }) => Ok(NamedSource {
                syntax: Arc::new(syntax),
                index,
                diagnostics,
            }),
            Ok(NamingOutcome::Rejected { diagnostics }) => {
                Err(NameSourceError::Rejected(diagnostics))
            }
            Err(error) => Err(NameSourceError::Namer { source, error }),
        }
    }
}

/// The original parse and the semantic index built from its exact tree and source.
/// Symbol and name IDs belong to the store used to bind and name this file.
#[derive(Debug)]
pub struct NamedSource {
    syntax: Arc<ParseResult>,
    index: Box<SourceSemanticIndex>,
    diagnostics: Vec<NamerDiagnostic>,
}

/// An ordinary-class projection of one named file, in source declaration order.
#[derive(Debug)]
pub struct NamedClassSource {
    named: NamedSource,
    classes: Vec<SourceClass>,
    navigation: OnceLock<Result<SourceFileCalls, DeclarationError>>,
}

impl NamedClassSource {
    /// Prepares direct-call navigation on first use and reuses the same index on
    /// subsequent queries. Shares the original parsed tokens and AST. Signature
    /// completion and class planning can skip this operation entirely.
    /// Inconsistent bindings or positions yield [`DeclarationError::InvalidBinding`].
    pub fn navigation(&self) -> Result<&SourceFileCalls, DeclarationError> {
        self.navigation
            .get_or_init(|| {
                SourceFileCalls::from_parse(&self.named.syntax, self.named.index(), self.classes())
            })
            .as_ref()
            .map_err(|e| *e)
    }

    /// File ID used by the typer for the declarations in this view.
    pub fn source(&self) -> SourceId {
        self.named.source()
    }

    /// Original untyped tree arena for this view's class, field and method IDs.
    pub fn ast(&self) -> &crate::core::AstArena<crate::core::Untyped> {
        &self.named.syntax.ast
    }

    /// Semantic bindings corresponding to [`Self::ast`].
    pub fn index(&self) -> &SourceSemanticIndex {
        self.named.index()
    }

    /// Original syntax and naming bindings used to build this projection.
    pub fn named(&self) -> &NamedSource {
        &self.named
    }

    /// Top-level ordinary classes in this unit and their canonical members.
    pub fn classes(&self) -> &[SourceClass] {
        &self.classes
    }
}

/// Complete ordinary classes and the declarations omitted from the same named file.
#[derive(Debug)]
#[must_use = "inspect rejections before treating the file as completely projected"]
pub struct ClassProjection {
    /// Original named file with its supported classes in source order. This can
    /// be empty even when naming has committed declarations to the store.
    pub file: NamedClassSource,
    /// One diagnostic per omitted top-level statement in source order, at its
    /// first unsupported tree. A rejected class contributes no members.
    pub rejections: Vec<DeclarationDiagnostic>,
}

/// A rejected ordinary-class projection. Naming has already committed successfully.
#[derive(Debug)]
pub struct ClassProjectionError {
    /// Original typed rejection and its declaration tree.
    pub error: DeclarationError,
    /// Complete named file, still usable through its original index and syntax.
    pub named: Box<NamedSource>,
}

impl std::fmt::Display for ClassProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ClassProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl NamedSource {
    /// Projects ordinary top-level classes with the same session's store, using
    /// [`source_classes`]. On rejection the error returns this entire named unit.
    /// Projection reads the store and preserves all committed naming state.
    pub fn into_classes(
        self,
        store: &SemanticStore,
    ) -> Result<NamedClassSource, ClassProjectionError> {
        let classes = source_classes(
            &self.syntax.ast,
            self.syntax.root,
            self.source(),
            &self.index,
            store,
        );
        self.with_classes(classes)
    }

    /// Projects complete ordinary classes and retains one typed rejection for
    /// each unsupported top-level declaration. An unsupported member excludes
    /// its entire enclosing class. Supply the store used for naming.
    ///
    /// A rejected package root or an encountered invalid binding returns the
    /// original named file in [`ClassProjectionError`], without a partial view.
    /// Projection leaves the store and naming index intact, including the symbols
    /// of omitted declarations. Inclusion by itself does not establish that
    /// a class's dependencies can be typed or emitted.
    pub fn project_classes(
        self,
        store: &SemanticStore,
    ) -> Result<ClassProjection, ClassProjectionError> {
        let mut rejections = Vec::new();
        let classes = read_source_classes(
            &self.syntax.ast,
            self.syntax.root,
            self.source(),
            &self.index,
            store,
            |error| {
                rejections.push(error.diagnostic(&self.syntax.ast));
                Ok(())
            },
        );
        Ok(ClassProjection {
            file: self.with_classes(classes)?,
            rejections,
        })
    }

    fn with_classes(
        self,
        classes: Result<Vec<SourceClass>, DeclarationError>,
    ) -> Result<NamedClassSource, ClassProjectionError> {
        match classes {
            Ok(classes) => Ok(NamedClassSource {
                named: self,
                classes,
                navigation: OnceLock::new(),
            }),
            Err(error) => Err(ClassProjectionError {
                error,
                named: Box::new(self),
            }),
        }
    }

    /// Source ID captured by the parser, including for a file without calls.
    pub fn source(&self) -> SourceId {
        self.syntax.call_names.source()
    }

    /// Original arena, root, call-name tokens and effective parser features.
    pub fn syntax(&self) -> &ParseResult {
        &self.syntax
    }

    /// Naming bindings for this file's original tree IDs.
    pub fn index(&self) -> &SourceSemanticIndex {
        &self.index
    }

    /// Diagnostics committed by the namer on success, in emission order.
    pub fn diagnostics(&self) -> &[NamerDiagnostic] {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Packages, SemanticStore, TreeKind};

    mod projection;

    #[test]
    fn definition_query_preserves_a_malformed_file_index_error() {
        let mut store = SemanticStore::new();
        let source = SourceId::from_index(3);
        let mut file = crate::parse_source(source, "class A; class B", Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Both.scala", &mut store, &mut Packages::new())
            .unwrap()
            .into_classes(&store)
            .unwrap();
        file.classes.reverse();
        let sources = crate::NamedClassSources::new([&file], &store).unwrap();
        assert_eq!(
            sources.definition_at(source, 0, &Default::default(), &Default::default(), &store),
            Err(DeclarationError::InvalidBinding {
                source,
                tree: file.classes[1].tree
            })
        );
    }

    #[test]
    fn navigation_is_lazy_and_shares_original_overload_name_tokens() {
        let text = "class A { def take(n: Int): Int = n; def take(b: Boolean): Int = 7 }";
        let mut store = SemanticStore::new();
        let unit = crate::parse_source(SourceId::from_index(2), text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("A.scala", &mut store, &mut Packages::new())
            .unwrap()
            .into_classes(&store)
            .unwrap();
        assert!(unit.navigation.get().is_none());
        let calls = unit.navigation().unwrap();
        assert!(std::ptr::eq(calls, unit.navigation().unwrap()));
        assert!(std::ptr::eq(
            calls.call_names(),
            &unit.named().syntax().call_names
        ));
        for (m, offset) in unit.classes()[0]
            .methods()
            .zip([text.find("take").unwrap(), text.rfind("take").unwrap()])
        {
            let span = calls.call_names().method_name(m.tree).unwrap();
            assert_eq!(span.start() as usize, offset);
            assert_eq!(span.end() as usize, offset + 4);
        }
    }

    #[test]
    fn trait_rejection_returns_the_named_file_and_exact_declaration_site() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let source = SourceId::from_index(9);
        let name = |store: &mut SemanticStore, packages: &mut Packages, source, text: &str| {
            crate::parse_source(source, text, Default::default())
                .unwrap()
                .bind(&mut store.names)
                .name("Api.scala", store, packages)
                .unwrap()
        };
        let text = "package demo\ntrait T";
        let unit = name(&mut store, &mut packages, source, text);
        let checkpoint = store.checkpoint();
        let failure = unit.into_classes(&store).unwrap_err();
        let DeclarationError::Unsupported { tree, source: id } = failure.error else {
            panic!("expected unsupported trait")
        };
        assert_eq!(id, source);
        let named = failure.named;
        let symbol = named.index().symbol_at(source, tree).unwrap();
        assert_eq!(
            store.symbols.get(symbol).kind,
            crate::core::SymbolKind::Trait
        );
        assert_eq!(store.checkpoint(), checkpoint);
        let span = failure.error.location(&named.syntax().ast).unwrap();
        let range = span.span().range();
        assert_eq!(
            &text[range.start() as usize..range.end() as usize],
            "trait T"
        );
        let mut other = name(
            &mut store,
            &mut packages,
            SourceId::from_index(10),
            "package demo\nclass U",
        );
        assert_eq!(failure.error.location(&other.syntax().ast), None);
        other.index = Box::default();
        assert!(matches!(
            other.into_classes(&store).unwrap_err().error,
            DeclarationError::InvalidBinding { .. }
        ));
    }
    #[test]
    fn syntax_and_naming_rejections_preserve_their_payloads_and_session() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let source = SourceId::from_index(9);
        let broken = "package demo\nclass Outer { private[Missing] val hidden: Int 1 }";
        let fixed_syntax = broken.replace("Int 1", "Int = 1");
        for (text, site) in [(broken, "1"), (&fixed_syntax, "Missing")] {
            let parsed = crate::parse_source(source, text, Default::default())
                .unwrap()
                .bind(&mut store.names);
            let checkpoint = store.checkpoint();
            let span = match parsed
                .name("Outer.scala", &mut store, &mut packages)
                .unwrap_err()
            {
                NameSourceError::Syntax(recovered) => {
                    assert!(matches!(
                        recovered.syntax.ast.get(recovered.syntax.root).kind,
                        TreeKind::PackageDef(_)
                    ));
                    let [SyntaxDiagnostic::Parser(d)] = recovered.diagnostics.as_slice() else {
                        panic!("expected parser issue")
                    };
                    assert_eq!(d.issue().code(), "parser.declaration.expected_value_equals");
                    recovered.diagnostics[0].location()
                }
                NameSourceError::Rejected(issues) => {
                    let [d] = issues.as_slice() else {
                        panic!("expected naming issue")
                    };
                    assert!(
                        matches!(d.issue(), crate::namer::NamerIssue::InvalidVisibilityQualifier { qualifier, .. } if qualifier == "Missing")
                    );
                    crate::SourceDiagnostic::from_namer(d).location.unwrap()
                }
                other => panic!("{other:?}"),
            };
            let r = span.span().range();
            assert_eq!(span.source(), source);
            assert_eq!(&text[r.start() as usize..r.end() as usize], site);
            assert_eq!(store.checkpoint(), checkpoint);
            assert!(packages.get(&["demo"]).is_none());
        }
        let valid = fixed_syntax.replace("[Missing]", "");
        let unit = crate::parse_source(source, &valid, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Outer.scala", &mut store, &mut packages)
            .unwrap();
        assert!(unit.diagnostics().is_empty());
        assert!(packages.get(&["demo"]).is_some());
    }

    #[test]
    fn structural_namer_failures_keep_the_typed_error() {
        let mut store = SemanticStore::new();
        let source = SourceId::from_index(3);
        let mut parsed = crate::parse_source(source, "class A", Default::default())
            .unwrap()
            .bind(&mut store.names);
        let tree = parsed
            .syntax
            .ast
            .iter()
            .find(|(_, n)| matches!(n.kind, TreeKind::TypeDef(_)))
            .unwrap()
            .0;
        parsed.syntax.root = tree;
        assert!(
            matches!(parsed.name("A.scala",&mut store,&mut Packages::new()),
            Err(NameSourceError::Namer {source:id, error:NamerError::RootIsNotPackage{tree_index}}) if id == source && tree_index == tree.index())
        );
    }
    #[test]
    fn same_slot_fields_retain_their_source_class_and_symbol() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let mut fields = Vec::new();
        for (i, name) in [(3, "A"), (7, "B")] {
            let source = SourceId::from_index(i);
            let unit = crate::parse_source(
                source,
                &format!("class {name} {{ val seed = 1 }}"),
                Default::default(),
            )
            .unwrap()
            .bind(&mut store.names)
            .name(&format!("{name}.scala"), &mut store, &mut packages)
            .unwrap();
            assert_eq!(unit.source(), source);
            let (tree, _) = unit
                .syntax()
                .ast
                .iter()
                .find(|(_, n)| matches!(n.kind, TreeKind::ValDef(_)))
                .unwrap();
            let symbol = unit.index().symbol_at(unit.source(), tree).unwrap();
            let class = store.symbols.owner(symbol).unwrap();
            assert_eq!(
                store.names.resolve(store.symbols.get(class).name.text()),
                name
            );
            assert_eq!(store.symbols.get(symbol).position.unwrap().source(), source);
            fields.push((tree, symbol));
        }
        assert_eq!(fields[0].0, fields[1].0);
        assert_ne!(fields[0].1, fields[1].1);
    }
}
