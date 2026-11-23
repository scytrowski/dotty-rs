//! Class-body work selected by one completed member signature phase.
use std::collections::HashMap;

use crate::core::SymbolId;
use crate::source_declarations::SourceClass;
use crate::{
    MemberSignatureFailure, MemberSignatureReport, MemberSignatureSelection, NamedClassSource,
    SignatureBlockage,
};

/// Reason a field or method cannot be checked from this signature snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodySkipReason {
    /// Its own signature was rejected. The original error is in the signature report.
    Rejected,
    /// The signature still depends on a provider, including a failed provider or cycle.
    Blocked(SignatureBlockage),
    /// The caller stopped before the initial signature attempt or a released retry.
    Remaining,
}

/// Body eligibility for the exact named files and classes selected in one session.
/// Obtain it from [`crate::MemberSignaturePhase::finish_with_body_work`]. It owns
/// the skip decisions independently of the movable diagnostics and borrows the
/// original named files. Start a new phase after edits or further signature work.
#[derive(Debug)]
pub struct ClassBodyWork<'file> {
    classes: HashMap<SymbolId, (&'file NamedClassSource, &'file SourceClass)>,
    skipped: HashMap<SymbolId, BodySkipReason>,
}

impl<'file> ClassBodyWork<'file> {
    pub(crate) fn from_report(
        selection: &MemberSignatureSelection<'_, 'file>,
        report: &MemberSignatureReport,
    ) -> Self {
        let classes = selection
            .selected_classes()
            .map(|(file, class)| (class.symbol, (file, class)))
            .collect();
        let mut skipped = HashMap::new();
        for failure in &report.failures {
            let member = match failure {
                MemberSignatureFailure::Typing { report, .. } => report.member(),
                MemberSignatureFailure::InvalidMember(id) => Some(*id),
            };
            if let Some(member) = member {
                skipped.insert(member, BodySkipReason::Rejected);
            }
        }
        skipped.extend(
            report
                .blockages
                .iter()
                .map(|(id, cause)| (*id, BodySkipReason::Blocked(cause.clone()))),
        );
        skipped.extend(
            report
                .remaining
                .iter()
                .map(|id| (*id, BodySkipReason::Remaining)),
        );
        Self { classes, skipped }
    }

    /// Lists constructor fields in parameter order followed by body declarations
    /// in source order. A member with no skip reason has a ready signature. An
    /// unselected class yields `None`; a selected empty class has an empty iterator.
    pub fn members(
        &self,
        class: SymbolId,
    ) -> Option<impl Iterator<Item = (SymbolId, Option<&BodySkipReason>)>> {
        let (_, view) = self.classes.get(&class)?;
        Some(
            view.constructor_fields
                .iter()
                .copied()
                .chain(view.members.iter().map(|m| m.symbol()))
                .map(|id| (id, self.skipped.get(&id))),
        )
    }
}
