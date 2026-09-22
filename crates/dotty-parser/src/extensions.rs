//! Source-level Scala 3.9 extension definitions.
//!
//! The contextual start is dispatched here first. Header and method parsing
//! are implemented in later increments; keeping the boundary separate avoids
//! misclassifying an ordinary identifier named `extension`.

use crate::statements::ParsedStatement;
use crate::{Location, ParseDiagnosticKind, Parser, RecoverySet};

impl<'src, 'names, S> Parser<'src, 'names, S>
where
    S: dotty_core::TokenSource,
{
    pub(crate) fn parse_extension_definition(&mut self, _location: Location) -> ParsedStatement {
        let position = self.current_span();
        self.report(
            ParseDiagnosticKind::UnsupportedSyntax,
            "extension definitions are not implemented yet",
        );
        self.advance();
        self.recover_until(RecoverySet::Statement);
        ParsedStatement::Expression(self.error_expr(position))
    }
}
