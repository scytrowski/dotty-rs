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

/// Explicit parser context carried through nested grammar calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseContext {
    pub location: Location,
    pub parse_kind: ParseKind,
    pub param_owner: Option<ParamOwner>,
}

impl Default for ParseContext {
    fn default() -> Self {
        Self {
            location: Location::Elsewhere,
            parse_kind: ParseKind::Expr,
            param_owner: None,
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
}
