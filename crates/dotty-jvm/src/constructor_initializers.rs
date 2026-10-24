//! Constructor-local lowering for canonical field initializers.

use std::collections::HashSet;

use dotty_classfile::bytecode::CodeBuilder;
use dotty_core::ast::{TreeKind, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::{Constant, TermRefTarget, Type};

use crate::{
    JVM_EXPRESSION_DEPTH_LIMIT, JVM_EXPRESSION_NODE_BUDGET, JvmClassIdentity,
    JvmClassInitializationPlan, JvmCodegenError, JvmCompilationUnit, JvmConstructorMetadata,
    JvmFieldMetadata, JvmIntIntrinsic, JvmType, resolve_boolean_intrinsic, resolve_int_intrinsic,
};

/// A verified source-ordered list of constructor-local field initializer fragments.
///
/// This plan does not contain the superclass constructor call or a return and
/// is not a complete JVM constructor. Whole-class emission calls the
/// validated direct parent constructor before appending these instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmConstructorFieldInitializerPlan {
    class: SymbolId,
    constructor: JvmConstructorMetadata,
    owner_internal_name: String,
    accessors: Vec<JvmConstructorAccessorInitializer>,
    fields: Vec<JvmConstructorFieldInitializer>,
}

/// Checked constructor parameter copy into one accessor's private JVM storage.
///
/// Unlike an ordinary field initializer, this has no source RHS tree. The
/// exact constructor-owned formal and its validated local layout are retained
/// directly from accessor metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmConstructorAccessorInitializer {
    field: SymbolId,
    parameter: SymbolId,
    source_ordinal: usize,
    slot: u16,
    width: u8,
    value_type: JvmType,
    field_name: String,
    field_descriptor: String,
}

/// One field initializer and its checked instruction sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmConstructorFieldInitializer {
    field: SymbolId,
    initializer: TreeId<Typed>,
    instructions: Vec<JvmConstructorInitializerInstruction>,
}

/// Typed instruction plan for the currently supported constructor initializer subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmConstructorInitializerInstruction {
    /// Push the current instance (`aload_0`).
    LoadThis,
    /// Push an int-like constant using the class-file builder's compact encoding.
    Constant(i32),
    /// Push a signed category-two Long constant without narrowing it.
    LongConstant(i64),
    /// Push an exact IEEE-754 Float bit pattern.
    FloatBits(u32),
    /// Push an exact IEEE-754 Double bit pattern.
    DoubleBits(u64),
    /// Allocate one planned class for a reference-valued field initializer.
    NewObject {
        /// Checked JVM internal name of the allocated class.
        owner: String,
    },
    /// Duplicate the new object's uninitialized allocation-site value.
    Dup,
    /// Load an exact constructor-owned primitive or planned-reference parameter.
    LoadParameter {
        /// Canonical constructor-owned parameter identity.
        symbol: SymbolId,
        /// Validated JVM local slot, after receiver slot zero.
        slot: u16,
        /// Exact projected JVM type of the parameter.
        value_type: JvmType,
    },
    /// Add two primitive Int operands.
    IntAdd,
    /// Invert a canonical Boolean value.
    BooleanNot,
    /// Initialize the exact allocation with its checked primary constructor.
    InvokeSpecial {
        /// Checked JVM internal name owning the selected constructor.
        owner: String,
        /// Exact descriptor of the selected constructor.
        descriptor: String,
    },
    /// Read a previously initialized current-class field.
    GetField {
        /// Canonical field identity.
        field: SymbolId,
        /// Validated field name and descriptor from class metadata.
        name: String,
        descriptor: String,
    },
    /// Store the completed initializer result to its canonical field.
    PutField {
        /// Canonical field identity.
        field: SymbolId,
        /// Validated field name and descriptor from class metadata.
        name: String,
        descriptor: String,
    },
}

impl JvmConstructorFieldInitializerPlan {
    /// Canonical class whose fields and constructor were validated.
    pub const fn class(&self) -> SymbolId {
        self.class
    }

    /// Validated constructor metadata, including ordered parameter slots.
    pub const fn constructor(&self) -> &JvmConstructorMetadata {
        &self.constructor
    }

    /// Field initializers in the frontend's source declaration order.
    pub fn fields(&self) -> &[JvmConstructorFieldInitializer] {
        &self.fields
    }

    /// Constructor accessors in source parameter order, before class-body
    /// field initializers.
    pub fn accessors(&self) -> &[JvmConstructorAccessorInitializer] {
        &self.accessors
    }

    /// Appends only the planned field initializer instructions to `code`.
    ///
    /// The caller is responsible for placing this sequence after the
    /// validated direct-parent constructor call and for adding a constructor
    /// return afterward. Whole-class emission performs both steps.
    pub fn append_to(&self, code: &mut CodeBuilder) -> Result<(), JvmCodegenError> {
        for accessor in &self.accessors {
            code.aload(0)
                .map_err(|error| JvmCodegenError::ConstructorCodeBuilder {
                    class: self.class,
                    error,
                })?;
            let loaded = match accessor.value_type {
                JvmType::Int | JvmType::Boolean => code.iload(usize::from(accessor.slot)),
                JvmType::Long => code.lload(usize::from(accessor.slot)),
                JvmType::Float => code.fload(usize::from(accessor.slot)),
                JvmType::Double => code.dload(usize::from(accessor.slot)),
                JvmType::Reference(_) => code.aload(usize::from(accessor.slot)),
            };
            loaded.map_err(|error| JvmCodegenError::ConstructorCodeBuilder {
                class: self.class,
                error,
            })?;
            code.putfield(
                self.owner_internal_name.clone(),
                accessor.field_name.clone(),
                accessor.field_descriptor.clone(),
            )
            .map_err(|error| JvmCodegenError::ConstructorCodeBuilder {
                class: self.class,
                error,
            })?;
        }
        for field in &self.fields {
            for instruction in &field.instructions {
                let result = match instruction {
                    JvmConstructorInitializerInstruction::LoadThis => code.aload(0),
                    JvmConstructorInitializerInstruction::Constant(value) => code.iconst(*value),
                    JvmConstructorInitializerInstruction::LongConstant(value) => {
                        code.lconst(*value)
                    }
                    JvmConstructorInitializerInstruction::FloatBits(bits) => {
                        code.fconst_bits(*bits)
                    }
                    JvmConstructorInitializerInstruction::DoubleBits(bits) => {
                        code.dconst_bits(*bits)
                    }
                    JvmConstructorInitializerInstruction::NewObject { owner } => {
                        code.new_object(owner.clone())
                    }
                    JvmConstructorInitializerInstruction::Dup => code.dup(),
                    JvmConstructorInitializerInstruction::LoadParameter {
                        slot,
                        value_type,
                        ..
                    } => match value_type {
                        JvmType::Int | JvmType::Boolean => code.iload(usize::from(*slot)),
                        JvmType::Reference(_) => code.aload(usize::from(*slot)),
                        JvmType::Long => code.lload(usize::from(*slot)),
                        JvmType::Float => code.fload(usize::from(*slot)),
                        JvmType::Double => code.dload(usize::from(*slot)),
                    },
                    JvmConstructorInitializerInstruction::IntAdd => code.iadd(),
                    JvmConstructorInitializerInstruction::BooleanNot => {
                        code.iconst(1).and_then(|code| code.ixor())
                    }
                    JvmConstructorInitializerInstruction::InvokeSpecial { owner, descriptor } => {
                        code.invokespecial(owner.clone(), "<init>", descriptor.clone())
                    }
                    JvmConstructorInitializerInstruction::GetField {
                        name, descriptor, ..
                    } => code.aload(0).and_then(|code| {
                        code.getfield(
                            self.owner_internal_name.clone(),
                            name.clone(),
                            descriptor.clone(),
                        )
                    }),
                    JvmConstructorInitializerInstruction::PutField {
                        name, descriptor, ..
                    } => code.putfield(
                        self.owner_internal_name.clone(),
                        name.clone(),
                        descriptor.clone(),
                    ),
                };
                result.map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: field.initializer,
                    error,
                })?;
            }
        }
        Ok(())
    }
}

impl JvmConstructorFieldInitializer {
    /// Canonical field receiving this initializer.
    pub const fn field(&self) -> SymbolId {
        self.field
    }

    /// Exact typed initializer root associated with the field plan.
    pub const fn initializer(&self) -> TreeId<Typed> {
        self.initializer
    }

    /// Checked instruction sequence, beginning with `LoadThis` and ending with `PutField`.
    pub fn instructions(&self) -> &[JvmConstructorInitializerInstruction] {
        &self.instructions
    }
}

impl JvmConstructorAccessorInitializer {
    /// Canonical source field that owns this backing storage.
    pub const fn field(&self) -> SymbolId {
        self.field
    }

    /// Exact constructor-owned formal copied into the backing field.
    pub const fn parameter(&self) -> SymbolId {
        self.parameter
    }

    /// Flattened source parameter ordinal, independent of JVM slot width.
    pub const fn source_ordinal(&self) -> usize {
        self.source_ordinal
    }

    /// First JVM local slot occupied by the formal.
    pub const fn slot(&self) -> u16 {
        self.slot
    }

    /// JVM local-slot width of the formal (one or two).
    pub const fn width(&self) -> u8 {
        self.width
    }

    /// Exact projected value type stored in the accessor backing field.
    pub const fn value_type(&self) -> JvmType {
        self.value_type
    }

    /// Validated private JVM backing field name.
    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    /// Validated JVM backing field descriptor.
    pub fn field_descriptor(&self) -> &str {
        &self.field_descriptor
    }
}

pub(crate) fn append_direct_super_constructor_call(
    unit: &JvmCompilationUnit<'_>,
    class: SymbolId,
    definitions: &Definitions,
    code: &mut CodeBuilder,
) -> Result<bool, JvmCodegenError> {
    unit.validate()?;
    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == class)
        .ok_or(JvmCodegenError::MissingClassPlan { class })?;
    let proof = match &class_plan.initialization {
        JvmClassInitializationPlan::PrimaryConstructor(constructor) => {
            constructor.direct_super_constructor()
        }
        JvmClassInitializationPlan::ImplicitNoArg | JvmClassInitializationPlan::Unsupported => None,
    };
    let Some(proof) = proof else {
        return Ok(false);
    };
    let all_metadata = unit.metadata(definitions)?;
    let child_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == class)
        .ok_or(JvmCodegenError::MissingClassPlan { class })?;
    let child_constructor = child_metadata
        .constructor()
        .ok_or(JvmCodegenError::ConstructorInitializerContextUnavailable { class })?;
    let parent_metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == proof.parent_class())
        .ok_or(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(proof.parent_class()),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        })?;
    let parent_constructor = parent_metadata
        .constructor()
        .filter(|constructor| constructor.symbol() == proof.parent_constructor())
        .ok_or(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(proof.parent_class()),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        })?;
    if !parent_constructor.access_flags().is_public()
        || parent_constructor.parameters().len() != proof.arguments().len()
    {
        return Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(proof.parent_class()),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        });
    }
    let class_identities = all_metadata
        .iter()
        .map(|metadata| metadata.identity().clone())
        .collect::<Vec<_>>();
    let mut context = ConstructorInitializerContext {
        unit,
        store: unit.store,
        typed_ast: unit.typed_ast,
        definitions,
        class_identities: &class_identities,
        class,
        constructor: child_constructor,
        fields: child_metadata.fields(),
        initialized_fields: HashSet::new(),
        current_field: class,
        node_visits: 0,
    };
    enum EncodedArgument {
        IntConstant(i32),
        LongConstant(i64),
        FloatBits(u32),
        DoubleBits(u64),
        Parameter { slot: u16, value_type: JvmType },
    }
    let mut encoded_arguments = Vec::with_capacity(proof.arguments().len());
    for (binding, formal) in proof
        .arguments()
        .iter()
        .zip(parent_constructor.parameters())
    {
        let argument = unit.typed_ast.try_get(binding.typed).ok_or(
            JvmCodegenError::UnsupportedJvmConstructorArgument {
                tree: proof.typed_application(),
                argument: binding.typed,
            },
        )?;
        if !matches!(argument.kind, TreeKind::Literal(_) | TreeKind::Ident(_)) {
            return Err(JvmCodegenError::UnsupportedJvmConstructorArgument {
                tree: proof.typed_application(),
                argument: binding.typed,
            });
        }
        context.node_visits = 0;
        let mut instructions = Vec::new();
        let found = context.lower_expression(binding.typed, &mut instructions, 0)?;
        if found != formal.ty() {
            return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                tree: proof.typed_application(),
                argument: binding.typed,
                expected: formal.ty(),
                found: Some(found),
            });
        }
        match instructions.as_slice() {
            [JvmConstructorInitializerInstruction::Constant(value)] => {
                encoded_arguments.push(EncodedArgument::IntConstant(*value));
            }
            [JvmConstructorInitializerInstruction::LongConstant(value)] => {
                if found != JvmType::Long {
                    return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                        tree: proof.typed_application(),
                        argument: binding.typed,
                        expected: formal.ty(),
                        found: Some(found),
                    });
                }
                encoded_arguments.push(EncodedArgument::LongConstant(*value));
            }
            [JvmConstructorInitializerInstruction::FloatBits(bits)] => {
                if found != JvmType::Float {
                    return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                        tree: proof.typed_application(),
                        argument: binding.typed,
                        expected: formal.ty(),
                        found: Some(found),
                    });
                }
                encoded_arguments.push(EncodedArgument::FloatBits(*bits));
            }
            [JvmConstructorInitializerInstruction::DoubleBits(bits)] => {
                if found != JvmType::Double {
                    return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                        tree: proof.typed_application(),
                        argument: binding.typed,
                        expected: formal.ty(),
                        found: Some(found),
                    });
                }
                encoded_arguments.push(EncodedArgument::DoubleBits(*bits));
            }
            [
                JvmConstructorInitializerInstruction::LoadParameter {
                    slot, value_type, ..
                },
            ] => {
                if *value_type != found {
                    return Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
                        tree: proof.typed_application(),
                        argument: binding.typed,
                        expected: formal.ty(),
                        found: Some(*value_type),
                    });
                }
                encoded_arguments.push(EncodedArgument::Parameter {
                    slot: *slot,
                    value_type: *value_type,
                });
            }
            _ => {
                return Err(JvmCodegenError::UnsupportedJvmConstructorArgument {
                    tree: proof.typed_application(),
                    argument: binding.typed,
                });
            }
        }
    }

    code.aload(0)
        .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    for argument in encoded_arguments {
        let result = match argument {
            EncodedArgument::IntConstant(value) => code.iconst(value),
            EncodedArgument::LongConstant(value) => code.lconst(value),
            EncodedArgument::FloatBits(bits) => code.fconst_bits(bits),
            EncodedArgument::DoubleBits(bits) => code.dconst_bits(bits),
            EncodedArgument::Parameter { slot, value_type } => match value_type {
                JvmType::Int | JvmType::Boolean => code.iload(usize::from(slot)),
                JvmType::Reference(_) => code.aload(usize::from(slot)),
                JvmType::Long => code.lload(usize::from(slot)),
                JvmType::Float => code.fload(usize::from(slot)),
                JvmType::Double => code.dload(usize::from(slot)),
            },
        };
        result.map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    }
    code.invokespecial(
        parent_constructor.owner_internal_name().to_owned(),
        "<init>".to_owned(),
        parent_constructor.descriptor().as_str().to_owned(),
    )
    .map_err(|error| JvmCodegenError::ConstructorCodeBuilder { class, error })?;
    Ok(true)
}

struct ConstructorInitializerContext<'a> {
    unit: &'a JvmCompilationUnit<'a>,
    store: &'a SemanticStore,
    typed_ast: &'a TypedAst,
    definitions: &'a Definitions,
    class_identities: &'a [JvmClassIdentity],
    class: SymbolId,
    constructor: &'a JvmConstructorMetadata,
    fields: &'a [JvmFieldMetadata],
    initialized_fields: HashSet<SymbolId>,
    current_field: SymbolId,
    node_visits: usize,
}

pub(super) fn lower(
    unit: &JvmCompilationUnit<'_>,
    class: SymbolId,
    definitions: &Definitions,
) -> Result<JvmConstructorFieldInitializerPlan, JvmCodegenError> {
    unit.validate()?;
    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == class)
        .ok_or(JvmCodegenError::MissingClassPlan { class })?;
    if let Some(field) = class_plan.fields.iter().find(|field| {
        matches!(
            field.source_visibility,
            crate::JvmFieldSourceVisibility::PlainPublic(_)
        )
    }) {
        return Err(JvmCodegenError::FieldEmissionUnsupported {
            class,
            field: field.field,
        });
    }
    if !matches!(
        class_plan.initialization,
        JvmClassInitializationPlan::PrimaryConstructor(_)
    ) {
        return Err(JvmCodegenError::ConstructorInitializerContextUnavailable { class });
    }
    let all_metadata = unit.metadata(definitions)?;
    let metadata = all_metadata
        .iter()
        .find(|metadata| metadata.identity().symbol() == class)
        .ok_or(JvmCodegenError::MissingClassPlan { class })?;
    let constructor = metadata
        .constructor()
        .cloned()
        .ok_or(JvmCodegenError::ConstructorInitializerContextUnavailable { class })?;
    let class_identities = all_metadata
        .iter()
        .map(|class| class.identity().clone())
        .collect::<Vec<_>>();
    let mut context = ConstructorInitializerContext {
        unit,
        store: unit.store,
        typed_ast: unit.typed_ast,
        definitions,
        class_identities: &class_identities,
        class,
        constructor: &constructor,
        fields: metadata.fields(),
        initialized_fields: HashSet::new(),
        current_field: class,
        node_visits: 0,
    };
    let mut lowered_accessors = Vec::with_capacity(metadata.accessors().len());
    let mut accessor_fields = HashSet::new();
    let mut accessor_parameters = HashSet::new();
    let mut previous_ordinal = None;
    for accessor in metadata.accessors() {
        let field = accessor.source_field();
        let parameter = accessor.constructor_parameter();
        let invalid = |issue| JvmCodegenError::InvalidJvmAccessorInitializer {
            class,
            field,
            parameter,
            issue,
        };
        if accessor.owner() != class
            || accessor.binding().constructor() != constructor.symbol()
            || !accessor_fields.insert(field)
            || !accessor_parameters.insert(parameter)
            || previous_ordinal.is_some_and(|ordinal| ordinal >= accessor.source_ordinal())
        {
            return Err(invalid(
                crate::JvmAccessorInitializerIssue::DuplicateOrUnorderedBinding,
            ));
        }
        previous_ordinal = Some(accessor.source_ordinal());

        let local = constructor
            .parameters()
            .iter()
            .find(|local| local.symbol() == parameter)
            .ok_or_else(|| invalid(crate::JvmAccessorInitializerIssue::MissingParameter))?;
        if constructor.parameters().get(accessor.source_ordinal()) != Some(local) {
            return Err(invalid(
                crate::JvmAccessorInitializerIssue::SourceOrdinalMismatch,
            ));
        }
        if local.ty() != accessor.value_type() {
            return Err(invalid(
                crate::JvmAccessorInitializerIssue::ValueTypeMismatch,
            ));
        }
        let expected_width = match accessor.value_type() {
            JvmType::Long | JvmType::Double => 2,
            JvmType::Int | JvmType::Boolean | JvmType::Float | JvmType::Reference(_) => 1,
        };
        if local.width() != expected_width {
            return Err(invalid(
                crate::JvmAccessorInitializerIssue::SlotWidthMismatch,
            ));
        }
        lowered_accessors.push(JvmConstructorAccessorInitializer {
            field,
            parameter,
            source_ordinal: accessor.source_ordinal(),
            slot: local.slot(),
            width: local.width(),
            value_type: accessor.value_type(),
            field_name: accessor.field_name().to_owned(),
            field_descriptor: accessor.field_descriptor().to_owned(),
        });
    }
    let mut lowered_fields = Vec::with_capacity(metadata.fields().len());
    for field in metadata.fields() {
        let initializer = field.initializer();
        context.current_field = field.symbol();
        context.node_visits = 0;
        let expected = field_value_type(field);
        let mut instructions = vec![JvmConstructorInitializerInstruction::LoadThis];
        let found = context.lower_expression(initializer, &mut instructions, 0)?;
        if !crate::reference::is_assignable(Some(unit), found, expected, definitions)? {
            return Err(JvmCodegenError::FieldInitializerTypeMismatch {
                field: field.symbol(),
                initializer,
                expected,
                found: Some(found),
            });
        }
        instructions.push(JvmConstructorInitializerInstruction::PutField {
            field: field.symbol(),
            name: field.name().to_owned(),
            descriptor: field.descriptor().to_owned(),
        });
        lowered_fields.push(JvmConstructorFieldInitializer {
            field: field.symbol(),
            initializer,
            instructions,
        });
        context.initialized_fields.insert(field.symbol());
    }

    Ok(JvmConstructorFieldInitializerPlan {
        class,
        constructor,
        owner_internal_name: metadata.identity().internal_name().to_owned(),
        accessors: lowered_accessors,
        fields: lowered_fields,
    })
}

impl ConstructorInitializerContext<'_> {
    fn lower_expression(
        &mut self,
        tree_id: TreeId<Typed>,
        instructions: &mut Vec<JvmConstructorInitializerInstruction>,
        depth: usize,
    ) -> Result<JvmType, JvmCodegenError> {
        let tree =
            self.typed_ast
                .try_get(tree_id)
                .ok_or(JvmCodegenError::UnknownFieldInitializer {
                    field: self.current_field,
                    initializer: tree_id,
                })?;
        if depth >= JVM_EXPRESSION_DEPTH_LIMIT {
            return Err(JvmCodegenError::ExpressionDepthExceeded {
                tree: tree_id,
                position: tree.position,
                limit: JVM_EXPRESSION_DEPTH_LIMIT,
            });
        }
        if self.node_visits >= JVM_EXPRESSION_NODE_BUDGET {
            return Err(JvmCodegenError::ExpressionNodeBudgetExceeded {
                tree: tree_id,
                limit: JVM_EXPRESSION_NODE_BUDGET,
            });
        }
        self.node_visits += 1;
        crate::expression::validate_tree_type(self.store, tree_id, tree.ty)?;

        match &tree.kind {
            TreeKind::Literal(literal) => {
                if let Constant::FloatBits(bits) = literal.value {
                    let found = self.tree_value_type(tree_id, tree.ty)?;
                    let constant_matches = match self.store.types.try_get(tree.ty) {
                        Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                        _ => found == JvmType::Float,
                    };
                    if found != JvmType::Float || !constant_matches {
                        return Err(JvmCodegenError::LiteralTypeMismatch {
                            tree: tree_id,
                            value: JvmType::Float,
                            typed: Some(found),
                        });
                    }
                    instructions.push(JvmConstructorInitializerInstruction::FloatBits(bits));
                    return Ok(JvmType::Float);
                }
                if let Constant::DoubleBits(bits) = literal.value {
                    let found = self.tree_value_type(tree_id, tree.ty)?;
                    let constant_matches = match self.store.types.try_get(tree.ty) {
                        Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                        _ => found == JvmType::Double,
                    };
                    if found != JvmType::Double || !constant_matches {
                        return Err(JvmCodegenError::LiteralTypeMismatch {
                            tree: tree_id,
                            value: JvmType::Double,
                            typed: Some(found),
                        });
                    }
                    instructions.push(JvmConstructorInitializerInstruction::DoubleBits(bits));
                    return Ok(JvmType::Double);
                }
                if let Constant::Long(value) = literal.value {
                    let found = self.tree_value_type(tree_id, tree.ty)?;
                    let constant_matches = match self.store.types.try_get(tree.ty) {
                        Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                        _ => found == JvmType::Long,
                    };
                    if found != JvmType::Long || !constant_matches {
                        return Err(JvmCodegenError::LiteralTypeMismatch {
                            tree: tree_id,
                            value: JvmType::Long,
                            typed: Some(found),
                        });
                    }
                    instructions.push(JvmConstructorInitializerInstruction::LongConstant(value));
                    return Ok(JvmType::Long);
                }
                let (value, value_type) = match literal.value {
                    Constant::Int(value) => (value, JvmType::Int),
                    Constant::Boolean(value) => (i32::from(value), JvmType::Boolean),
                    _ => return self.unsupported(tree_id, "unsupported literal type"),
                };
                let found = self.tree_value_type(tree_id, tree.ty)?;
                let constant_matches = match self.store.types.try_get(tree.ty) {
                    Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                    _ => found == value_type,
                };
                if !constant_matches || found != value_type {
                    return Err(JvmCodegenError::LiteralTypeMismatch {
                        tree: tree_id,
                        value: value_type,
                        typed: Some(found),
                    });
                }
                instructions.push(JvmConstructorInitializerInstruction::Constant(value));
                Ok(value_type)
            }
            TreeKind::Ident(_) => {
                let symbol = self.term_ref_symbol(tree_id, tree.ty)?;
                self.lower_reference(tree_id, symbol, None, instructions)
            }
            TreeKind::Select(select) => {
                let boolean_intrinsic = resolve_boolean_intrinsic(
                    self.store,
                    self.typed_ast,
                    tree_id,
                    self.definitions,
                )
                .map_err(|error| JvmCodegenError::BooleanIntrinsicResolution {
                    tree: tree_id,
                    error,
                })?;
                if boolean_intrinsic == Some(crate::JvmBooleanIntrinsic::Not) {
                    let found = self.lower_expression(select.qualifier, instructions, depth + 1)?;
                    if found != JvmType::Boolean {
                        return self.type_mismatch(tree_id, JvmType::Boolean, found);
                    }
                    instructions.push(JvmConstructorInitializerInstruction::BooleanNot);
                    return Ok(JvmType::Boolean);
                }
                let symbol = self.term_ref_symbol(tree_id, tree.ty)?;
                self.lower_reference(tree_id, symbol, Some(select.qualifier), instructions)
            }
            TreeKind::Apply(application) => {
                if self.is_constructor_application(tree_id, application)? {
                    return self.lower_new_reference_initializer(
                        tree_id,
                        application,
                        instructions,
                        depth,
                    );
                }
                let intrinsic =
                    resolve_int_intrinsic(self.store, self.typed_ast, tree_id, self.definitions)
                        .map_err(|error| JvmCodegenError::IntIntrinsicResolution {
                            tree: tree_id,
                            error,
                        })?;
                if intrinsic != Some(JvmIntIntrinsic::IntAdd) {
                    return self.unsupported(tree_id, "non-approved constructor application");
                }
                let function = self.typed_ast.try_get(application.function).ok_or(
                    JvmCodegenError::UnsupportedConstructorFieldInitializer {
                        field: self.current_field,
                        tree: tree_id,
                        node: "missing intrinsic function tree",
                    },
                )?;
                let TreeKind::Select(select) = &function.kind else {
                    return self.unsupported(tree_id, "unsupported Int intrinsic function shape");
                };
                let [argument] = application.args.as_slice() else {
                    return self.unsupported(tree_id, "unsupported Int intrinsic arity");
                };
                for operand in [select.qualifier, *argument] {
                    let found = self.lower_expression(operand, instructions, depth + 1)?;
                    if found != JvmType::Int {
                        return self.type_mismatch(tree_id, JvmType::Int, found);
                    }
                }
                instructions.push(JvmConstructorInitializerInstruction::IntAdd);
                Ok(JvmType::Int)
            }
            TreeKind::Block(block) if block.stats.is_empty() => {
                let found = self.lower_expression(block.expr, instructions, depth + 1)?;
                let declared = self.tree_value_type(tree_id, tree.ty)?;
                if found != declared {
                    return self.type_mismatch(tree_id, declared, found);
                }
                Ok(found)
            }
            _ => self.unsupported(tree_id, typed_node_name(&tree.kind)),
        }
    }

    fn lower_reference(
        &mut self,
        tree: TreeId<Typed>,
        symbol: SymbolId,
        receiver: Option<TreeId<Typed>>,
        instructions: &mut Vec<JvmConstructorInitializerInstruction>,
    ) -> Result<JvmType, JvmCodegenError> {
        if !self.store.symbols.contains(symbol) {
            return Err(JvmCodegenError::UnknownFieldInitializerSymbol {
                field: self.current_field,
                initializer: tree,
                symbol,
            });
        }
        let symbol_info = self.store.symbols.get(symbol);
        if let Some(parameter) = self
            .constructor
            .parameters()
            .iter()
            .find(|parameter| parameter.symbol() == symbol)
        {
            if receiver.is_some() {
                return self.unsupported(tree, "qualified constructor parameter reference");
            }
            let Some(Type::TermRef { prefix, target }) = self
                .typed_ast
                .try_get(tree)
                .and_then(|tree| self.store.types.try_get(tree.ty))
            else {
                return self.unsupported(
                    tree,
                    "constructor parameter reference without a term prefix",
                );
            };
            if *prefix != self.definitions.no_prefix || *target != TermRefTarget::Symbol(symbol) {
                return self.unsupported(
                    tree,
                    "constructor parameter reference with a noncanonical prefix",
                );
            }
            if symbol_info.kind != SymbolKind::Parameter
                || symbol_info.owner != Some(self.constructor.symbol())
            {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree,
                    symbol: Some(symbol),
                });
            }
            let found = self.tree_value_type(tree, self.typed_ast.get(tree).ty)?;
            if found != parameter.ty() {
                return Err(JvmCodegenError::ParameterReferenceTypeMismatch {
                    tree,
                    parameter: symbol,
                    expected: parameter.ty(),
                    found: Some(found),
                });
            }
            instructions.push(JvmConstructorInitializerInstruction::LoadParameter {
                symbol,
                slot: parameter.slot(),
                value_type: parameter.ty(),
            });
            return Ok(parameter.ty());
        }

        if symbol_info.kind != SymbolKind::Field {
            return Err(JvmCodegenError::InvalidParameterReference {
                tree,
                symbol: Some(symbol),
            });
        }
        if symbol_info.owner != Some(self.class) {
            return Err(JvmCodegenError::WrongOwner {
                symbol,
                expected: self.class,
                actual: symbol_info.owner,
            });
        }
        let field = self
            .fields
            .iter()
            .find(|field| field.symbol() == symbol)
            .ok_or(JvmCodegenError::FieldNotInJvmPlan {
                tree,
                class: self.class,
                field: symbol,
            })?;
        self.validate_field_receiver(tree, field, receiver)?;
        if !self.initialized_fields.contains(&symbol) {
            return Err(JvmCodegenError::FieldInitializerReadsUninitializedField {
                field: self.current_field,
                tree,
                referenced_field: symbol,
            });
        }
        let value_type = field_value_type(field);
        if matches!(value_type, JvmType::Reference(_)) {
            return self.unsupported(tree, "reference field reads in constructor initializers");
        }
        let found = self.tree_value_type(tree, self.typed_ast.get(tree).ty)?;
        if found != value_type {
            return self.type_mismatch(tree, value_type, found);
        }
        instructions.push(JvmConstructorInitializerInstruction::GetField {
            field: symbol,
            name: field.name().to_owned(),
            descriptor: field.descriptor().to_owned(),
        });
        Ok(value_type)
    }

    fn is_constructor_application(
        &self,
        tree_id: TreeId<Typed>,
        application: &dotty_core::ast::Apply<Typed>,
    ) -> Result<bool, JvmCodegenError> {
        if application.kind != dotty_core::ast::ApplyKind::Regular {
            return Ok(false);
        }
        let Some(function) = self.typed_ast.try_get(application.function) else {
            return Err(JvmCodegenError::UnsupportedConstructorFieldInitializer {
                field: self.current_field,
                tree: tree_id,
                node: "missing constructor function tree",
            });
        };
        let TreeKind::Select(select) = &function.kind else {
            return Ok(false);
        };
        if select.backquoted || self.store.names.try_resolve(select.name.text()) != Some("<init>") {
            return Ok(false);
        }
        let Some(qualifier) = self.typed_ast.try_get(select.qualifier) else {
            return Err(JvmCodegenError::UnsupportedConstructorFieldInitializer {
                field: self.current_field,
                tree: tree_id,
                node: "missing constructor allocation tree",
            });
        };
        Ok(matches!(qualifier.kind, TreeKind::New(_)))
    }

    fn lower_new_reference_initializer(
        &mut self,
        tree_id: TreeId<Typed>,
        application: &dotty_core::ast::Apply<Typed>,
        instructions: &mut Vec<JvmConstructorInitializerInstruction>,
        depth: usize,
    ) -> Result<JvmType, JvmCodegenError> {
        let target = self.unit.resolve_constructor_call(
            self.class,
            self.constructor.symbol(),
            tree_id,
            self.definitions,
        )?;
        if !target.access_flags().is_public()
            || !matches!(target.result_type(), JvmType::Reference(_))
            || application.args.len() != target.argument_types().len()
        {
            return self.unsupported(tree_id, "unsupported reference constructor target");
        }

        let mut argument_instructions = Vec::new();
        for (argument, expected) in application
            .args
            .iter()
            .copied()
            .zip(target.argument_types().iter().copied())
        {
            if !matches!(expected, JvmType::Int | JvmType::Boolean) {
                return self.unsupported(argument, "reference constructor argument type");
            }
            let argument_tree = self.typed_ast.try_get(argument).ok_or(
                JvmCodegenError::UnsupportedConstructorFieldInitializer {
                    field: self.current_field,
                    tree: argument,
                    node: "missing reference constructor argument",
                },
            )?;
            match &argument_tree.kind {
                TreeKind::Literal(_) => {}
                TreeKind::Ident(_) => {
                    let symbol = self.term_ref_symbol(argument, argument_tree.ty)?;
                    if !self
                        .constructor
                        .parameters()
                        .iter()
                        .any(|parameter| parameter.symbol() == symbol)
                    {
                        return self.unsupported(
                            argument,
                            "reference constructor argument is not an exact constructor parameter",
                        );
                    }
                }
                _ => {
                    return self.unsupported(
                        argument,
                        "reference constructor argument is not a primitive literal or parameter",
                    );
                }
            }
            let mut lowered_argument = Vec::new();
            let found = self.lower_expression(argument, &mut lowered_argument, depth + 1)?;
            if found != expected {
                return self.type_mismatch(argument, expected, found);
            }
            if !matches!(
                lowered_argument.as_slice(),
                [JvmConstructorInitializerInstruction::Constant(_)]
                    | [JvmConstructorInitializerInstruction::LoadParameter {
                        value_type: JvmType::Int | JvmType::Boolean,
                        ..
                    }]
            ) {
                return self.unsupported(
                    argument,
                    "reference constructor argument is not a primitive literal or parameter",
                );
            }
            argument_instructions.extend(lowered_argument);
        }

        instructions.push(JvmConstructorInitializerInstruction::NewObject {
            owner: target.owner_internal_name().to_owned(),
        });
        instructions.push(JvmConstructorInitializerInstruction::Dup);
        instructions.extend(argument_instructions);
        instructions.push(JvmConstructorInitializerInstruction::InvokeSpecial {
            owner: target.owner_internal_name().to_owned(),
            descriptor: target.descriptor().as_str().to_owned(),
        });
        Ok(target.result_type())
    }

    fn validate_field_receiver(
        &self,
        tree: TreeId<Typed>,
        field: &JvmFieldMetadata,
        receiver: Option<TreeId<Typed>>,
    ) -> Result<(), JvmCodegenError> {
        let reference_prefix = match self.store.types.try_get(self.typed_ast.get(tree).ty) {
            Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(target),
            }) if *target == field.symbol() => *prefix,
            _ => {
                return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
                    tree,
                    field: field.symbol(),
                    class: self.class,
                });
            }
        };
        if let Some(receiver) = receiver {
            let receiver_tree = self.typed_ast.try_get(receiver).ok_or(
                JvmCodegenError::UnsupportedConstructorFieldInitializer {
                    field: self.current_field,
                    tree: receiver,
                    node: "missing field receiver tree",
                },
            )?;
            if !matches!(
                receiver_tree.kind,
                TreeKind::This(dotty_core::ast::This { qual: None })
            ) || !is_current_this_type(self.store, receiver_tree.ty, self.class)
                || receiver_tree.ty != reference_prefix
            {
                return Err(JvmCodegenError::UnsupportedJvmFieldReceiver {
                    tree,
                    field: field.symbol(),
                });
            }
        } else if !is_current_this_type(self.store, reference_prefix, self.class) {
            return Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
                tree,
                field: field.symbol(),
                class: self.class,
            });
        }
        Ok(())
    }

    fn term_ref_symbol(
        &self,
        tree: TreeId<Typed>,
        ty: dotty_core::TypeId,
    ) -> Result<SymbolId, JvmCodegenError> {
        match self.store.types.try_get(ty) {
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) => Ok(*symbol),
            _ => Err(JvmCodegenError::UnsupportedConstructorFieldInitializer {
                field: self.current_field,
                tree,
                node: "typed reference does not target a canonical symbol",
            }),
        }
    }

    fn tree_value_type(
        &self,
        tree: TreeId<Typed>,
        ty: dotty_core::TypeId,
    ) -> Result<JvmType, JvmCodegenError> {
        if let Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(ty)
            && self.store.symbols.contains(*symbol)
        {
            let parameter = self.store.symbols.get(*symbol);
            let parameter_owner = parameter.owner.and_then(|owner| {
                self.store
                    .symbols
                    .contains(owner)
                    .then(|| self.store.symbols.get(owner))
            });
            if parameter.kind == SymbolKind::Parameter
                && matches!(parameter_owner, Some(owner)
                    if owner.kind == SymbolKind::Constructor
                        && parameter.owner == Some(self.constructor.symbol()))
            {
                let parameter_type = match self.store.symbols.info(*symbol) {
                    SymbolInfo::Complete(ty) => *ty,
                    SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                        return Err(JvmCodegenError::MissingSemanticInfo { symbol: *symbol });
                    }
                    SymbolInfo::Error => {
                        return Err(JvmCodegenError::ErrorSemanticInfo { symbol: *symbol });
                    }
                };
                return crate::identity::project_field_value_type(
                    self.store,
                    self.definitions,
                    self.current_field,
                    parameter_type,
                    Some(self.class_identities),
                    Some(self.unit),
                );
            }
        }
        match crate::identity::typed_expression_primitive_type(
            self.store,
            self.definitions,
            self.current_field,
            tree,
            ty,
        ) {
            Err(JvmCodegenError::UnsupportedJvmType { .. }) => {
                crate::identity::typed_expression_field_value_type(
                    self.store,
                    self.definitions,
                    self.current_field,
                    tree,
                    ty,
                    Some(self.class_identities),
                    Some(self.unit),
                )
            }
            result => result,
        }
    }

    fn type_mismatch(
        &self,
        tree: TreeId<Typed>,
        expected: JvmType,
        found: JvmType,
    ) -> Result<JvmType, JvmCodegenError> {
        Err(JvmCodegenError::FieldInitializerTypeMismatch {
            field: self.current_field,
            initializer: tree,
            expected,
            found: Some(found),
        })
    }

    fn unsupported<T>(
        &self,
        tree: TreeId<Typed>,
        node: &'static str,
    ) -> Result<T, JvmCodegenError> {
        Err(JvmCodegenError::UnsupportedConstructorFieldInitializer {
            field: self.current_field,
            tree,
            node,
        })
    }
}

fn field_value_type(field: &JvmFieldMetadata) -> JvmType {
    field.value_type()
}

fn is_current_this_type(store: &SemanticStore, ty: dotty_core::TypeId, class: SymbolId) -> bool {
    matches!(store.types.try_get(ty), Some(Type::ThisType { class: found }) if *found == class)
}

fn typed_node_name(kind: &TreeKind<Typed>) -> &'static str {
    match kind {
        TreeKind::Literal(_) => "Literal",
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::Apply(_) => "Apply",
        TreeKind::Block(_) => "Block",
        TreeKind::This(_) => "This",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::If(_) => "If",
        TreeKind::While(_) => "While",
        TreeKind::Return(_) => "Return",
        TreeKind::Assign(_) => "Assign",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        _ => "unsupported typed expression",
    }
}
