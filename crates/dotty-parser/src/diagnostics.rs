use dotty_core::{
    Diagnostic, DiagnosticSeverity, Name, SourceId, SourceSpan, TextRange, TokenKind,
};

/// Parser-specific category for a recoverable diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseDiagnosticKind {
    ExpectedToken,
    UnexpectedToken,
    ExpectedExpression,
    ExpectedType,
    ExpectedPattern,
    UnsupportedSyntax,
    UnboundPlaceholderParameter,
}

/// Typed reasons emitted by the type-grammar portion owned by issue #910.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeIssue {
    /// A type-argument clause contains no type arguments.
    EmptyTypeArgumentList,
    /// A comma in a type-argument clause is followed by its closing bracket.
    MissingTypeArgumentAfterComma { found: TokenKind },
    /// The source token used as a repeated-parameter marker is invalid.
    InvalidRepeatedParameterMarker { found: TokenKind },
    /// A repeated parameter is followed by another parameter.
    RepeatedParameterMustBeLast { found: TokenKind },
    /// A context function type has an empty parameter list.
    EmptyContextFunctionParameterList,
    /// A wildcard type appears where the grammar does not permit it.
    WildcardTypeNotAllowed,
    /// A match type is missing its braced or indented case region.
    ExpectedMatchTypeCaseRegion { found: TokenKind },
    /// An indented match-type case region did not close with an outdent.
    ExpectedMatchTypeOutdent { found: TokenKind },
    /// A braced match-type case region did not close with `}`.
    ExpectedMatchTypeRightBrace { found: TokenKind },
    /// A match type contains no case clauses.
    MissingMatchTypeCase,
    /// Parsing a match-type case did not advance the token source.
    MatchTypeCaseNoProgress { found: TokenKind },
    /// A match-type case pattern is not followed by `=>`.
    ExpectedMatchTypeCaseArrow { found: TokenKind },
    /// An indented match-type result did not close with an outdent.
    ExpectedMatchTypeResultOutdent { found: TokenKind },
    /// The wildcard in a match-type case is not a valid type name.
    InvalidMatchTypeWildcard { found: TokenKind },
    /// A type lambda has no type parameters.
    EmptyTypeLambdaParameterList,
    /// Polymorphic function type parameters are not followed by `=>`.
    ExpectedPolymorphicFunctionTypeArrow { found: TokenKind },
    /// A polymorphic function type lacks parameters or a function-type body.
    InvalidPolymorphicFunctionTypeShape {
        has_type_parameters: bool,
        has_function_body: bool,
    },
    /// A function-type parameter list has a comma without a following parameter.
    MissingFunctionTypeParameterAfterComma { named: bool, found: TokenKind },
    /// A function-type parameter is not followed by a comma or closing parenthesis.
    ExpectedFunctionTypeParameterSeparator { named: bool, found: TokenKind },
    /// An unnamed erased function type must start with an erased parameter.
    ExpectedLeadingErasedFunctionTypeParameter { found: TokenKind },
    /// An erased parameter occurs after the permitted leading unnamed parameter.
    OnlyLeadingUnnamedFunctionTypeParameterMayBeErased { found: TokenKind },
    /// A named function-type parameter list is missing its closing parenthesis.
    ExpectedNamedFunctionTypeCloseParen { found: TokenKind },
    /// A named function-type parameter is missing its identifier.
    ExpectedNamedFunctionTypeParameter { found: TokenKind },
    /// A named function-type parameter identifier is not followed by `:`.
    ExpectedNamedFunctionTypeParameterColon { found: TokenKind },
    /// A parsed function-type parameter list is not followed by its required arrow.
    ExpectedFunctionTypeArrow {
        arrow: TypeFunctionArrow,
        found: TokenKind,
    },
    /// An infix type reduction loop failed to advance its token source.
    InfixTypeNoProgress { found: TokenKind },
    /// Equal-precedence infix type operators use conflicting associativities.
    MixedAssociativityTypeOperators { left: Name, right: Name },
    /// A legacy `with` type operator has no following type operand.
    ExpectedTypeAfterLegacyWith { found: TokenKind },
    /// A colon-introduced refinement is missing its indentation region.
    ExpectedIndentedRefinementBody { found: TokenKind },
    /// A refinement-member loop failed to advance its token source.
    RefinementNoProgress { found: TokenKind },
    /// A class-like declaration is not allowed as a refinement member.
    ClassLikeRefinementMemberNotAllowed { found: TokenKind },
    /// A modifier or annotation is not allowed on a refinement member.
    ModifiedRefinementMemberNotAllowed { found: TokenKind },
    /// A refinement member has an unsupported declaration shape.
    UnsupportedRefinementMember { found: TokenKind },
    /// A refinement method parameter has a default argument.
    RefinementMethodDefaultArgumentNotAllowed,
    /// A refinement value or method has a right-hand side.
    RefinementMemberRightHandSideNotAllowed,
    /// A capture set contains a token that cannot start a capture reference.
    ExpectedCaptureReference { found: TokenKind },
    /// A capture-set comma is not followed by a capture reference.
    ExpectedCaptureReferenceAfterComma { found: TokenKind },
    /// A capture reference is followed by neither a comma nor `}`.
    ExpectedCaptureSetSeparator { found: TokenKind },
    /// A selected capture reference is missing its member name.
    ExpectedCaptureReferenceMember { found: TokenKind },
    /// The unsupported read-only `.rd` capture suffix was encountered.
    ReadOnlyCaptureSuffixUnsupported,
    /// A capture filter is missing its qualified type.
    ExpectedCaptureFilterType { found: TokenKind },
    /// A capture reference started where no simple reference is valid.
    InvalidCaptureReference { found: TokenKind },
}

/// Function-arrow shape expected by a parsed function-type parameter clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeFunctionArrow {
    /// The ordinary `=>` function arrow.
    Ordinary,
    /// The context `?=>` function arrow.
    Context,
    /// The pure `->` function arrow.
    Pure,
    /// The pure context `?->` function arrow.
    PureContext,
}

impl TypeIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::EmptyTypeArgumentList
            | Self::MissingTypeArgumentAfterComma { .. }
            | Self::InvalidRepeatedParameterMarker { .. }
            | Self::EmptyContextFunctionParameterList
            | Self::WildcardTypeNotAllowed
            | Self::MissingMatchTypeCase
            | Self::InvalidMatchTypeWildcard { .. }
            | Self::EmptyTypeLambdaParameterList
            | Self::ExpectedLeadingErasedFunctionTypeParameter { .. } => {
                ParseDiagnosticKind::ExpectedType
            }
            Self::MissingFunctionTypeParameterAfterComma { named: false, .. }
            | Self::OnlyLeadingUnnamedFunctionTypeParameterMayBeErased { .. } => {
                ParseDiagnosticKind::ExpectedType
            }
            Self::RepeatedParameterMustBeLast { .. }
            | Self::MatchTypeCaseNoProgress { .. }
            | Self::InvalidPolymorphicFunctionTypeShape { .. } => {
                ParseDiagnosticKind::UnexpectedToken
            }
            Self::ExpectedMatchTypeCaseRegion { .. }
            | Self::ExpectedMatchTypeOutdent { .. }
            | Self::ExpectedMatchTypeRightBrace { .. }
            | Self::ExpectedMatchTypeCaseArrow { .. }
            | Self::ExpectedMatchTypeResultOutdent { .. }
            | Self::ExpectedPolymorphicFunctionTypeArrow { .. }
            | Self::ExpectedFunctionTypeParameterSeparator { .. }
            | Self::MissingFunctionTypeParameterAfterComma { named: true, .. }
            | Self::ExpectedNamedFunctionTypeCloseParen { .. }
            | Self::ExpectedNamedFunctionTypeParameter { .. }
            | Self::ExpectedNamedFunctionTypeParameterColon { .. }
            | Self::ExpectedFunctionTypeArrow { .. }
            | Self::ExpectedIndentedRefinementBody { .. }
            | Self::ExpectedCaptureSetSeparator { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::InfixTypeNoProgress { .. }
            | Self::MixedAssociativityTypeOperators { .. }
            | Self::RefinementNoProgress { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::ExpectedTypeAfterLegacyWith { .. }
            | Self::ExpectedCaptureReference { .. }
            | Self::ExpectedCaptureReferenceAfterComma { .. }
            | Self::ExpectedCaptureReferenceMember { .. }
            | Self::ExpectedCaptureFilterType { .. }
            | Self::InvalidCaptureReference { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ClassLikeRefinementMemberNotAllowed { .. }
            | Self::ModifiedRefinementMemberNotAllowed { .. }
            | Self::UnsupportedRefinementMember { .. }
            | Self::RefinementMethodDefaultArgumentNotAllowed
            | Self::RefinementMemberRightHandSideNotAllowed
            | Self::ReadOnlyCaptureSuffixUnsupported => ParseDiagnosticKind::UnsupportedSyntax,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::EmptyTypeArgumentList => "parser.type.empty_type_argument_list",
            Self::MissingTypeArgumentAfterComma { .. } => {
                "parser.type.missing_type_argument_after_comma"
            }
            Self::InvalidRepeatedParameterMarker { .. } => {
                "parser.type.invalid_repeated_parameter_marker"
            }
            Self::RepeatedParameterMustBeLast { .. } => {
                "parser.type.repeated_parameter_must_be_last"
            }
            Self::EmptyContextFunctionParameterList => {
                "parser.type.empty_context_function_parameter_list"
            }
            Self::WildcardTypeNotAllowed => "parser.type.wildcard_type_not_allowed",
            Self::ExpectedMatchTypeCaseRegion { .. } => {
                "parser.type.expected_match_type_case_region"
            }
            Self::ExpectedMatchTypeOutdent { .. } => "parser.type.expected_match_type_outdent",
            Self::ExpectedMatchTypeRightBrace { .. } => {
                "parser.type.expected_match_type_right_brace"
            }
            Self::MissingMatchTypeCase => "parser.type.missing_match_type_case",
            Self::MatchTypeCaseNoProgress { .. } => "parser.type.match_type_case_no_progress",
            Self::ExpectedMatchTypeCaseArrow { .. } => "parser.type.expected_match_type_case_arrow",
            Self::ExpectedMatchTypeResultOutdent { .. } => {
                "parser.type.expected_match_type_result_outdent"
            }
            Self::InvalidMatchTypeWildcard { .. } => "parser.type.invalid_match_type_wildcard",
            Self::EmptyTypeLambdaParameterList => "parser.type.empty_type_lambda_parameters",
            Self::ExpectedPolymorphicFunctionTypeArrow { .. } => {
                "parser.type.expected_polymorphic_function_arrow"
            }
            Self::InvalidPolymorphicFunctionTypeShape { .. } => {
                "parser.type.invalid_polymorphic_function_shape"
            }
            Self::MissingFunctionTypeParameterAfterComma { named: true, .. } => {
                "parser.type.missing_named_function_parameter_after_comma"
            }
            Self::MissingFunctionTypeParameterAfterComma { named: false, .. } => {
                "parser.type.missing_function_parameter_after_comma"
            }
            Self::ExpectedFunctionTypeParameterSeparator { named: true, .. } => {
                "parser.type.expected_named_function_parameter_separator"
            }
            Self::ExpectedFunctionTypeParameterSeparator { named: false, .. } => {
                "parser.type.expected_function_parameter_separator"
            }
            Self::ExpectedLeadingErasedFunctionTypeParameter { .. } => {
                "parser.type.expected_leading_erased_function_parameter"
            }
            Self::OnlyLeadingUnnamedFunctionTypeParameterMayBeErased { .. } => {
                "parser.type.only_leading_function_parameter_may_be_erased"
            }
            Self::ExpectedNamedFunctionTypeCloseParen { .. } => {
                "parser.type.expected_named_function_parameter_close_paren"
            }
            Self::ExpectedNamedFunctionTypeParameter { .. } => {
                "parser.type.expected_named_function_parameter"
            }
            Self::ExpectedNamedFunctionTypeParameterColon { .. } => {
                "parser.type.expected_named_function_parameter_colon"
            }
            Self::ExpectedFunctionTypeArrow { arrow, .. } => match arrow {
                TypeFunctionArrow::Ordinary => "parser.type.expected_function_arrow",
                TypeFunctionArrow::Context => "parser.type.expected_context_function_arrow",
                TypeFunctionArrow::Pure => "parser.type.expected_pure_function_arrow",
                TypeFunctionArrow::PureContext => {
                    "parser.type.expected_pure_context_function_arrow"
                }
            },
            Self::InfixTypeNoProgress { .. } => "parser.type.infix_type_no_progress",
            Self::MixedAssociativityTypeOperators { .. } => {
                "parser.type.mixed_associativity_operators"
            }
            Self::ExpectedTypeAfterLegacyWith { .. } => {
                "parser.type.expected_type_after_legacy_with"
            }
            Self::ExpectedIndentedRefinementBody { .. } => {
                "parser.type.expected_indented_refinement_body"
            }
            Self::RefinementNoProgress { .. } => "parser.type.refinement_no_progress",
            Self::ClassLikeRefinementMemberNotAllowed { .. } => {
                "parser.type.class_like_refinement_member_not_allowed"
            }
            Self::ModifiedRefinementMemberNotAllowed { .. } => {
                "parser.type.modified_refinement_member_not_allowed"
            }
            Self::UnsupportedRefinementMember { .. } => "parser.type.unsupported_refinement_member",
            Self::RefinementMethodDefaultArgumentNotAllowed => {
                "parser.type.refinement_method_default_argument_not_allowed"
            }
            Self::RefinementMemberRightHandSideNotAllowed => {
                "parser.type.refinement_member_rhs_not_allowed"
            }
            Self::ExpectedCaptureReference { .. } => "parser.type.expected_capture_reference",
            Self::ExpectedCaptureReferenceAfterComma { .. } => {
                "parser.type.expected_capture_reference_after_comma"
            }
            Self::ExpectedCaptureSetSeparator { .. } => {
                "parser.type.expected_capture_set_separator"
            }
            Self::ExpectedCaptureReferenceMember { .. } => {
                "parser.type.expected_capture_reference_member"
            }
            Self::ReadOnlyCaptureSuffixUnsupported => {
                "parser.type.read_only_capture_suffix_unsupported"
            }
            Self::ExpectedCaptureFilterType { .. } => "parser.type.expected_capture_filter_type",
            Self::InvalidCaptureReference { .. } => "parser.type.invalid_capture_reference",
        }
    }
}

/// Structured parser issue data.
///
/// `Legacy` temporarily retains messages from parser call sites that have not
/// yet migrated to typed issues. Typed variants carry issue data rather than
/// preformatted diagnostic text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseIssue {
    /// Transitional payload for parser diagnostics that still use free-form
    /// messages. New reporting code should use a typed variant instead.
    Legacy {
        kind: ParseDiagnosticKind,
        message: String,
    },
    /// A required parser-facing token was absent.
    ExpectedToken {
        expected: TokenKind,
        found: TokenKind,
    },
    /// An expression was required at the current token.
    ExpectedExpression { found: TokenKind },
    /// A type was required at the current token.
    ExpectedType { found: TokenKind },
    /// A pattern was required at the current token.
    ExpectedPattern { found: TokenKind },
    /// A token is not valid in the current parser position.
    UnexpectedToken { found: TokenKind },
    /// The expression fragment contains tokens after its expression.
    TrailingInput { found: TokenKind },
    /// A placeholder parameter escaped the expression that owns it.
    UnboundPlaceholderParameter,
    /// A structured failure emitted by the Scala type grammar.
    Type(TypeIssue),
}

impl ParseIssue {
    /// Returns the stable parser diagnostic category for this issue.
    pub const fn kind(&self) -> ParseDiagnosticKind {
        match self {
            Self::Legacy { kind, .. } => *kind,
            Self::ExpectedToken { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ExpectedExpression { .. } => ParseDiagnosticKind::ExpectedExpression,
            Self::ExpectedType { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ExpectedPattern { .. } => ParseDiagnosticKind::ExpectedPattern,
            Self::UnexpectedToken { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::TrailingInput { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::UnboundPlaceholderParameter => ParseDiagnosticKind::UnboundPlaceholderParameter,
            Self::Type(issue) => issue.kind(),
        }
    }

    /// Returns a stable, text-independent identifier for corpus reporting.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Legacy { kind, .. } => match kind {
                ParseDiagnosticKind::ExpectedToken => "parser.expected_token",
                ParseDiagnosticKind::UnexpectedToken => "parser.unexpected_token",
                ParseDiagnosticKind::ExpectedExpression => "parser.expected_expression",
                ParseDiagnosticKind::ExpectedType => "parser.expected_type",
                ParseDiagnosticKind::ExpectedPattern => "parser.expected_pattern",
                ParseDiagnosticKind::UnsupportedSyntax => "parser.unsupported_syntax",
                ParseDiagnosticKind::UnboundPlaceholderParameter => {
                    "parser.unbound_placeholder_parameter"
                }
            },
            Self::ExpectedToken { .. } => "parser.expected_token",
            Self::ExpectedExpression { .. } => "parser.expected_expression",
            Self::ExpectedType { .. } => "parser.expected_type",
            Self::ExpectedPattern { .. } => "parser.expected_pattern",
            Self::UnexpectedToken { .. } => "parser.unexpected_token",
            Self::TrailingInput { .. } => "parser.trailing_input",
            Self::UnboundPlaceholderParameter => "parser.unbound_placeholder_parameter",
            Self::Type(issue) => issue.code(),
        }
    }
}

/// A parser diagnostic that retains the shared diagnostic payload and source identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDiagnostic {
    source: SourceId,
    diagnostic: Diagnostic<ParseIssue>,
}

impl ParseDiagnostic {
    /// Creates a transitional legacy-message error diagnostic at `span`.
    pub fn error(kind: ParseDiagnosticKind, span: SourceSpan, message: impl Into<String>) -> Self {
        Self::with_issue(
            span,
            ParseIssue::Legacy {
                kind,
                message: message.into(),
            },
        )
    }

    /// Creates a parser error diagnostic with structured issue data.
    pub fn with_issue(span: SourceSpan, issue: ParseIssue) -> Self {
        Self {
            source: span.source(),
            diagnostic: Diagnostic::with_issue(
                DiagnosticSeverity::Error,
                span.span().range(),
                issue,
            ),
        }
    }

    /// Returns the structured parser issue payload.
    pub fn issue(&self) -> &ParseIssue {
        self.diagnostic.issue()
    }

    /// Returns the parser-specific diagnostic category.
    ///
    /// This compatibility classification is derived from the issue payload;
    /// typed callers should inspect [`Self::issue`] for its structured data.
    pub const fn kind(&self) -> ParseDiagnosticKind {
        self.diagnostic.issue().kind()
    }

    /// Returns the source file identity.
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// Returns the shared diagnostic severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.diagnostic.severity()
    }

    /// Returns the source range of the diagnostic.
    pub const fn span(&self) -> TextRange {
        self.diagnostic.span()
    }

    /// Returns the message when this issue still uses the transitional legacy
    /// payload. Typed issues intentionally have no rendered message here.
    pub fn legacy_message(&self) -> Option<&str> {
        match self.issue() {
            ParseIssue::Legacy { message, .. } => Some(message),
            ParseIssue::ExpectedToken { .. }
            | ParseIssue::ExpectedExpression { .. }
            | ParseIssue::ExpectedType { .. }
            | ParseIssue::ExpectedPattern { .. }
            | ParseIssue::UnexpectedToken { .. }
            | ParseIssue::TrailingInput { .. }
            | ParseIssue::UnboundPlaceholderParameter => None,
            ParseIssue::Type(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::{Span, TextRange};

    #[test]
    fn parser_diagnostic_preserves_legacy_kind_source_range_and_message() {
        let range = TextRange::new(2, 5).expect("valid range");
        let source = SourceId::from_index(4);
        let diagnostic = ParseDiagnostic::error(
            ParseDiagnosticKind::ExpectedExpression,
            SourceSpan::new(source, Span::without_point(range)),
            "expected expression",
        );

        assert_eq!(diagnostic.kind(), ParseDiagnosticKind::ExpectedExpression);
        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span(), range);
        assert_eq!(diagnostic.legacy_message(), Some("expected expression"));
        assert!(matches!(diagnostic.issue(), ParseIssue::Legacy { .. }));
    }

    #[test]
    fn typed_issue_preserves_source_severity_payload_and_clone_without_message() {
        let range = TextRange::new(2, 5).expect("valid range");
        let source = SourceId::from_index(7);
        let diagnostic = ParseDiagnostic::with_issue(
            SourceSpan::new(source, Span::without_point(range)),
            ParseIssue::ExpectedToken {
                expected: TokenKind::Keyword(dotty_core::HardKeyword::If),
                found: TokenKind::Identifier,
            },
        );
        let cloned = diagnostic.clone();

        assert_eq!(diagnostic, cloned);
        assert_eq!(diagnostic.kind(), ParseDiagnosticKind::ExpectedToken);
        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.span(), range);
        assert!(matches!(
            diagnostic.issue(),
            ParseIssue::ExpectedToken {
                expected: TokenKind::Keyword(dotty_core::HardKeyword::If),
                found: TokenKind::Identifier
            }
        ));
        assert_eq!(diagnostic.legacy_message(), None);
    }
}
