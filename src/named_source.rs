//! One session-bound compilation unit after successful source naming.
use crate::core::{Packages, SemanticStore, SourceId, SourceSemanticIndex};
use crate::namer::{
    NamerDiagnostic, NamerError, NamingOutcome, SourceParseStatus,
    name_compilation_unit_with_diagnostics,
};
use crate::parser::ParseResult;
use crate::{ParsedSource, SyntaxDiagnostic};

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
                syntax,
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
    syntax: ParseResult,
    index: Box<SourceSemanticIndex>,
    diagnostics: Vec<NamerDiagnostic>,
}

impl NamedSource {
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
