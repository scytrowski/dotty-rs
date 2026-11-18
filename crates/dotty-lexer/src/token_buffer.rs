use std::ops::{Index, IndexMut, Range};
use std::sync::OnceLock;

use dotty_core::Token;

/// A gap buffer for edits near the parser cursor. The suffix is stored in reverse
/// order, so moving the gap transfers each crossed token once. Reads use logical
/// indices and leave the gap in place, including speculative parser lookahead.
#[derive(Debug)]
pub(crate) struct TokenBuffer {
    before: Vec<Token>,
    after: Vec<Token>,
    snapshot: OnceLock<Vec<Token>>,
}

impl From<Vec<Token>> for TokenBuffer {
    fn from(before: Vec<Token>) -> Self {
        Self {
            before,
            after: Vec::new(),
            snapshot: OnceLock::new(),
        }
    }
}

impl TokenBuffer {
    pub(crate) fn len(&self) -> usize {
        self.before.len() + self.after.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Token> {
        if index < self.before.len() {
            self.before.get(index)
        } else {
            self.after
                .get(self.len().checked_sub(index)?.checked_sub(1)?)
        }
    }

    pub(crate) fn range(
        &self,
        range: Range<usize>,
    ) -> impl DoubleEndedIterator<Item = &Token> + ExactSizeIterator {
        assert!(range.start <= range.end && range.end <= self.len());
        range.map(|index| &self[index])
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &Token> + ExactSizeIterator {
        self.range(0..self.len())
    }

    /// Materializes the public contiguous view on demand. Every edit invalidates
    /// this view, while ordinary parser reads use the two vectors directly.
    pub(crate) fn as_slice(&self) -> &[Token] {
        if self.after.is_empty() {
            &self.before
        } else {
            self.snapshot.get_or_init(|| self.iter().cloned().collect())
        }
    }

    pub(crate) fn insert(&mut self, index: usize, token: Token) {
        self.move_gap(index);
        self.snapshot.take();
        self.before.push(token);
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.remove_range(index..index + 1);
    }

    pub(crate) fn remove_range(&mut self, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= self.len());
        self.move_gap(range.start);
        self.snapshot.take();
        self.after.truncate(self.after.len() - range.len());
    }

    fn move_gap(&mut self, index: usize) {
        assert!(index <= self.len());
        if index < self.before.len() {
            self.after.extend(self.before.drain(index..).rev());
        } else {
            let split = self.len() - index;
            self.before.extend(self.after.drain(split..).rev());
        }
    }
}

impl Index<usize> for TokenBuffer {
    type Output = Token;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("token index is in bounds")
    }
}

impl IndexMut<usize> for TokenBuffer {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.snapshot.take();
        if index < self.before.len() {
            &mut self.before[index]
        } else {
            let offset = self.len() - index - 1;
            &mut self.after[offset]
        }
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::{TextRange, TokenKind};

    use super::*;

    fn token(offset: u32) -> Token {
        Token::new(
            TokenKind::Identifier,
            TextRange::new(offset, offset).unwrap(),
        )
    }

    fn assert_tokens(buffer: &TokenBuffer, expected: &[Token]) {
        assert_eq!(buffer.len(), expected.len());
        assert_eq!(buffer.as_slice(), expected);
        assert!(buffer.iter().eq(expected.iter()));
        assert!(buffer.iter().rev().eq(expected.iter().rev()));
        for start in 0..=expected.len() {
            for end in start..=expected.len() {
                assert!(buffer.range(start..end).eq(expected[start..end].iter()));
            }
        }
        assert_eq!(buffer.get(expected.len()), None);
        assert_eq!(buffer.get(usize::MAX), None);
    }

    #[test]
    fn edits_and_cached_views_follow_logical_token_order() {
        let mut buffer = TokenBuffer::from(vec![token(1), token(3), token(5)]);
        assert_tokens(&buffer, &[token(1), token(3), token(5)]);
        buffer.insert(1, token(2));
        assert_tokens(&buffer, &[token(1), token(2), token(3), token(5)]);
        buffer.insert(3, token(4));
        buffer.insert(0, token(0));
        buffer.insert(6, token(6));
        assert_tokens(&buffer, &(0..7).map(token).collect::<Vec<_>>());
        buffer.remove_range(2..5);
        assert_tokens(&buffer, &[token(0), token(1), token(5), token(6)]);
        buffer[2].kind = TokenKind::Outdent;
        let mut changed = token(5);
        changed.kind = TokenKind::Outdent;
        assert_tokens(&buffer, &[token(0), token(1), changed, token(6)]);
        buffer.remove(0);
        buffer.remove(2);
        buffer.remove_range(0..2);
        assert_tokens(&buffer, &[]);
        buffer.insert(0, token(7));
        buffer.remove_range(0..0);
        assert_tokens(&buffer, &[token(7)]);
    }
}
