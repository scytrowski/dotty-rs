//! Retained complete and partial class checks from one frontend session.
use crate::{
    CheckedClassBody,
    core::{SourceTypedIndex, SymbolId},
    jvm_class_plan::TypedClassBody,
};
use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
};

/// Failure to record or retrieve a checked class body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedBodyError {
    /// A result for this class is already present and remains unchanged.
    Duplicate(SymbolId),
    /// No outcome has been recorded for the requested class.
    Missing(SymbolId),
    /// The class has committed mappings but was not fully checked.
    Partial(SymbolId),
    /// The original check rejected this symbol before checking any members.
    Invalid(SymbolId),
}

impl fmt::Display for CheckedBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(class) => write!(f, "no body recorded for {class:?}"),
            Self::Partial(class) => write!(f, "partial body for {class:?}"),
            Self::Invalid(class) => write!(f, "invalid class for body check: {class:?}"),
            Self::Duplicate(class) => write!(f, "body already recorded for {class:?}"),
        }
    }
}

impl std::error::Error for CheckedBodyError {}

/// Owns the semantic outcomes of class checks independently of their diagnostics.
/// Record and query symbols from one session and retain its typed arena. Create
/// a new collection when rebuilding that session after an edit.
#[derive(Debug, Default)]
pub struct CheckedClassBodies {
    bodies: HashMap<SymbolId, CheckedClassBody>,
}

impl CheckedClassBodies {
    /// Moves an outcome into the collection under its original symbol. Recording
    /// a duplicate returns an error and keeps the first outcome, including a partial one.
    pub fn record(&mut self, body: CheckedClassBody) -> Result<(), CheckedBodyError> {
        match self.bodies.entry(body.class()) {
            Entry::Vacant(entry) => {
                entry.insert(body);
                Ok(())
            }
            Entry::Occupied(entry) => Err(CheckedBodyError::Duplicate(*entry.key())),
        }
    }

    /// Borrows a recorded outcome, including an invalid check. An unrecorded
    /// symbol yields `None` independently of checks performed on its siblings.
    pub fn get(&self, class: SymbolId) -> Option<&CheckedClassBody> {
        self.bodies.get(&class)
    }

    /// Borrows the committed mappings of a partial or complete body. Missing and
    /// invalid checks yield `None`. Use the original session's arenas for lookup.
    pub fn bindings(&self, class: SymbolId) -> Option<&SourceTypedIndex> {
        self.get(class)?.bindings()
    }

    /// Borrows a fully checked body for [`crate::jvm_class_plan::JvmClassPlanner::plan_typed_class`].
    /// Missing, partial and invalid outcomes retain distinct typed errors. The
    /// body stays available for navigation and subsequent planning in this session.
    pub fn complete(&self, class: SymbolId) -> Result<&TypedClassBody, CheckedBodyError> {
        match self.get(class) {
            Some(CheckedClassBody::Complete(body)) => Ok(body),
            Some(CheckedClassBody::Partial(_)) => Err(CheckedBodyError::Partial(class)),
            Some(CheckedClassBody::Invalid(_)) => Err(CheckedBodyError::Invalid(class)),
            None => Err(CheckedBodyError::Missing(class)),
        }
    }
}
