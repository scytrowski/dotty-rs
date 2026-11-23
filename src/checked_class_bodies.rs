//! Retained complete and partial class checks from one frontend session.
use crate::{CheckedClassBody, core::SymbolId};
use std::{
    collections::{HashMap, hash_map::Entry},
    fmt,
};

/// Failure to record or retrieve a checked class body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedBodyError {
    /// A result for this class is already present and remains unchanged.
    Duplicate(SymbolId),
}

impl fmt::Display for CheckedBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
}
