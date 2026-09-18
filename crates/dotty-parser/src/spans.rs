/// A parser position used to build a source span after consuming tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub(crate) start: u32,
}

impl Mark {
    /// Returns the byte offset at which this mark was created.
    pub const fn start(self) -> u32 {
        self.start
    }
}
