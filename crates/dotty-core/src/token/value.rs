/// Optional data associated with a parser-facing token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenValue {
    /// Token has no additional semantic payload.
    None,
}
