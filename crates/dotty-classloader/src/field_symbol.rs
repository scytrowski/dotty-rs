use dotty_classfile::access_flags::FieldAccessFlags;
use dotty_classfile::descriptor::FieldType;

/// A loaded class's field: its name, access flags, and declared type
/// (JVMS §4.5).
///
/// `field_type` is exactly what `dotty-classfile` already parses from the
/// field's descriptor (JVMS §4.3.2) — an `Object`/`Array` entry's class
/// name stays an internal-form string, not a resolved [`crate::ClassRef`].
/// Resolving those names into loaded classes is a later milestone
/// (`docs/classloader.md` §3/§9).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSymbol {
    name: String,
    flags: FieldAccessFlags,
    field_type: FieldType,
}

impl FieldSymbol {
    pub fn new(name: String, flags: FieldAccessFlags, field_type: FieldType) -> Self {
        Self {
            name,
            flags,
            field_type,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn flags(&self) -> FieldAccessFlags {
        self.flags
    }

    pub fn field_type(&self) -> &FieldType {
        &self.field_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_name_flags_and_field_type() {
        let symbol = FieldSymbol::new(
            "ANSWER".to_owned(),
            FieldAccessFlags(0x0019),
            FieldType::Int,
        );

        assert_eq!(symbol.name(), "ANSWER");
        assert_eq!(symbol.flags(), FieldAccessFlags(0x0019));
        assert_eq!(symbol.field_type(), &FieldType::Int);
    }
}
