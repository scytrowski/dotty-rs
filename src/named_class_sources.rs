//! Ordered access to the named class sources selected by the caller.
use std::{collections::HashMap, fmt};

use crate::core::{AstArena, Definitions, Packages, SymbolKind, TypeId, Typed};
use crate::core::{SemanticStore, SourceId, SymbolId};
use crate::source_declarations::SourceClass;
use crate::typer::{SignatureCompletion, TyperError};
use crate::{ClassHeaderBindings, ClassHeaderError, ReportedTyperError};
use crate::{
    MemberSignatureError, NamedClassSource, SourceDeclaration, SourceDeclarationIndex,
    SourceDeclarationIndexError,
};

/// Failure to index the files supplied to [`NamedClassSources::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedClassSourcesError {
    /// Two inputs have the same source ID, including files without declarations.
    DuplicateSource(SourceId),
    /// The session store and the supplied declarations have inconsistent bindings.
    Declaration(SourceDeclarationIndexError),
}

impl fmt::Display for NamedClassSourcesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSource(source) => write!(f, "duplicate named source {source:?}"),
            Self::Declaration(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NamedClassSourcesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Declaration(error) => Some(error),
            Self::DuplicateSource(_) => None,
        }
    }
}

/// An ordered collection of borrowed [`NamedClassSource`] files from one naming session.
/// Files retain their input order, independent of the numeric source IDs.
#[derive(Debug)]
pub struct NamedClassSources<'a> {
    declarations: SourceDeclarationIndex,
    files: Vec<&'a NamedClassSource>,
    by_source: HashMap<SourceId, &'a NamedClassSource>,
    by_class: HashMap<SymbolId, (&'a NamedClassSource, &'a SourceClass)>,
}

impl<'a> NamedClassSources<'a> {
    /// Collects the supplied files in order. A repeated source ID yields
    /// [`NamedClassSourcesError::DuplicateSource`]. Construction validates
    /// declarations against the naming store. The collection owns the index,
    /// so clients can continue to mutate the store during typing.
    /// The supplied files and store must belong to the same session: numeric
    /// symbol IDs alone cannot distinguish arenas. Navigation is left lazy.
    pub fn new(
        files: impl IntoIterator<Item = &'a NamedClassSource>,
        store: &SemanticStore,
    ) -> Result<Self, NamedClassSourcesError> {
        let mut by_source = HashMap::new();
        let mut ordered = Vec::new();
        for file in files {
            if by_source.insert(file.source(), file).is_some() {
                return Err(NamedClassSourcesError::DuplicateSource(file.source()));
            }
            ordered.push(file);
        }
        let declarations =
            SourceDeclarationIndex::from_classes(store, ordered.iter().flat_map(|f| f.classes()))
                .map_err(NamedClassSourcesError::Declaration)?;
        let by_class = ordered
            .iter()
            .flat_map(|&file| {
                file.classes()
                    .iter()
                    .map(move |class| (class.symbol, (file, class)))
            })
            .collect();
        Ok(Self {
            by_class,
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

    /// Finds the declaring file and canonical class view for a class symbol in
    /// this collection. Members, constructors and classes from omitted files
    /// have no entry. The symbol must belong to the same naming session.
    pub fn class(&self, symbol: SymbolId) -> Option<(&'a NamedClassSource, &'a SourceClass)> {
        self.by_class.get(&symbol).copied()
    }

    /// Returns the validated declaration index, including constructors and their fields.
    pub fn declarations(&self) -> &SourceDeclarationIndex {
        &self.declarations
    }

    /// Resolves a same-session symbol to its declaring file and canonical owner.
    /// A symbol outside these files has no entry.
    pub fn declaration(
        &self,
        symbol: SymbolId,
    ) -> Option<(&'a NamedClassSource, SourceDeclaration)> {
        let declaration = self.declarations.get(symbol)?;
        Some((self.file(declaration.source)?, declaration))
    }

    /// Completes the primary constructor of the supplied class in its declaring
    /// file. Supply the naming session's store, definitions and packages. The
    /// returned type belongs to that store. Prepare the constructors of eligible
    /// classes before attempting their headers.
    ///
    /// An ineligible class yields [`ClassHeaderError::InvalidClass`] before
    /// typing. A rejected constructor rolls back its attempt and retains both the
    /// class and constructor symbols in the report.
    pub fn complete_constructor_signature(
        &self,
        class: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
    ) -> Result<TypeId, ClassHeaderError> {
        let (file, view) = self
            .class(class)
            .ok_or(ClassHeaderError::InvalidClass(class))?;
        crate::typer::SourceTyper::new(
            file.ast(),
            file.source(),
            file.index(),
            store,
            definitions,
            packages,
        )
        .complete_symbol(view.constructor)
        .map_err(|error| self.header_error(class, Some(view.constructor), error))
    }

    /// Prepares the requested constructors in caller order, one per iterator step.
    /// Consume the iterator before attempting headers. Collect into a `Result`
    /// to stop at the first error, or visit every outcome to recover across files.
    /// A failed attempt preserves earlier successful constructor signatures.
    /// All inputs, symbols and returned IDs belong to this naming session.
    pub fn prepare_class_constructors(
        &self,
        classes: impl IntoIterator<Item = SymbolId>,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
    ) -> impl Iterator<Item = Result<SymbolId, crate::ClassHeaderFailure>> {
        classes.into_iter().map(move |class| {
            self.complete_constructor_signature(class, store, definitions, packages)
                .map(|_| class)
                .map_err(|error| crate::ClassHeaderFailure {
                    class,
                    source: self.class(class).map(|(file, _)| file.source()),
                    error,
                })
        })
    }

    fn header_error(
        &self,
        class: SymbolId,
        member: Option<SymbolId>,
        error: TyperError,
    ) -> ClassHeaderError {
        ClassHeaderError::Typing(Box::new(ReportedTyperError::from_class(
            error,
            class,
            member,
            |id| self.file(id).map(NamedClassSource::ast),
        )))
    }

    /// Attempts one header using the declaring file and the indexed classes
    /// as eligible dependencies. Prepare constructor signatures first. Reuse the
    /// same session store, typed arena and bindings on every attempt, including
    /// retries after a returned dependency completes.
    ///
    /// An unknown symbol yields `UnknownSymbol`; a known symbol outside these files
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

    /// Attempts the header under the same contract as [`Self::try_complete_class_header`],
    /// capturing any typer failure before the source session ends. The report
    /// identifies the requested class and carries the actual failure's optional
    /// byte span, including for a recursively checked parent. No member is
    /// attributed to a header attempt. An ineligible symbol yields
    /// [`ClassHeaderError::InvalidClass`] without changing the session.
    pub fn try_complete_class_header_reported(
        &self,
        class: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
        arena: &mut AstArena<Typed>,
        bindings: &mut ClassHeaderBindings,
    ) -> Result<SignatureCompletion, ClassHeaderError> {
        self.try_complete_class_header(class, store, definitions, packages, arena, bindings)
            .map_err(|error| {
                if self.class(class).is_some() {
                    self.header_error(class, None, error)
                } else {
                    ClassHeaderError::InvalidClass(class)
                }
            })
    }

    /// Attempts one field or method signature after header completion. The
    /// collection supplies the declaring AST and naming index. An unresolved
    /// dependency in these files can yield `BlockedOn`. An unavailable external
    /// member type retains the original typer error. The caller chooses when to retry. An unknown or ineligible
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

    /// Types the ordinary class identified by `class` after signature completion.
    /// Supplies its canonical AST and index to [`crate::type_class_body_with_resolver`].
    /// Pass [`crate::core::NoResolver`] for source-only typing or borrow the session
    /// resolver. The first failing member rolls back its attempt. Earlier members
    /// remain committed; later members are skipped. An absent class returns
    /// [`crate::ClassBodyError::InvalidClass`] before touching the store or arena.
    pub fn type_class_body(
        &self,
        class: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
        arena: &mut AstArena<Typed>,
        resolver: impl crate::core::SymbolResolver,
    ) -> Result<crate::jvm_class_plan::TypedClassBody, crate::ClassBodyError> {
        let (file, view) = self
            .class(class)
            .ok_or(crate::ClassBodyError::InvalidClass(class))?;
        crate::type_class_body_with_resolver(
            view,
            file.ast(),
            file.index(),
            store,
            definitions,
            packages,
            arena,
            resolver,
        )
    }

    /// Checks all declared members of an ordinary class after signature completion.
    /// Uses the canonical source bindings and the supplied session arena and resolver.
    /// [`crate::check_class_body`] describes member rollback, warning order and
    /// the retained bindings for successful siblings. A symbol outside the indexed
    /// classes yields only [`crate::ClassBodyError::InvalidClass`], without typing.
    pub fn check_class_body(
        &self,
        class: SymbolId,
        store: &mut SemanticStore,
        definitions: Definitions,
        packages: &Packages,
        arena: &mut AstArena<Typed>,
        resolver: impl crate::core::SymbolResolver,
    ) -> crate::ClassBodyCheck {
        let Some((file, view)) = self.class(class) else {
            return crate::ClassBodyCheck::invalid(class);
        };
        crate::check_class_body(
            view,
            file.ast(),
            file.index(),
            store,
            definitions,
            packages,
            arena,
            resolver,
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
            let sources = NamedClassSources::new(files, &store).unwrap();
            assert_eq!(
                sources
                    .files()
                    .map(NamedClassSource::source)
                    .collect::<Vec<_>>(),
                files.map(NamedClassSource::source)
            );
            assert!(std::ptr::eq(sources.file(a.source()).unwrap(), &a));
            assert!(std::ptr::eq(sources.file(empty.source()).unwrap(), &empty));
            assert!(sources.file(SourceId::from_index(0)).is_none());
        }
        assert_eq!(
            NamedClassSources::new([&empty, &a, &empty], &store).unwrap_err(),
            NamedClassSourcesError::DuplicateSource(empty.source())
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
        let sources = NamedClassSources::new([&c, &ab], &store).unwrap();
        assert_eq!(
            sources
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
            let (owner_file, d) = sources.declaration(seed).unwrap();
            assert!(std::ptr::eq(owner_file, file));
            assert_eq!(
                (d.class, d.source, d.kind),
                (class.symbol, file.source(), crate::core::SymbolKind::Field)
            );
        }
        assert!(sources.declaration(omitted.classes()[0].symbol).is_none());
    }
    #[test]
    fn class_lookup_returns_exact_borrowed_views_only_for_included_classes() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let ab = named(
            42,
            "class A; class B { def value() = 1 }",
            &mut store,
            &mut packages,
        );
        let c = named(7, "class C", &mut store, &mut packages);
        let other = named(13, "class Other", &mut store, &mut packages);
        let sources = NamedClassSources::new([&c, &ab], &store).unwrap();
        for f in [&ab, &c] {
            for class in f.classes() {
                let (file, view) = sources.class(class.symbol).unwrap();
                assert!(std::ptr::eq(file, f));
                assert!(std::ptr::eq(view, class));
                assert!(sources.class(class.constructor).is_none());
            }
        }
        assert!(
            sources
                .class(ab.classes()[1].methods().next().unwrap().symbol)
                .is_none()
        );
        assert!(sources.class(other.classes()[0].symbol).is_none());
    }

    mod headers;
    mod members;

    fn prepare_constructors(
        sources: &NamedClassSources<'_>,
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
    ) {
        for (file, class) in sources.classes() {
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
        let sources = NamedClassSources::new([&leaf, &mid, &base], &store).unwrap();
        prepare_constructors(&sources, &mut store, defs, &packages);
        let [a, b, c] = [&leaf, &mid, &base].map(|f| f.classes()[0].symbol);
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        let mut attempt = |id| {
            sources.try_complete_class_header(
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
            crate::typer_error_span_with(&error, |id| sources.file(id).map(NamedClassSource::ast))
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
            let sources = NamedClassSources::new(files, &store).unwrap();
            prepare_constructors(&sources, &mut store, defs, &packages);
            let mut arena = AstArena::new();
            let mut bindings = ClassHeaderBindings::default();
            for (_, class) in sources.classes() {
                assert!(matches!(
                    sources.try_complete_class_header(
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
            let partial = NamedClassSources::new([&provider], &store).unwrap();
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
                |s| sources.try_complete_member_signature(s, &mut store, defs, &packages);
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

    // All classes in these body fixtures have independent headers and annotated members.
    fn prepare_bodies(
        sources: &NamedClassSources<'_>,
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
    ) -> AstArena<Typed> {
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        prepare_constructors(sources, store, defs, packages);
        for (_, class) in sources.classes() {
            assert!(matches!(
                sources.try_complete_class_header(
                    class.symbol,
                    store,
                    defs,
                    packages,
                    &mut arena,
                    &mut bindings
                ),
                Ok(SignatureCompletion::Complete(_))
            ));
            for symbol in class.fields().chain(class.methods()).map(|d| d.symbol) {
                assert!(matches!(
                    sources.try_complete_member_signature(symbol, store, defs, packages),
                    Ok(SignatureCompletion::Complete(_))
                ));
            }
        }
        arena
    }

    #[test]
    fn definition_query_uses_checked_call_bindings_and_target_overload_ranges() {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let target_text = "class A { def take(n: Int): Int = n; def take(b: Boolean): Int = 7 }";
        let text =
            "class B { def yes(a: A): Int = a.take(false); def skipped(a: A): Int = a.take(1) }";
        let target = named(42, target_text, &mut store, &mut packages);
        let caller = named(8, text, &mut store, &mut packages);
        let sources = NamedClassSources::new([&caller, &target], &store).unwrap();
        let mut arena = AstArena::new();
        let mut bindings = ClassHeaderBindings::default();
        let mut phase = crate::ClassHeaderPhase::prepare(
            &sources,
            &mut store,
            defs,
            &packages,
            &mut arena,
            &mut bindings,
        );
        for (_, c) in sources.classes() {
            let _ = phase.attempt(c.symbol).unwrap();
        }
        let selection = sources
            .member_signatures(phase.finish().ready, &store)
            .unwrap();
        let mut phase = crate::MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
        let owner = &caller.classes()[0];
        for d in target.classes()[0].methods().chain(owner.methods().take(1)) {
            let _ = phase.attempt(d.symbol).unwrap();
        }
        let (_, work) = phase.finish_with_body_work();
        let (body, report) = work.check(
            owner.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        );
        assert!(report.errors.is_empty());
        assert!(matches!(body, crate::CheckedClassBody::Partial(_)));
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        let lookup = |file, byte| {
            sources
                .definition_at(file, byte, &bodies, &arena, &store)
                .unwrap()
        };
        let byte = text.find("take").unwrap() as u32;
        let hit = lookup(caller.source(), byte + 1).unwrap();
        assert_eq!(hit.source, target.source());
        let slice = |r: crate::core::TextRange| &target_text[r.start() as usize..r.end() as usize];
        assert_eq!(slice(hit.name), "take");
        assert_eq!(
            hit.name.start() as usize,
            target_text.rfind("take").unwrap()
        );
        assert_eq!(slice(hit.declaration), "def take(b: Boolean): Int = 7");
        assert!(lookup(caller.source(), byte + 4).is_none());
        assert!(lookup(caller.source(), text.rfind("take").unwrap() as u32).is_none());
        assert!(lookup(SourceId::from_index(900), byte).is_none());
    }

    #[test]
    fn projected_classes_navigate_past_an_omitted_owner() {
        use crate::core::{NoResolver, TreeKind};
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let text = "class A { def value() = 40 }; class Dropped { def value() = 2; value() }; class B { def value(): Int = 7 }";
        let model = crate::parse_source(SourceId::from_index(7), text, Default::default())
            .unwrap()
            .bind(&mut store.names)
            .name("Model.scala", &mut store, &mut packages)
            .unwrap()
            .project_classes(&store)
            .unwrap();
        let file = &model.file;
        assert_eq!(model.rejections.len(), 1);
        let caller_text = "class Caller { def read(b: B): Int = b.value() }";
        let caller = named(42, caller_text, &mut store, &mut packages);
        let sources = NamedClassSources::new([&caller, file], &store).unwrap();
        let TreeKind::PackageDef(pkg) = &file.ast().get(file.named().syntax().root).kind else {
            panic!()
        };
        let omitted = pkg.stats[1];
        let TreeKind::TypeDef(def) = &file.ast().get(omitted).kind else {
            panic!()
        };
        let TreeKind::Template(template) = &file.ast().get(def.rhs).kind else {
            panic!()
        };
        for tree in [omitted, template.constructor, template.body[0]] {
            let symbol = file.index().symbol_at(file.source(), tree).unwrap();
            assert!(sources.declaration(symbol).is_none());
        }

        let target_class = &file.classes()[1];
        let method = target_class.methods().next().unwrap();
        let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
        let owner = caller.classes()[0].symbol;
        let body =
            sources.check_class_body(owner, &mut store, defs, &packages, &mut arena, NoResolver);
        let calls = caller.navigation().unwrap();
        let offset = caller_text.find("value").unwrap() as u32;
        assert_eq!(
            calls.checked_method_at(offset, caller.ast(), &arena, &store, |_| Some(&body)),
            Some(method.symbol)
        );
        let target = calls
            .definition_at(
                offset,
                caller.ast(),
                &arena,
                &store,
                |_| Some(&body),
                |source| sources.file(source)?.navigation().ok(),
            )
            .unwrap();
        assert_eq!(target.source, file.source());
        assert_eq!(target.name.start() as usize, text.rfind("value").unwrap());
        assert_eq!(
            sources.declaration(method.symbol).unwrap().1.class,
            target_class.symbol
        );
    }

    #[test]
    fn body_by_symbol_stops_at_the_first_field_and_rejects_an_omitted_class() {
        use crate::{ClassBodyError, core::NoResolver};
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let file = named(
            42,
            "class C { val bad: Int = false; def later(): Int = 7 }",
            &mut store,
            &mut packages,
        );
        let omitted = named(9, "class Other", &mut store, &mut packages);
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
        let before = (store.checkpoint(), arena.checkpoint());
        let other = omitted.classes()[0].symbol;
        assert!(
            matches!(sources.type_class_body(other,&mut store,defs,&packages,&mut arena,NoResolver),Err(ClassBodyError::InvalidClass(s)) if s == other)
        );
        let class = &file.classes()[0];
        assert!(
            matches!(sources.type_class_body(class.symbol,&mut store,defs,&packages,&mut arena,NoResolver),
            Err(ClassBodyError::Member{class:owner,member,error:TyperError::ExpectedExpressionTypeMismatch{actual,expected,..}})
            if owner == class.symbol && member == class.members[0] && actual == defs.boolean && expected == defs.int)
        );
        assert_eq!(before, (store.checkpoint(), arena.checkpoint()));
    }

    #[test]
    fn checking_by_symbol_keeps_ordered_errors_committed_bindings_and_warnings() {
        use crate::{ClassBodyError, core::NoResolver, typer::TyperWarning};
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let file = named(
            42,
            "class C { def f(implicit n: Int): Int = n; val first: Int = false; def good(): Int = f(1); val last: Int = true }",
            &mut store,
            &mut packages,
        );
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
        let class = &file.classes()[0];
        let before = (store.checkpoint(), arena.checkpoint());
        let invalid = sources.check_class_body(
            class.constructor,
            &mut store,
            defs,
            &packages,
            &mut arena,
            NoResolver,
        );
        assert!(
            matches!(invalid.body.as_ref().unwrap_err().as_slice(),[ClassBodyError::InvalidClass(id)] if *id == class.constructor)
        );
        assert!(invalid.diagnostics.is_empty() && invalid.bindings().is_none());
        assert_eq!(before, (store.checkpoint(), arena.checkpoint()));
        let check = sources.check_class_body(
            class.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            NoResolver,
        );
        let errors = check.body.as_ref().unwrap_err();
        assert_eq!(errors.len(), 2);
        for (error, decl) in errors.iter().zip(class.fields()) {
            assert!(
                matches!(error, ClassBodyError::Member {class: owner, member: crate::source_declarations::ClassMember::Field(field),error:TyperError::ExpectedExpressionTypeMismatch{actual,expected,..}}
                if *owner == class.symbol && field.symbol == decl.symbol && *actual == defs.boolean && *expected == defs.int)
            );
        }
        let [(source, warning)] = check.diagnostics.as_slice() else {
            panic!("expected one warning")
        };
        assert_eq!(*source, file.source());
        assert_eq!(warning.issue(), &TyperWarning::ImplicitClauseWithoutUsing);
        let good = class.methods().nth(1).unwrap();
        assert!(
            check
                .bindings()
                .unwrap()
                .get(file.source(), good.tree)
                .is_some()
        );
    }

    #[test]
    fn a_moved_report_leaves_navigation_only_for_committed_calls() {
        use crate::{CheckedClassBody, ClassBodyError, core::NoResolver, typer::TyperWarning};
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let text = "class C { def only(implicit x: Int): Int = x; def bad(): Int = only(false); def good(): Int = only(1) }";
        let file = named(42, text, &mut store, &mut packages);
        let sources = NamedClassSources::new([&file], &store).unwrap();
        let class = &file.classes()[0];
        let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
        let (body, report) = sources
            .check_class_body(
                class.symbol,
                &mut store,
                defs,
                &packages,
                &mut arena,
                NoResolver,
            )
            .into_parts();
        assert!(matches!(body, CheckedClassBody::Partial(_)));
        let mut errors = report.errors.into_iter();
        assert!(
            matches!(errors.next(), Some(ClassBodyError::Member{error:TyperError::ApplicationArgumentTypeMismatch{actual,expected,..},..}) if actual == defs.boolean && expected == defs.int)
        );
        assert!(errors.next().is_none());
        let [(id, warning)] = report.diagnostics.as_slice() else {
            panic!("expected good call warning")
        };
        assert_eq!(
            (*id, warning.issue()),
            (file.source(), &TyperWarning::ImplicitClauseWithoutUsing)
        );
        drop(report.diagnostics);
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        assert_eq!(
            bodies.complete(class.symbol).unwrap_err(),
            crate::CheckedBodyError::Partial(class.symbol)
        );
        let calls = file.navigation().unwrap();
        let definition = |needle| {
            calls.definition_at_with_bindings(
                text.find(needle).unwrap() as u32,
                file.ast(),
                &arena,
                &store,
                |owner| {
                    assert_eq!(owner, class.symbol);
                    bodies.bindings(owner)
                },
                |id| sources.file(id)?.navigation().ok(),
            )
        };
        assert!(definition("only(false)").is_none());
        let target = definition("only(1)").unwrap();
        assert_eq!(target.source, file.source());
        assert_eq!(target.name.start() as usize, text.find("only(").unwrap());
        let (invalid, report) = sources
            .check_class_body(
                class.constructor,
                &mut store,
                defs,
                &packages,
                &mut arena,
                NoResolver,
            )
            .into_parts();
        assert!(matches!(invalid, CheckedClassBody::Invalid(_)));
        assert_eq!(
            bodies.complete(class.constructor).unwrap_err(),
            crate::CheckedBodyError::Missing(class.constructor)
        );
        bodies.record(invalid).unwrap();
        assert_eq!(
            bodies.complete(class.constructor).unwrap_err(),
            crate::CheckedBodyError::Invalid(class.constructor)
        );
        assert!(bodies.bindings(class.constructor).is_none());
        assert!(
            matches!(report.errors.as_slice(),[ClassBodyError::InvalidClass(id)] if *id == class.constructor)
        );
    }
}
