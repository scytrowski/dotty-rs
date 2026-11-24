//! Members eligible for signature completion after class header checking.
use std::{collections::HashSet, fmt};

use crate::NamedClassSources;
use crate::core::{SemanticStore, SymbolId, SymbolInfo, Type, Visibility};
use crate::source_declarations::ClassMember;

/// Reason a requested class cannot contribute members to a selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberSelectionError {
    /// The symbol does not name an ordinary class in the supplied files.
    InvalidClass(SymbolId),
    /// The class lacks a completed primary constructor or its own class information.
    HeaderUnavailable(SymbolId),
}

impl fmt::Display for MemberSelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClass(id) => write!(f, "invalid source class {id:?}"),
            Self::HeaderUnavailable(id) => write!(f, "class header unavailable for {id:?}"),
        }
    }
}

impl std::error::Error for MemberSelectionError {}

/// Ordered members of the ready ordinary classes selected from one naming session.
/// Construct this view using [`NamedClassSources::member_signatures`]. The original
/// files stay borrowed while the store remains available for signature completion.
#[derive(Debug)]
pub struct MemberSignatureSelection<'s, 'file> {
    pub(crate) sources: &'s NamedClassSources<'file>,
    classes: HashSet<SymbolId>,
    members: Vec<SymbolId>,
    field_roots: Vec<SymbolId>,
}

impl<'file> NamedClassSources<'file> {
    /// Selects classes in caller order, typically from [`crate::ClassHeaderReport::ready`].
    /// Repeated classes contribute members only on first occurrence. Invalid or
    /// unavailable classes reject the entire selection without changing the store.
    /// Use the store, symbols and files from the same session.
    pub fn member_signatures(
        &self,
        ready: impl IntoIterator<Item = SymbolId>,
        store: &SemanticStore,
    ) -> Result<MemberSignatureSelection<'_, 'file>, MemberSelectionError> {
        let mut classes = HashSet::new();
        let mut members = Vec::new();
        let mut field_roots = Vec::new();
        for symbol in ready {
            if !classes.insert(symbol) {
                continue;
            }
            let (_, class) = self
                .class(symbol)
                .ok_or(MemberSelectionError::InvalidClass(symbol))?;
            if !matches!(store.symbols.info(symbol), SymbolInfo::Complete(ty)
                if matches!(store.types.try_get(*ty), Some(Type::ClassInfo(info)) if info.class == symbol))
                || !matches!(
                    store.symbols.info(class.constructor),
                    SymbolInfo::Complete(_)
                )
            {
                return Err(MemberSelectionError::HeaderUnavailable(symbol));
            }
            members.extend_from_slice(&class.constructor_fields);
            field_roots.extend_from_slice(&class.constructor_fields);
            for member in &class.members {
                let (ClassMember::Field(d) | ClassMember::Method(d)) = member;
                members.push(d.symbol);
                if matches!(member, ClassMember::Field(_))
                    && store.symbols.get(d.symbol).visibility == Visibility::Public
                {
                    field_roots.push(d.symbol);
                }
            }
        }
        Ok(MemberSignatureSelection {
            sources: self,
            classes,
            members,
            field_roots,
        })
    }
}

impl<'file> MemberSignatureSelection<'_, 'file> {
    pub(crate) fn selected_classes(
        &self,
    ) -> impl Iterator<
        Item = (
            &'file crate::NamedClassSource,
            &'file crate::source_declarations::SourceClass,
        ),
    > + '_ {
        self.classes
            .iter()
            .map(|id| self.sources.class(*id).expect("validated selected class"))
    }

    /// Constructor fields in parameter order, then all body fields and methods
    /// in interleaved source order, for each selected class.
    pub fn members(&self) -> &[SymbolId] {
        &self.members
    }

    /// Roots used by the class field signature pass: constructor fields followed
    /// by public body fields of each class. A compiler can visit these first,
    /// then request methods in its own order. Private body fields remain eligible.
    pub fn field_roots(&self) -> &[SymbolId] {
        &self.field_roots
    }

    /// Whether this field or method belongs to a selected class. Actual
    /// cross-file dependencies also obey the typer's kind and visibility checks.
    pub fn contains(&self, member: SymbolId) -> bool {
        self.declaration(member).is_some()
    }

    fn declaration(&self, member: SymbolId) -> Option<crate::SourceDeclaration> {
        self.sources
            .declarations()
            .member(member)
            .filter(|d| self.classes.contains(&d.class))
    }

    /// Attempts one selected signature using its canonical file. Only a missing
    /// eligible provider in this selection may become `BlockedOn`. A missing
    /// provider outside the selection retains its typer error. Recursive work
    /// within the declaring file follows the typer's ordinary completion rules.
    /// Failed or blocked attempts roll back; previously committed signatures remain.
    pub fn try_complete(
        &self,
        member: SymbolId,
        store: &mut SemanticStore,
        definitions: crate::core::Definitions,
        packages: &crate::core::Packages,
    ) -> Result<crate::typer::SignatureCompletion, crate::MemberSignatureError> {
        self.try_complete_with_resolver(
            member,
            store,
            definitions,
            packages,
            crate::core::NoResolver,
        )
    }

    pub(crate) fn try_complete_with_resolver(
        &self,
        member: SymbolId,
        store: &mut SemanticStore,
        definitions: crate::core::Definitions,
        packages: &crate::core::Packages,
        resolver: impl crate::core::SymbolResolver,
    ) -> Result<crate::typer::SignatureCompletion, crate::MemberSignatureError> {
        let (file, _) = self
            .sources
            .declaration(member)
            .ok_or(crate::MemberSignatureError::InvalidMember(member))?;
        crate::member_signature::try_complete_selected_member(
            member,
            |id| self.declaration(id),
            file.ast(),
            file.index(),
            store,
            definitions,
            packages,
            resolver,
        )
    }
}
