/// Syntactic location that affects parser interpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    InParens,
    InArgs,
    InColonArg,
    InPattern,
    InGuard,
    InPatternArgs,
    InBlock,
    InPackageBody,
    Elsewhere,
}

/// Owner of a parameter clause currently being parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamOwner {
    Class,
    CaseClass,
    Def,
    Type,
    Hk,
    Given,
    ExtensionPrefix,
    ExtensionFollow,
}

/// Grammar category currently being parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseKind {
    Expr,
    Type,
    Pattern,
}

/// Feature policy used by grammar productions whose meaning is dialect- or
/// feature-dependent in Scala 3.9.0.
///
/// These switches deliberately do not affect lexical tokenization. A word
/// remains an identifier until a future grammar production consults the
/// policy in a context where that word has contextual meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParserFeatures {
    /// Enables capture-checking grammar for this compilation unit.
    ///
    /// The default is disabled. A caller may enable it explicitly, or the
    /// parser may enable it after a Scala 3.9 global language import. Capture
    /// syntax productions are added incrementally.
    pub capture_checking: bool,
    /// Enables `erased` definitions when that grammar is implemented.
    pub erased_definitions: bool,
    /// Enables `into` syntax when that grammar is implemented.
    pub into: bool,
    /// Enables legacy postfix operator syntax.
    pub postfix_ops: bool,
    /// Enables Scala's experimental single-case `match case` syntax.
    pub sub_cases: bool,
}

/// Explicit parser context carried through nested grammar calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseContext {
    pub location: Location,
    pub parse_kind: ParseKind,
    pub param_owner: Option<ParamOwner>,
    pub features: ParserFeatures,
    /// The delimiter owned by the expression block currently being parsed.
    ///
    /// This is separate from [`Location::InBlock`]: nested expression parsing
    /// needs to know whether its enclosing block ends at `}` or `Outdent`.
    pub block_end: Option<dotty_core::TokenKind>,
    /// Whether the current block is a case/catch body that also ends before
    /// the next `case` clause.
    pub case_body: bool,
    /// Whether the current template body belongs to an enum definition.
    pub enum_body: bool,
    /// Whether secondary constructors are allowed in the current template.
    pub secondary_constructor_allowed: bool,
}

impl Default for ParseContext {
    fn default() -> Self {
        Self {
            location: Location::Elsewhere,
            parse_kind: ParseKind::Expr,
            param_owner: None,
            features: ParserFeatures::default(),
            block_end: None,
            case_body: false,
            enum_body: false,
            secondary_constructor_allowed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_starts_elsewhere_in_expression_mode() {
        assert_eq!(
            ParseContext::default(),
            ParseContext {
                location: Location::Elsewhere,
                parse_kind: ParseKind::Expr,
                param_owner: None,
                features: ParserFeatures::default(),
                block_end: None,
                case_body: false,
                enum_body: false,
                secondary_constructor_allowed: false,
            }
        );
    }

    #[test]
    fn all_parser_locations_are_distinct() {
        let locations = [
            Location::InParens,
            Location::InArgs,
            Location::InColonArg,
            Location::InPattern,
            Location::InGuard,
            Location::InPatternArgs,
            Location::InBlock,
            Location::Elsewhere,
        ];

        for (index, location) in locations.iter().enumerate() {
            for (other_index, other) in locations.iter().enumerate() {
                assert_eq!(index == other_index, location == other);
            }
        }
    }

    #[test]
    fn all_parameter_owners_are_distinct() {
        let owners = [
            ParamOwner::Class,
            ParamOwner::CaseClass,
            ParamOwner::Def,
            ParamOwner::Type,
            ParamOwner::Hk,
            ParamOwner::Given,
            ParamOwner::ExtensionPrefix,
            ParamOwner::ExtensionFollow,
        ];

        for (index, owner) in owners.iter().enumerate() {
            for (other_index, other) in owners.iter().enumerate() {
                assert_eq!(index == other_index, owner == other);
            }
        }
    }

    #[test]
    fn parse_kinds_are_expression_type_and_pattern() {
        assert_ne!(ParseKind::Expr, ParseKind::Type);
        assert_ne!(ParseKind::Type, ParseKind::Pattern);
        assert_ne!(ParseKind::Expr, ParseKind::Pattern);
    }

    #[test]
    fn feature_policy_defaults_to_disabled() {
        assert_eq!(
            ParserFeatures::default(),
            ParserFeatures {
                capture_checking: false,
                erased_definitions: false,
                into: false,
                postfix_ops: false,
                sub_cases: false,
            }
        );
    }

    #[test]
    fn feature_policy_flags_are_independent() {
        let features = ParserFeatures {
            capture_checking: true,
            erased_definitions: false,
            into: true,
            postfix_ops: false,
            sub_cases: false,
        };

        assert!(features.capture_checking);
        assert!(!features.erased_definitions);
        assert!(features.into);
        assert!(!features.postfix_ops);
        assert!(!features.sub_cases);
    }

    #[test]
    fn enum_body_is_disabled_by_default() {
        assert!(!ParseContext::default().enum_body);
    }
}
