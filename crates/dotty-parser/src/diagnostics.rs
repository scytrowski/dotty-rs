use dotty_core::{
    Diagnostic, DiagnosticSeverity, Name, SourceId, SourceSpan, TextRange, TokenKind,
};

use crate::ParamOwner;

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

/// Typed failures in Scala type-parameter and context-bound clauses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeParamIssue {
    /// A type-parameter clause is empty.
    EmptyParameterClause { found: TokenKind },
    /// A comma is not followed by a type parameter.
    ExpectedParameterAfterComma { found: TokenKind },
    /// A type parameter has no valid name.
    ExpectedParameterName { found: TokenKind },
    /// A type-parameter clause is missing a comma or closing bracket.
    ExpectedParameterSeparator { found: TokenKind },
    /// Parsing a type parameter did not advance the token source.
    ParameterNoProgress { found: TokenKind },
    /// Variance is not permitted on a polymorphic function type parameter.
    VarianceNotAllowedForPolyFunctionParameter,
    /// Context bounds are not supported for this parameter owner.
    ContextBoundsNotAllowedForOwner { owner: Option<ParamOwner> },
    /// Context bounds are not supported on polymorphic function type parameters.
    ContextBoundsNotAllowedForPolyFunctionParameter,
    /// An empty braced context-bound list needs a type.
    EmptyBracedContextBoundList { found: TokenKind },
    /// A braced context-bound list begins with a comma instead of a type.
    ExpectedContextBoundAtListStart { found: TokenKind },
    /// Parsing a context bound did not advance the token source.
    ContextBoundNoProgress { found: TokenKind },
    /// A comma is not followed by another context-bound type.
    ExpectedContextBoundAfterComma { found: TokenKind },
    /// A type parameter's context bound has no type.
    ExpectedContextBoundType { found: TokenKind },
    /// A context-bound `as` clause is missing its alias name.
    ExpectedContextBoundAlias { found: TokenKind },
}

impl TypeParamIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::EmptyParameterClause { .. }
            | Self::ExpectedParameterAfterComma { .. }
            | Self::ExpectedParameterName { .. }
            | Self::EmptyBracedContextBoundList { .. }
            | Self::ExpectedContextBoundAtListStart { .. }
            | Self::ExpectedContextBoundAfterComma { .. }
            | Self::ExpectedContextBoundType { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ExpectedParameterSeparator { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ParameterNoProgress { .. } | Self::ContextBoundNoProgress { .. } => {
                ParseDiagnosticKind::UnexpectedToken
            }
            Self::VarianceNotAllowedForPolyFunctionParameter
            | Self::ContextBoundsNotAllowedForOwner { .. }
            | Self::ContextBoundsNotAllowedForPolyFunctionParameter => {
                ParseDiagnosticKind::UnsupportedSyntax
            }
            Self::ExpectedContextBoundAlias { .. } => ParseDiagnosticKind::ExpectedExpression,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::EmptyParameterClause { .. } => "parser.type_param.empty_clause",
            Self::ExpectedParameterAfterComma { .. } => {
                "parser.type_param.expected_parameter_after_comma"
            }
            Self::ExpectedParameterName { .. } => "parser.type_param.expected_name",
            Self::ExpectedParameterSeparator { .. } => "parser.type_param.expected_separator",
            Self::ParameterNoProgress { .. } => "parser.type_param.no_progress",
            Self::VarianceNotAllowedForPolyFunctionParameter => {
                "parser.type_param.variance_not_allowed_for_polyfunction"
            }
            Self::ContextBoundsNotAllowedForOwner { .. } => {
                "parser.type_param.context_bounds_not_allowed_for_owner"
            }
            Self::ContextBoundsNotAllowedForPolyFunctionParameter => {
                "parser.type_param.context_bounds_not_allowed_for_polyfunction"
            }
            Self::EmptyBracedContextBoundList { .. } => {
                "parser.type_param.empty_braced_context_bound_list"
            }
            Self::ExpectedContextBoundAtListStart { .. } => {
                "parser.type_param.expected_context_bound_at_list_start"
            }
            Self::ContextBoundNoProgress { .. } => "parser.type_param.context_bound_no_progress",
            Self::ExpectedContextBoundAfterComma { .. } => {
                "parser.type_param.expected_context_bound_after_comma"
            }
            Self::ExpectedContextBoundType { .. } => {
                "parser.type_param.expected_context_bound_type"
            }
            Self::ExpectedContextBoundAlias { .. } => {
                "parser.type_param.expected_context_bound_alias"
            }
        }
    }
}

/// Typed failures in Scala type definitions and their right-hand sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeDefinitionIssue {
    /// A `type` definition has no valid name.
    ExpectedName { found: TokenKind },
    /// A type alias is missing the type after `=`.
    ExpectedAliasType { found: TokenKind },
    /// A match type alias cannot have a lower bound.
    LowerBoundNotAllowedOnMatchTypeAlias { found: TokenKind },
    /// Only match type aliases can combine a bound with an alias.
    BoundedAliasMustBeMatchType { found: TokenKind },
    /// An opaque type definition without bounds is missing `=`.
    ExpectedEqualsAfterOpaqueDefinition { found: TokenKind },
    /// An opaque type bound is missing its required `=`.
    ExpectedEqualsAfterOpaqueBound { found: TokenKind },
}

impl TypeDefinitionIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::ExpectedName { .. } | Self::ExpectedAliasType { .. } => {
                ParseDiagnosticKind::ExpectedType
            }
            Self::LowerBoundNotAllowedOnMatchTypeAlias { .. }
            | Self::BoundedAliasMustBeMatchType { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::ExpectedEqualsAfterOpaqueDefinition { .. }
            | Self::ExpectedEqualsAfterOpaqueBound { .. } => ParseDiagnosticKind::ExpectedToken,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::ExpectedName { .. } => "parser.type_definition.expected_name",
            Self::ExpectedAliasType { .. } => "parser.type_definition.expected_alias_type",
            Self::LowerBoundNotAllowedOnMatchTypeAlias { .. } => {
                "parser.type_definition.match_alias_lower_bound"
            }
            Self::BoundedAliasMustBeMatchType { .. } => {
                "parser.type_definition.bounded_alias_must_be_match_type"
            }
            Self::ExpectedEqualsAfterOpaqueDefinition { .. } => {
                "parser.type_definition.expected_equals_after_opaque_definition"
            }
            Self::ExpectedEqualsAfterOpaqueBound { .. } => {
                "parser.type_definition.expected_equals_after_opaque_bound"
            }
        }
    }
}

/// Typed failures emitted by expression parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionIssue {
    /// A `using` application has no argument.
    MissingUsingArgument { found: TokenKind },
    /// An application argument is missing.
    MissingArgument { found: TokenKind },
    /// A spread argument appears before the final argument.
    NonFinalArgumentSpread,
    /// Parsing a postfix operator did not advance the token source.
    PostfixOperatorNoProgress { found: TokenKind },
    /// Parsing an infix expression did not advance the token source.
    InfixExpressionNoProgress { found: TokenKind },
    /// Equal-precedence infix operators use conflicting associativities.
    MixedAssociativityOperators { left: Name, right: Name },
    /// A prefix operator is not followed by its operand on the same line.
    PrefixOperandMustShareLine { found: TokenKind },
    /// `inline` is not followed by an inline `if` or `match` form.
    ExpectedInlineIfOrMatch { found: TokenKind },
    /// An assignment operator has no right-hand expression.
    MissingAssignmentRhs { found: TokenKind },
    /// An assignment left-hand tree is not assignable.
    UnassignableAssignmentTarget { found: TokenKind },
    /// The legacy `_*` splice is not the final application argument.
    LegacyWildcardSpliceNotFinal { found: TokenKind },
    /// A quoted expression is missing its closing brace.
    ExpectedQuotedExpressionCloseBrace { found: TokenKind },
    /// A quoted type is missing its closing bracket.
    ExpectedQuotedTypeCloseBracket { found: TokenKind },
    /// A quote marker is not followed by a quoted expression or type.
    ExpectedQuoteBodyStart { found: TokenKind },
    /// Consecutive quoted type definitions are missing a separator.
    ExpectedQuotedTypeDefinitionSeparator { found: TokenKind },
    /// A pattern splice is missing its closing brace.
    ExpectedPatternSpliceCloseBrace { found: TokenKind },
    /// An expression splice is missing its closing brace.
    ExpectedExpressionSpliceCloseBrace { found: TokenKind },
    /// A braced case-lambda is missing its closing brace.
    ExpectedCaseLambdaCloseBrace { found: TokenKind },
    /// A brace block is missing its closing brace.
    ExpectedBlockCloseBrace { found: TokenKind },
    /// A `super[...]` qualifier is missing a type name.
    ExpectedSuperTypeQualifier { found: TokenKind },
    /// A `super` expression is missing a selector.
    ExpectedSelectorAfterSuper { found: TokenKind },
    /// A `super.` expression is missing its selector.
    ExpectedSelectorAfterSuperDot { found: TokenKind },
    /// A selection dot is not followed by a selector.
    ExpectedSelectorAfterDot { found: TokenKind },
    /// A suffix attempts to call an expression that is not callable in this syntax.
    InvalidApplicationTarget {
        target: ExpressionApplicationTarget,
        found: TokenKind,
    },
    /// Parsing a simple-expression suffix did not advance the token source.
    ExpressionSuffixNoProgress { found: TokenKind },
    /// No expression can start at the current token.
    ExpectedExpressionAtCurrentToken { found: TokenKind },
    /// A guard's `if` is not followed by its condition expression.
    ExpectedGuardExpression { found: TokenKind },
    /// An `if` condition is not followed by `then`.
    ExpectedIfThen { found: TokenKind },
    /// A `while` condition is not followed by `do`.
    ExpectedWhileDo { found: TokenKind },
    /// A legacy `do` loop has no body expression.
    MissingDoWhileBody { found: TokenKind },
    /// A legacy `do` loop body is not followed by `while`.
    ExpectedDoWhileWhile { found: TokenKind },
    /// A legacy `do ... while` loop has no condition.
    MissingDoWhileCondition { found: TokenKind },
    /// A catch handler has no body.
    EmptyCatchHandler { found: TokenKind },
    /// A control-flow construct is missing its branch/body expression.
    ExpectedControlFlowBranch { found: TokenKind },
    /// A layout-introduced expression is missing its body.
    ExpectedLayoutExpression {
        context: LayoutExpressionContext,
        found: TokenKind,
    },
    /// An indented expression is not closed by an outdent.
    ExpectedIndentedExpressionOutdent { found: TokenKind },
    /// A catch handler is missing a `case` clause.
    ExpectedCatchCase { found: TokenKind },
    /// Braced catch clauses are missing their closing brace.
    ExpectedCatchCloseBrace { found: TokenKind },
    /// Indented catch clauses are missing their closing outdent.
    ExpectedCatchOutdent { found: TokenKind },
    /// Consuming control-flow newlines did not advance the token source.
    ControlFlowNewlineNoProgress { found: TokenKind },
    /// An indented case-lambda is missing its closing outdent.
    ExpectedCaseLambdaOutdent { found: TokenKind },
    /// An indented block is missing its closing outdent.
    ExpectedIndentedBlockOutdent { found: TokenKind },
    /// An indented for-enumerator region is missing its closing outdent.
    ExpectedForEnumeratorOutdent { found: TokenKind },
    /// A for-comprehension is missing `yield` or `do` after its enumerators.
    ExpectedForBodyKeyword { found: TokenKind },
    /// A for guard appears before any generator.
    ForGuardMissingGenerator { found: TokenKind },
    /// Parsing for enumerators did not advance the token source.
    ForEnumeratorsNoProgress { found: TokenKind },
    /// A for-comprehension contains no generator.
    ForMissingGenerator { found: TokenKind },
    /// A for pattern is not followed by `<-` or `=`.
    ExpectedForEnumeratorOperator { found: TokenKind },
    /// A case-generator pattern is not followed by `<-`.
    ExpectedCaseGeneratorOperator { found: TokenKind },
    /// A for enumerator operator has no right-hand expression.
    ExpectedForEnumeratorExpression { found: TokenKind },
    /// A for body delimiter is not followed by an expression.
    ExpectedForBodyExpression { found: TokenKind },
    /// An indented colon-introduced match clause is missing its outdent.
    ExpectedMatchColonOutdent { found: TokenKind },
    /// Braced match cases are missing their closing brace.
    ExpectedMatchCloseBrace { found: TokenKind },
    /// Indented match cases are missing their closing outdent.
    ExpectedMatchOutdent { found: TokenKind },
    /// A `match` expression is not followed by a braced or indented case region.
    ExpectedMatchCaseRegion { found: TokenKind },
    /// A match expression has no case clauses.
    ExpectedMatchCase { found: TokenKind },
    /// A legacy implicit lambda parameter is not followed by `=>`.
    ExpectedLegacyLambdaArrow { found: TokenKind },
    /// A context-function literal has no formal parameters.
    ContextFunctionRequiresParameter { found: TokenKind },
    /// Lambda parameters are not followed by `=>`.
    ExpectedLambdaArrow { found: TokenKind },
    /// A lambda parameter list contains a missing parameter.
    ExpectedLambdaParameter { found: TokenKind },
    /// Lambda parameters are not followed by `)` before the arrow.
    ExpectedLambdaParameterCloseParen { found: TokenKind },
    /// A lambda parameter is not followed by `,` or `)`.
    ExpectedLambdaParameterSeparator { found: TokenKind },
    /// A lambda parameter does not begin with an identifier or `_`.
    ExpectedLambdaParameterName { found: TokenKind },
    /// A lambda arrow is not followed by a body expression.
    ExpectedLambdaBody { found: TokenKind },
    /// Polymorphic function type parameters are not followed by `=>`.
    ExpectedPolyFunctionArrow { found: TokenKind },
    /// A polymorphic function body is not a value-parameter function.
    InvalidPolyFunctionBody { found: TokenKind },
    /// An interpolated identifier is not followed by a string part.
    ExpectedInterpolatedStringPart { found: TokenKind },
    /// An interpolated pattern splice is missing its closing brace.
    ExpectedInterpolatedPatternSpliceCloseBrace { found: TokenKind },
}

/// Layout-delimited expression positions which may need a recovery node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutExpressionContext {
    /// The body introduced by `throw`.
    ThrowBody,
    /// The body introduced by `try`.
    TryBody,
    /// The body introduced by `catch`.
    CatchBody,
    /// The body introduced by `finally`.
    FinallyBody,
}

/// Expression shapes which the source parser does not permit as direct call targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionApplicationTarget {
    /// A block expression, which must be parenthesized before direct application.
    Block,
    /// A case-lambda expression, which must be parenthesized before application.
    CaseLambda,
    /// An already completed constructor application.
    ConstructorApplication,
}

impl ExpressionIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::MissingUsingArgument { .. }
            | Self::MissingArgument { .. }
            | Self::PrefixOperandMustShareLine { .. } => ParseDiagnosticKind::ExpectedExpression,
            Self::ExpectedInlineIfOrMatch { .. } | Self::MissingAssignmentRhs { .. } => {
                ParseDiagnosticKind::ExpectedExpression
            }
            Self::ExpectedSuperTypeQualifier { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ExpectedQuotedExpressionCloseBrace { .. }
            | Self::ExpectedQuotedTypeCloseBracket { .. }
            | Self::ExpectedQuoteBodyStart { .. }
            | Self::ExpectedPatternSpliceCloseBrace { .. }
            | Self::ExpectedExpressionSpliceCloseBrace { .. }
            | Self::ExpectedCaseLambdaCloseBrace { .. }
            | Self::ExpectedBlockCloseBrace { .. }
            | Self::ExpectedSelectorAfterSuper { .. }
            | Self::ExpectedSelectorAfterSuperDot { .. }
            | Self::ExpectedSelectorAfterDot { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ExpectedQuotedTypeDefinitionSeparator { .. }
            | Self::UnassignableAssignmentTarget { .. }
            | Self::LegacyWildcardSpliceNotFinal { .. }
            | Self::InvalidApplicationTarget { .. }
            | Self::ExpressionSuffixNoProgress { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::ExpectedExpressionAtCurrentToken { .. }
            | Self::ExpectedGuardExpression { .. } => ParseDiagnosticKind::ExpectedExpression,
            Self::MissingDoWhileBody { .. }
            | Self::MissingDoWhileCondition { .. }
            | Self::EmptyCatchHandler { .. }
            | Self::ExpectedControlFlowBranch { .. }
            | Self::ExpectedLayoutExpression { .. }
            | Self::ExpectedForEnumeratorExpression { .. }
            | Self::ExpectedForBodyExpression { .. }
            | Self::ContextFunctionRequiresParameter { .. }
            | Self::ExpectedLambdaParameter { .. }
            | Self::ExpectedLambdaParameterName { .. }
            | Self::ExpectedLambdaBody { .. }
            | Self::ExpectedInterpolatedStringPart { .. } => {
                ParseDiagnosticKind::ExpectedExpression
            }
            Self::ExpectedCatchCase { .. }
            | Self::ForGuardMissingGenerator { .. }
            | Self::ForMissingGenerator { .. }
            | Self::ExpectedMatchCase { .. } => ParseDiagnosticKind::ExpectedPattern,
            Self::ExpectedIfThen { .. }
            | Self::ExpectedWhileDo { .. }
            | Self::ExpectedDoWhileWhile { .. }
            | Self::ExpectedIndentedExpressionOutdent { .. }
            | Self::ExpectedCatchCloseBrace { .. }
            | Self::ExpectedCatchOutdent { .. }
            | Self::ExpectedCaseLambdaOutdent { .. }
            | Self::ExpectedIndentedBlockOutdent { .. }
            | Self::ExpectedForEnumeratorOutdent { .. }
            | Self::ExpectedForBodyKeyword { .. }
            | Self::ExpectedForEnumeratorOperator { .. }
            | Self::ExpectedCaseGeneratorOperator { .. }
            | Self::ExpectedMatchColonOutdent { .. }
            | Self::ExpectedMatchCloseBrace { .. }
            | Self::ExpectedMatchOutdent { .. }
            | Self::ExpectedMatchCaseRegion { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ExpectedLegacyLambdaArrow { .. }
            | Self::ExpectedLambdaArrow { .. }
            | Self::ExpectedLambdaParameterCloseParen { .. }
            | Self::ExpectedLambdaParameterSeparator { .. }
            | Self::ExpectedPolyFunctionArrow { .. }
            | Self::ExpectedInterpolatedPatternSpliceCloseBrace { .. } => {
                ParseDiagnosticKind::ExpectedToken
            }
            Self::InvalidPolyFunctionBody { .. } => ParseDiagnosticKind::UnexpectedToken,
            Self::ControlFlowNewlineNoProgress { .. } | Self::ForEnumeratorsNoProgress { .. } => {
                ParseDiagnosticKind::UnexpectedToken
            }
            Self::NonFinalArgumentSpread
            | Self::PostfixOperatorNoProgress { .. }
            | Self::InfixExpressionNoProgress { .. }
            | Self::MixedAssociativityOperators { .. } => ParseDiagnosticKind::UnexpectedToken,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::MissingUsingArgument { .. } => "parser.expression.missing_using_argument",
            Self::MissingArgument { .. } => "parser.expression.missing_argument",
            Self::NonFinalArgumentSpread => "parser.expression.nonfinal_argument_spread",
            Self::PostfixOperatorNoProgress { .. } => {
                "parser.expression.postfix_operator_no_progress"
            }
            Self::InfixExpressionNoProgress { .. } => "parser.expression.infix_no_progress",
            Self::MixedAssociativityOperators { .. } => {
                "parser.expression.mixed_operator_associativity"
            }
            Self::PrefixOperandMustShareLine { .. } => "parser.expression.prefix_operand_newline",
            Self::ExpectedInlineIfOrMatch { .. } => "parser.expression.expected_inline_if_or_match",
            Self::MissingAssignmentRhs { .. } => "parser.expression.missing_assignment_rhs",
            Self::UnassignableAssignmentTarget { .. } => {
                "parser.expression.unassignable_assignment_target"
            }
            Self::LegacyWildcardSpliceNotFinal { .. } => {
                "parser.expression.legacy_wildcard_splice_not_final"
            }
            Self::ExpectedQuotedExpressionCloseBrace { .. } => {
                "parser.expression.expected_quoted_expression_close_brace"
            }
            Self::ExpectedQuotedTypeCloseBracket { .. } => {
                "parser.expression.expected_quoted_type_close_bracket"
            }
            Self::ExpectedQuoteBodyStart { .. } => "parser.expression.expected_quote_body_start",
            Self::ExpectedQuotedTypeDefinitionSeparator { .. } => {
                "parser.expression.expected_quoted_type_definition_separator"
            }
            Self::ExpectedPatternSpliceCloseBrace { .. } => {
                "parser.expression.expected_pattern_splice_close_brace"
            }
            Self::ExpectedExpressionSpliceCloseBrace { .. } => {
                "parser.expression.expected_expression_splice_close_brace"
            }
            Self::ExpectedCaseLambdaCloseBrace { .. } => {
                "parser.expression.expected_case_lambda_close_brace"
            }
            Self::ExpectedBlockCloseBrace { .. } => "parser.expression.expected_block_close_brace",
            Self::ExpectedSuperTypeQualifier { .. } => {
                "parser.expression.expected_super_type_qualifier"
            }
            Self::ExpectedSelectorAfterSuper { .. } => {
                "parser.expression.expected_selector_after_super"
            }
            Self::ExpectedSelectorAfterSuperDot { .. } => {
                "parser.expression.expected_selector_after_super_dot"
            }
            Self::ExpectedSelectorAfterDot { .. } => {
                "parser.expression.expected_selector_after_dot"
            }
            Self::InvalidApplicationTarget { target, .. } => match target {
                ExpressionApplicationTarget::Block => {
                    "parser.expression.invalid_application_target.block"
                }
                ExpressionApplicationTarget::CaseLambda => {
                    "parser.expression.invalid_application_target.case_lambda"
                }
                ExpressionApplicationTarget::ConstructorApplication => {
                    "parser.expression.invalid_application_target.constructor_application"
                }
            },
            Self::ExpressionSuffixNoProgress { .. } => "parser.expression.suffix_no_progress",
            Self::ExpectedExpressionAtCurrentToken { .. } => {
                "parser.expression.expected_at_current_token"
            }
            Self::ExpectedGuardExpression { .. } => "parser.expression.expected_guard_expression",
            Self::ExpectedIfThen { .. } => "parser.expression.expected_if_then",
            Self::ExpectedWhileDo { .. } => "parser.expression.expected_while_do",
            Self::MissingDoWhileBody { .. } => "parser.expression.missing_do_while_body",
            Self::ExpectedDoWhileWhile { .. } => "parser.expression.expected_do_while_while",
            Self::MissingDoWhileCondition { .. } => "parser.expression.missing_do_while_condition",
            Self::EmptyCatchHandler { .. } => "parser.expression.empty_catch_handler",
            Self::ExpectedControlFlowBranch { .. } => {
                "parser.expression.expected_control_flow_branch"
            }
            Self::ExpectedLayoutExpression { context, .. } => match context {
                LayoutExpressionContext::ThrowBody => "parser.expression.expected_throw_body",
                LayoutExpressionContext::TryBody => "parser.expression.expected_try_body",
                LayoutExpressionContext::CatchBody => "parser.expression.expected_catch_body",
                LayoutExpressionContext::FinallyBody => "parser.expression.expected_finally_body",
            },
            Self::ExpectedIndentedExpressionOutdent { .. } => {
                "parser.expression.expected_layout_expression_outdent"
            }
            Self::ExpectedCatchCase { .. } => "parser.expression.expected_catch_case",
            Self::ExpectedCatchCloseBrace { .. } => "parser.expression.expected_catch_close_brace",
            Self::ExpectedCatchOutdent { .. } => "parser.expression.expected_catch_outdent",
            Self::ControlFlowNewlineNoProgress { .. } => {
                "parser.expression.control_newline_no_progress"
            }
            Self::ExpectedCaseLambdaOutdent { .. } => {
                "parser.expression.expected_case_lambda_outdent"
            }
            Self::ExpectedIndentedBlockOutdent { .. } => "parser.expression.expected_block_outdent",
            Self::ExpectedForEnumeratorOutdent { .. } => {
                "parser.expression.expected_for_enumerator_outdent"
            }
            Self::ExpectedForBodyKeyword { .. } => "parser.expression.expected_for_body_keyword",
            Self::ForGuardMissingGenerator { .. } => {
                "parser.expression.for_guard_missing_generator"
            }
            Self::ForEnumeratorsNoProgress { .. } => "parser.expression.for_enumerator_no_progress",
            Self::ForMissingGenerator { .. } => "parser.expression.for_missing_generator",
            Self::ExpectedForEnumeratorOperator { .. } => {
                "parser.expression.expected_for_enumerator_operator"
            }
            Self::ExpectedCaseGeneratorOperator { .. } => {
                "parser.expression.expected_case_generator_operator"
            }
            Self::ExpectedForEnumeratorExpression { .. } => {
                "parser.expression.expected_for_enumerator_expression"
            }
            Self::ExpectedForBodyExpression { .. } => {
                "parser.expression.expected_for_body_expression"
            }
            Self::ExpectedMatchColonOutdent { .. } => {
                "parser.expression.expected_match_colon_outdent"
            }
            Self::ExpectedMatchCloseBrace { .. } => "parser.expression.expected_match_close_brace",
            Self::ExpectedMatchOutdent { .. } => "parser.expression.expected_match_outdent",
            Self::ExpectedMatchCaseRegion { .. } => "parser.expression.expected_match_case_region",
            Self::ExpectedMatchCase { .. } => "parser.expression.expected_match_case",
            Self::ExpectedLegacyLambdaArrow { .. } => {
                "parser.expression.expected_legacy_lambda_arrow"
            }
            Self::ContextFunctionRequiresParameter { .. } => {
                "parser.expression.context_function_requires_parameter"
            }
            Self::ExpectedLambdaArrow { .. } => "parser.expression.expected_lambda_arrow",
            Self::ExpectedLambdaParameter { .. } => "parser.expression.expected_lambda_parameter",
            Self::ExpectedLambdaParameterCloseParen { .. } => {
                "parser.expression.expected_lambda_parameter_close_paren"
            }
            Self::ExpectedLambdaParameterSeparator { .. } => {
                "parser.expression.expected_lambda_parameter_separator"
            }
            Self::ExpectedLambdaParameterName { .. } => {
                "parser.expression.expected_lambda_parameter_name"
            }
            Self::ExpectedLambdaBody { .. } => "parser.expression.expected_lambda_body",
            Self::ExpectedPolyFunctionArrow { .. } => {
                "parser.expression.expected_poly_function_arrow"
            }
            Self::InvalidPolyFunctionBody { .. } => "parser.expression.invalid_poly_function_body",
            Self::ExpectedInterpolatedStringPart { .. } => {
                "parser.expression.expected_interpolated_string_part"
            }
            Self::ExpectedInterpolatedPatternSpliceCloseBrace { .. } => {
                "parser.expression.expected_interpolated_pattern_splice_close_brace"
            }
        }
    }
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
    /// No type operand starts at the current type-expression position.
    ExpectedTypeOperand { found: TokenKind },
    /// A singleton type expected a literal token.
    ExpectedLiteralType { found: TokenKind },
    /// An empty parenthesized type is not followed by a function arrow.
    EmptyParenthesizedType { found: TokenKind },
    /// A tuple type comma is not followed by another type.
    ExpectedTypeAfterTupleComma { found: TokenKind },
    /// A named tuple type is missing an element name.
    ExpectedNamedTupleElement { found: TokenKind },
    /// A named tuple element name is not followed by a colon.
    ExpectedNamedTupleElementColon { found: TokenKind },
    /// A named tuple comma is not followed by another element.
    ExpectedNamedTupleElementAfterComma { found: TokenKind },
    /// A type projection `#` is missing its member name.
    ExpectedTypeProjectionMember { found: TokenKind },
    /// A singleton type has no path before `.type`.
    ExpectedPathBeforeSingletonType { found: TokenKind },
    /// A simple type reference is missing its initial name.
    ExpectedSimpleType { found: TokenKind },
    /// A qualified type reference is missing a name after `.`.
    ExpectedTypeNameAfterDot { found: TokenKind },
    /// A braced legacy type splice is missing its closing brace.
    ExpectedLegacyTypeSpliceCloseBrace { found: TokenKind },
    /// A type splice appears in a quoted type, where Scala 3.9 rejects it.
    LegacyTypeSpliceUnsupported,
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
            | Self::InvalidCaptureReference { .. }
            | Self::ExpectedTypeOperand { .. }
            | Self::ExpectedLiteralType { .. }
            | Self::EmptyParenthesizedType { .. }
            | Self::ExpectedTypeAfterTupleComma { .. }
            | Self::ExpectedNamedTupleElement { .. }
            | Self::ExpectedNamedTupleElementAfterComma { .. }
            | Self::ExpectedTypeProjectionMember { .. }
            | Self::ExpectedPathBeforeSingletonType { .. }
            | Self::ExpectedSimpleType { .. }
            | Self::ExpectedTypeNameAfterDot { .. } => ParseDiagnosticKind::ExpectedType,
            Self::ExpectedNamedTupleElementColon { .. }
            | Self::ExpectedLegacyTypeSpliceCloseBrace { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::ClassLikeRefinementMemberNotAllowed { .. }
            | Self::ModifiedRefinementMemberNotAllowed { .. }
            | Self::UnsupportedRefinementMember { .. }
            | Self::RefinementMethodDefaultArgumentNotAllowed
            | Self::RefinementMemberRightHandSideNotAllowed
            | Self::ReadOnlyCaptureSuffixUnsupported
            | Self::LegacyTypeSpliceUnsupported => ParseDiagnosticKind::UnsupportedSyntax,
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
            Self::ExpectedTypeOperand { .. } => "parser.type.expected_operand",
            Self::ExpectedLiteralType { .. } => "parser.type.expected_literal_type",
            Self::EmptyParenthesizedType { .. } => "parser.type.empty_parenthesized_type",
            Self::ExpectedTypeAfterTupleComma { .. } => {
                "parser.type.expected_type_after_tuple_comma"
            }
            Self::ExpectedNamedTupleElement { .. } => "parser.type.expected_named_tuple_element",
            Self::ExpectedNamedTupleElementColon { .. } => {
                "parser.type.expected_named_tuple_element_colon"
            }
            Self::ExpectedNamedTupleElementAfterComma { .. } => {
                "parser.type.expected_named_tuple_element_after_comma"
            }
            Self::ExpectedTypeProjectionMember { .. } => "parser.type.expected_projection_member",
            Self::ExpectedPathBeforeSingletonType { .. } => {
                "parser.type.expected_path_before_singleton_type"
            }
            Self::ExpectedSimpleType { .. } => "parser.type.expected_simple_type",
            Self::ExpectedTypeNameAfterDot { .. } => "parser.type.expected_name_after_dot",
            Self::ExpectedLegacyTypeSpliceCloseBrace { .. } => {
                "parser.type.expected_legacy_splice_close_brace"
            }
            Self::LegacyTypeSpliceUnsupported => "parser.type.legacy_splice_unsupported",
        }
    }
}

/// Typed failures emitted while parsing Scala patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternIssue {
    /// The pattern fragment contains tokens after the pattern.
    TrailingInput { found: TokenKind },
    /// An alternative operator is not followed by a pattern.
    ExpectedPatternAfterAlternative { found: TokenKind },
    /// A binder marker is not preceded by an identifier pattern.
    ExpectedIdentifierBeforeBinder { found: TokenKind },
    /// Sequence-pattern syntax is only supported in extractor arguments.
    SequencePatternOutsideExtractorArguments { found: TokenKind },
    /// A sequence wildcard in extractor arguments is not preceded by a variable pattern.
    SequencePatternRequiresVariable { found: TokenKind },
    /// Equal-precedence pattern operators use conflicting associativities.
    MixedAssociativityOperators { left: Name, right: Name },
    /// Parsing an infix pattern did not advance the token source.
    InfixPatternNoProgress { found: TokenKind },
    /// A pattern selection dot is not followed by a selector.
    ExpectedSelectorAfterDot { found: TokenKind },
    /// An extractor argument comma is not followed by a pattern.
    ExpectedPatternAfterComma { found: TokenKind },
    /// A pattern was required at the current token.
    ExpectedPattern { found: TokenKind },
    /// The current pattern form is not implemented.
    UnsupportedPattern { found: TokenKind },
}

impl PatternIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::ExpectedPatternAfterAlternative { .. }
            | Self::ExpectedPatternAfterComma { .. }
            | Self::ExpectedPattern { .. } => ParseDiagnosticKind::ExpectedPattern,
            Self::ExpectedSelectorAfterDot { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::SequencePatternOutsideExtractorArguments { .. }
            | Self::SequencePatternRequiresVariable { .. }
            | Self::UnsupportedPattern { .. } => ParseDiagnosticKind::UnsupportedSyntax,
            Self::TrailingInput { .. }
            | Self::ExpectedIdentifierBeforeBinder { .. }
            | Self::MixedAssociativityOperators { .. }
            | Self::InfixPatternNoProgress { .. } => ParseDiagnosticKind::UnexpectedToken,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::TrailingInput { .. } => "parser.pattern.trailing_input",
            Self::ExpectedPatternAfterAlternative { .. } => {
                "parser.pattern.expected_after_alternative"
            }
            Self::ExpectedIdentifierBeforeBinder { .. } => {
                "parser.pattern.expected_identifier_before_binder"
            }
            Self::SequencePatternOutsideExtractorArguments { .. } => {
                "parser.pattern.sequence_outside_extractor_arguments"
            }
            Self::SequencePatternRequiresVariable { .. } => {
                "parser.pattern.sequence_requires_variable"
            }
            Self::MixedAssociativityOperators { .. } => "parser.pattern.mixed_associativity",
            Self::InfixPatternNoProgress { .. } => "parser.pattern.infix_no_progress",
            Self::ExpectedSelectorAfterDot { .. } => "parser.pattern.expected_selector_after_dot",
            Self::ExpectedPatternAfterComma { .. } => "parser.pattern.expected_after_comma",
            Self::ExpectedPattern { .. } => "parser.pattern.expected",
            Self::UnsupportedPattern { .. } => "parser.pattern.unsupported",
        }
    }
}

/// Typed failures emitted while parsing case clauses and their guards/bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseIssue {
    /// A case clause does not begin with the `case` keyword.
    ExpectedCaseKeyword { found: TokenKind },
    /// A case pattern and optional guard are not followed by `=>`.
    ExpectedCaseArrow { found: TokenKind },
    /// Parsing a case list did not advance the token source.
    CaseListNoProgress { found: TokenKind },
    /// An indented case body is not closed by an outdent.
    ExpectedCaseBodyOutdent { found: TokenKind },
}

impl CaseIssue {
    const fn kind(self) -> ParseDiagnosticKind {
        match self {
            Self::ExpectedCaseKeyword { .. }
            | Self::ExpectedCaseArrow { .. }
            | Self::ExpectedCaseBodyOutdent { .. } => ParseDiagnosticKind::ExpectedToken,
            Self::CaseListNoProgress { .. } => ParseDiagnosticKind::UnexpectedToken,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::ExpectedCaseKeyword { .. } => "parser.case.expected_keyword",
            Self::ExpectedCaseArrow { .. } => "parser.case.expected_arrow",
            Self::CaseListNoProgress { .. } => "parser.case.no_progress",
            Self::ExpectedCaseBodyOutdent { .. } => "parser.case.expected_body_outdent",
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
    /// A structured failure emitted while parsing a type parameter clause.
    TypeParameter(TypeParamIssue),
    /// A structured failure emitted while parsing a type definition.
    TypeDefinition(TypeDefinitionIssue),
    /// A structured failure emitted while parsing an expression.
    Expression(ExpressionIssue),
    /// A structured failure emitted while parsing a pattern.
    Pattern(PatternIssue),
    /// A structured failure emitted while parsing a case clause.
    Case(CaseIssue),
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
            Self::TypeParameter(issue) => issue.kind(),
            Self::TypeDefinition(issue) => issue.kind(),
            Self::Expression(issue) => issue.kind(),
            Self::Pattern(issue) => issue.kind(),
            Self::Case(issue) => issue.kind(),
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
            Self::TypeParameter(issue) => issue.code(),
            Self::TypeDefinition(issue) => issue.code(),
            Self::Expression(issue) => issue.code(),
            Self::Pattern(issue) => issue.code(),
            Self::Case(issue) => issue.code(),
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
            ParseIssue::TypeParameter(_) => None,
            ParseIssue::TypeDefinition(_) => None,
            ParseIssue::Expression(_) => None,
            ParseIssue::Pattern(_) => None,
            ParseIssue::Case(_) => None,
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
    fn expression_issue_codes_preserve_layout_context_without_messages() {
        let try_issue = ParseIssue::Expression(ExpressionIssue::ExpectedLayoutExpression {
            context: LayoutExpressionContext::TryBody,
            found: TokenKind::Eof,
        });
        let finally_issue = ParseIssue::Expression(ExpressionIssue::ExpectedLayoutExpression {
            context: LayoutExpressionContext::FinallyBody,
            found: TokenKind::Eof,
        });

        assert_eq!(try_issue.kind(), ParseDiagnosticKind::ExpectedExpression);
        assert_eq!(try_issue.code(), "parser.expression.expected_try_body");
        assert_eq!(
            finally_issue.kind(),
            ParseDiagnosticKind::ExpectedExpression
        );
        assert_eq!(
            finally_issue.code(),
            "parser.expression.expected_finally_body"
        );
        assert_ne!(try_issue.code(), finally_issue.code());
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

    #[test]
    fn pattern_and_case_issues_have_stable_codes_and_categories() {
        let source = SourceId::from_index(9);
        let range = TextRange::new(4, 4).expect("valid range");
        let pattern = ParseIssue::Pattern(PatternIssue::ExpectedPatternAfterAlternative {
            found: TokenKind::Eof,
        });
        let case = ParseIssue::Case(CaseIssue::ExpectedCaseArrow {
            found: TokenKind::Eof,
        });

        let pattern_diagnostic = ParseDiagnostic::with_issue(
            SourceSpan::new(source, Span::without_point(range)),
            pattern.clone(),
        );
        let case_diagnostic = ParseDiagnostic::with_issue(
            SourceSpan::new(source, Span::without_point(range)),
            case.clone(),
        );

        assert_eq!(pattern.code(), "parser.pattern.expected_after_alternative");
        assert_eq!(pattern.kind(), ParseDiagnosticKind::ExpectedPattern);
        assert_eq!(pattern_diagnostic.issue(), &pattern);
        assert_eq!(pattern_diagnostic.source(), source);
        assert_eq!(pattern_diagnostic.span(), range);
        assert_eq!(pattern_diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(pattern_diagnostic.legacy_message(), None);

        assert_eq!(case.code(), "parser.case.expected_arrow");
        assert_eq!(case.kind(), ParseDiagnosticKind::ExpectedToken);
        assert_eq!(case_diagnostic.issue(), &case);
        assert_eq!(case_diagnostic.source(), source);
        assert_eq!(case_diagnostic.span(), range);
        assert_eq!(case_diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(case_diagnostic.legacy_message(), None);
    }
}
