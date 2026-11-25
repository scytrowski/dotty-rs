//! Binding written arguments to an ordered method clause.

/// Progress through one candidate's parameter list in source order.
/// Resolve a written name against this particular declaration before calling
/// [`Self::bind`]. Create a fresh instance for each overload.
///
/// ```
/// use dotty_typer::{ArgumentSlots, ArgumentSlotError};
/// let mut clause = ArgumentSlots::new(2);
/// assert_eq!(clause.bind(Some(1)), Ok(1));
/// assert_eq!(clause.bind(None), Err(ArgumentSlotError::MisplacedPositional));
/// assert_eq!(clause.next_unfilled(), Some(0));
/// assert_eq!(clause.bind(Some(0)), Ok(0));
/// assert_eq!(clause.next_unfilled(), None);
/// ```
pub struct ArgumentSlots {
    occupied: Vec<bool>,
    written: usize,
    next: usize,
}

/// Failure to assign the next written operand. The binding state is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentSlotError {
    /// The supplied ordinal or positional operand is outside the clause.
    OutsideClause,
    /// The formal was supplied by an earlier operand.
    Duplicate(usize),
    /// A positional argument would skip an earlier unfilled formal.
    MisplacedPositional,
}

impl ArgumentSlots {
    /// Starts an unbound clause with this many formals.
    pub fn new(arity: usize) -> Self {
        Self {
            occupied: vec![false; arity],
            written: 0,
            next: 0,
        }
    }

    /// First available formal for an insertion point without a written operand.
    pub fn next_unfilled(&self) -> Option<usize> {
        (self.next < self.occupied.len()).then_some(self.next)
    }

    /// Binds a named formal ordinal, or a positional argument when passed `None`.
    pub fn bind(&mut self, named: Option<usize>) -> Result<usize, ArgumentSlotError> {
        if named.is_none() && self.written != self.next {
            return Err(ArgumentSlotError::MisplacedPositional);
        }
        let ordinal = named.unwrap_or(self.written);
        let slot = self
            .occupied
            .get_mut(ordinal)
            .ok_or(ArgumentSlotError::OutsideClause)?;
        if *slot {
            return Err(ArgumentSlotError::Duplicate(ordinal));
        }
        *slot = true;
        self.written += 1;
        while self.occupied.get(self.next) == Some(&true) {
            self.next += 1;
        }
        Ok(ordinal)
    }
}
