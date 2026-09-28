use crate::{ScannerEvent, Token};

/// Incremental source of parser-facing tokens.
pub trait TokenSource {
    /// Returns the current token.
    fn current(&self) -> &Token;

    /// Returns the explicit logical position of the current token.
    ///
    /// The position must change whenever [`Self::advance`] moves to the next
    /// token, even when two adjacent tokens have equal values. It must remain
    /// unchanged when advancing cannot move the source.
    fn position(&self) -> usize;

    /// Advances to the next token.
    fn advance(&mut self);

    /// Returns the token `n` positions after the current token.
    fn lookahead(&mut self, n: usize) -> &Token;

    /// Reports parser context needed by the contextual scanner.
    fn observe(&mut self, event: ScannerEvent);

    /// Reports an event at a lookahead token without advancing the source.
    ///
    /// Sources that cannot apply speculative layout feedback may keep the
    /// default behavior; parser callers should still verify the resulting
    /// lookahead before relying on it.
    fn observe_at(&mut self, offset: usize, event: ScannerEvent) {
        if offset == 0 {
            self.observe(event);
        }
    }
}
