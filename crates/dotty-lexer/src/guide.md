## Read the lossless raw stream

Keep the source buffer to interpret token and trivia ranges. Raw token kinds
classify spellings; later scanning, parsing, and typing interpret their values.
A recovered stream can contain diagnostics even when every call succeeds.

```rust
use dotty_lexer::{RawItem, RawLexer};

let source = "val ż = 42 // answer";
let mut lexer = RawLexer::new(source)?;
let mut reconstructed = String::new();
while let Some(item) = lexer.next()? {
    let range = match item {
        RawItem::Token(token) => token.span,
        RawItem::Trivia(trivia) => trivia.span,
    };
    reconstructed.push_str(&source[range.start() as usize..range.end() as usize]);
}
assert_eq!(reconstructed, source);
assert!(lexer.diagnostics().is_empty());

let mut broken = RawLexer::new("/* unfinished")?;
while broken.next()?.is_some() {}
assert!(!broken.diagnostics().is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Use [`ContextualScanner`] for the parser-facing [`dotty_core::TokenSource`]
contract. Parser feedback controls context-sensitive layout; pre-tokenizing a
raw stream is not equivalent to driving that contract. Inspect both scanner
construction errors and its collected diagnostics. Source ranges count UTF-8
bytes, and cursors reject offsets inside encoded scalars.
