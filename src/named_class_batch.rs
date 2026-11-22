//! Ordered access to the named class sources in a caller-selected batch.
use std::{collections::HashMap, fmt};

use crate::core::{AstArena, Definitions, Packages, SymbolKind, Typed};
use crate::core::{SemanticStore, SourceId, SymbolId};
use crate::jvm_class_plan::ClassHeaderBindings;
use crate::source_declarations::SourceClass;
use crate::typer::{SignatureCompletion, TyperError};
use crate::{
    MemberSignatureError, NamedClassSource, SourceDeclaration, SourceDeclarationIndex,
    SourceDeclarationIndexError,
};

/// Failure to assemble a batch from named files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedClassBatchError {
    /// Two inputs have the same source ID, including files without declarations.
    DuplicateSource(SourceId),
    /// The session store and the supplied declarations have inconsistent bindings.
    Declaration(SourceDeclarationIndexError),
}

impl fmt::Display for NamedClassBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSource(source) => write!(f, "duplicate named source {source:?}"),
            Self::Declaration(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NamedClassBatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Declaration(error) => Some(error),
            Self::DuplicateSource(_) => None,
        }
    }
}

/// Borrows ordinary-class projections from a single naming session.
/// Files retain their input order, independent of the numeric source IDs.
#[derive(Debug)]
pub struct NamedClassBatch<'a> {
    declarations: SourceDeclarationIndex,
    files: Vec<&'a NamedClassSource>,
    by_source: HashMap<SourceId, &'a NamedClassSource>,
}

impl<'a> NamedClassBatch<'a> {
    /// Collects the supplied files in order. A repeated source ID rejects the
    /// whole batch. Validates declarations against their naming store and owns
    /// the resulting index, so the store remains available for mutable typing.
    /// The supplied files and store must belong to the same session: numeric
    /// symbol IDs alone cannot distinguish arenas. Navigation is left lazy.
    pub fn new(
        files: impl IntoIterator<Item = &'a NamedClassSource>,
        store: &SemanticStore,
    ) -> Result<Self, NamedClassBatchError> {
        let mut by_source = HashMap::new();
        let mut ordered = Vec::new();
        for file in files {
            if by_source.insert(file.source(), file).is_some() {
                return Err(NamedClassBatchError::DuplicateSource(file.source()));
            }
            ordered.push(file);
        }
        let declarations =
            SourceDeclarationIndex::from_classes(store, ordered.iter().flat_map(|f| f.classes()))
                .map_err(NamedClassBatchError::Declaration)?;
        Ok(Self {
            declarations,
            files: ordered,
            by_source,
        })
    }

    /// Iterates over the borrowed files in the order supplied at construction.
    pub fn files(&self) -> impl DoubleEndedIterator<Item = &'a NamedClassSource> + '_ {
        self.files.iter().copied()
    }

    /// Iterates by input file and then by class declaration order within each file.
    pub fn classes(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&'a NamedClassSource, &'a SourceClass)> + '_ {
        self.files()
            .flat_map(|file| file.classes().iter().map(move |class| (file, class)))
    }

    /// Returns the batch's validated index, including constructors and their fields.
    pub fn declarations(&self) -> &SourceDeclarationIndex {
        &self.declarations
    }

    /// Resolves a same-session symbol to its declaring file and canonical owner.
    /// A symbol outside this batch has no entry.
    pub fn declaration(
        &self,
        symbol: SymbolId,
    ) -> Option<(&'a NamedClassSource, SourceDeclaration)> {
        let declaration = self.declarations.get(symbol)?;
        Some((self.file(declaration.source)?, declaration))
    }

    /// Attempts one header using the declaring file and the classes in this batch
    /// as eligible dependencies. Prepare constructor signatures first. Reuse the
    /// same session store, typed arena and bindings on every attempt, including
    /// retries after a returned dependency completes.
    ///
    /// An unknown symbol yields `UnknownSymbol`; a known symbol outside the batch
    /// yields `SourceProvenanceMissing`. Typing failures retain their original
    /// source and leave prior bindings intact. See [`crate::try_complete_class_header`].
    pub fn try_complete_class_header(
        &self,
        class: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
        arena: &mut AstArena<Typed>,
        bindings: &mut ClassHeaderBindings,
    ) -> Result<SignatureCompletion, TyperError> {
        let Some((file, _)) = self.declaration(class) else {
            return Err(if store.symbols.contains(class) {
                TyperError::SourceProvenanceMissing { symbol: class }
            } else {
                TyperError::UnknownSymbol { symbol: class }
            });
        };
        crate::try_complete_class_header(
            class,
            file.ast(),
            file.index(),
            store,
            definitions,
            packages,
            arena,
            bindings,
            |dep| {
                self.declarations
                    .get(dep)
                    .is_some_and(|d| d.kind == SymbolKind::Class)
            },
        )
    }

    /// Attempts one field or method signature after header completion. The
    /// batch supplies its declaring AST and naming index. A dependency in the
    /// batch may return `BlockedOn`; missing external member types retain the
    /// typer error. The caller chooses when to retry. An unknown or ineligible
    /// root yields [`MemberSignatureError::InvalidMember`] before typing.
    pub fn try_complete_member_signature(
        &self,
        member: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
    ) -> Result<SignatureCompletion, MemberSignatureError> {
        let (file, _) = self
            .declaration(member)
            .ok_or(MemberSignatureError::InvalidMember(member))?;
        crate::try_complete_member_signature(
            member,
            &self.declarations,
            file.ast(),
            file.index(),
            store,
            definitions,
            packages,
        )
    }

    /// Finds the original named file by its session source ID, including empty files.
    pub fn file(&self, source: SourceId) -> Option<&'a NamedClassSource> {
        self.by_source.get(&source).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Packages, SemanticStore};

    fn named(
        id: u32,
        text: &str,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> NamedClassSource {
        crate::parse_source(SourceId::from_index(id), text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name(&format!("File{id}.scala"), store, packages)
            .unwrap()
            .into_classes(store)
            .unwrap()
    }

    #[test]
    fn sparse_sources_preserve_input_order_and_reject_repeated_empty_files() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let a = named(42, "class A", &mut store, &mut packages);
        let empty = named(7, "", &mut store, &mut packages);
        for files in [[&a, &empty], [&empty, &a]] {
            let batch = NamedClassBatch::new(files, &store).unwrap();
            assert_eq!(
                batch
                    .files()
                    .map(NamedClassSource::source)
                    .collect::<Vec<_>>(),
                files.map(NamedClassSource::source)
            );
            assert!(std::ptr::eq(batch.file(a.source()).unwrap(), &a));
            assert!(std::ptr::eq(batch.file(empty.source()).unwrap(), &empty));
            assert!(batch.file(SourceId::from_index(0)).is_none());
        }
        assert_eq!(
            NamedClassBatch::new([&empty, &a, &empty], &store).unwrap_err(),
            NamedClassBatchError::DuplicateSource(empty.source())
        );
    }
    #[test]
    fn same_named_fields_resolve_to_their_exact_classes_and_files() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let ab = named(
            42,
            "class A { val seed = 1 }; class B",
            &mut store,
            &mut packages,
        );
        let c = named(7, "class C { val seed = 7 }", &mut store, &mut packages);
        let omitted = named(3, "class D", &mut store, &mut packages);
        let batch = NamedClassBatch::new([&c, &ab], &store).unwrap();
        assert_eq!(
            batch
                .classes()
                .map(|(f, c)| (f.source(), c.symbol))
                .collect::<Vec<_>>(),
            [
                (c.source(), c.classes()[0].symbol),
                (ab.source(), ab.classes()[0].symbol),
                (ab.source(), ab.classes()[1].symbol)
            ]
        );
        for file in [&ab, &c] {
            let class = &file.classes()[0];
            let seed = class.fields().next().unwrap().symbol;
            let (owner_file, d) = batch.declaration(seed).unwrap();
            assert!(std::ptr::eq(owner_file, file));
            assert_eq!(
                (d.class, d.source, d.kind),
                (class.symbol, file.source(), crate::core::SymbolKind::Field)
            );
        }
        assert!(batch.declaration(omitted.classes()[0].symbol).is_none());
    }
    fn prepare_constructors(
        batch: &NamedClassBatch<'_>,
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
    ) {
        for (file, class) in batch.classes() {
            crate::typer::SourceTyper::new(
                file.ast(),
                file.source(),
                file.index(),
                store,
                defs,
                packages,
            )
            .complete_symbol(class.constructor)
            .unwrap();
        }
    }

    #[test]
    fn header_dependencies_route_to_the_parent_and_errors_to_the_child() {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let base = named(42, "class Base(seed: Int)", &mut store, &mut packages);
        let mid = named(
            7,
            "class Mid(start: Int) extends Base(start)",
            &mut store,
            &mut packages,
        );
        let text = "class Leaf(start: Boolean) extends Mid(start)";
        let leaf = named(21, text, &mut store, &mut packages);
        let batch = NamedClassBatch::new([&leaf, &mid, &base], &store).unwrap();
        prepare_constructors(&batch, &mut store, defs, &packages);
        let [a, b, c] = [&leaf, &mid, &base].map(|f| f.classes()[0].symbol);
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        let mut attempt = |id| {
            batch.try_complete_class_header(
                id,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut bindings,
            )
        };
        assert_eq!(attempt(a).unwrap(), SignatureCompletion::BlockedOn(b));
        assert_eq!(attempt(b).unwrap(), SignatureCompletion::BlockedOn(c));
        for id in [c, b] {
            assert!(matches!(attempt(id), Ok(SignatureCompletion::Complete(_))));
        }
        let error = attempt(a).unwrap_err();
        assert!(
            matches!(error, TyperError::ApplicationArgumentTypeMismatch { argument_index: 0, actual, expected, .. } if actual == defs.boolean && expected == defs.int)
        );
        let span =
            crate::typer_error_span_with(&error, |id| batch.file(id).map(NamedClassSource::ast))
                .unwrap();
        assert_eq!(span.source(), leaf.source());
        let range = span.span().range();
        assert_eq!(range.start() as usize, text.rfind("start").unwrap());
        assert_eq!(range.len(), 5);
    }
    #[test]
    fn member_attempts_preserve_dependencies_external_failures_and_bad_calls() {
        use crate::core::{SymbolInfo, Type};
        for reverse in [false, true] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let consumer = named(
                42,
                "class Consumer(p: Provider) { val answer = p.value(); val bad = p.value(1) }",
                &mut store,
                &mut packages,
            );
            let provider = named(
                7,
                "class Provider(val s: Seeds) { def value() = s.seed }",
                &mut store,
                &mut packages,
            );
            let seeds = named(
                3,
                "class Seeds { val seed = 40 }",
                &mut store,
                &mut packages,
            );
            let mut files = [&consumer, &provider, &seeds];
            if reverse {
                files.reverse();
            }
            let batch = NamedClassBatch::new(files, &store).unwrap();
            prepare_constructors(&batch, &mut store, defs, &packages);
            let mut arena = AstArena::new();
            let mut bindings = ClassHeaderBindings::default();
            for (_, class) in batch.classes() {
                assert!(matches!(
                    batch.try_complete_class_header(
                        class.symbol,
                        &mut store,
                        defs,
                        &packages,
                        &mut arena,
                        &mut bindings
                    ),
                    Ok(SignatureCompletion::Complete(_))
                ));
            }
            let mut fields = consumer.classes()[0].fields();
            let answer = fields.next().unwrap().symbol;
            let bad = fields.next().unwrap().symbol;
            let value = provider.classes()[0].methods().next().unwrap().symbol;
            let seed = seeds.classes()[0].fields().next().unwrap().symbol;
            let partial = NamedClassBatch::new([&provider], &store).unwrap();
            let checkpoint = store.checkpoint();
            assert!(
                matches!(partial.try_complete_member_signature(value,&mut store,defs,&packages),
                Err(MemberSignatureError::Typing { member, error: TyperError::MemberTypeUnavailable { symbol }, .. }) if member == value && symbol == seed)
            );
            assert!(
                matches!(partial.try_complete_member_signature(seed,&mut store,defs,&packages),
                Err(MemberSignatureError::InvalidMember(s)) if s == seed)
            );
            assert_eq!(store.checkpoint(), checkpoint);
            let mut attempt =
                |s| batch.try_complete_member_signature(s, &mut store, defs, &packages);
            assert_eq!(
                attempt(answer).unwrap(),
                SignatureCompletion::BlockedOn(value)
            );
            assert_eq!(
                attempt(value).unwrap(),
                SignatureCompletion::BlockedOn(seed)
            );
            for s in [seed, value, answer] {
                assert!(matches!(attempt(s), Ok(SignatureCompletion::Complete(_))));
            }
            assert!(
                matches!(attempt(bad), Err(MemberSignatureError::Typing { member, declaration, error: TyperError::ApplicationArityMismatch { expected: 0, actual: 1, .. } }) if member == bad && declaration.source == consumer.source())
            );
            for s in [seed, answer] {
                assert_eq!(store.symbols.info(s), &SymbolInfo::Complete(defs.int));
            }
            let SymbolInfo::Complete(ty) = *store.symbols.info(value) else {
                panic!("missing method signature")
            };
            assert!(matches!(store.types.get(ty),Type::Method(m) if m.result == defs.int));
        }
    }
}
