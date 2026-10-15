//! Canonical, deterministic JVM local-slot allocation.

use std::collections::HashMap;

use dotty_core::definitions::Definitions;
use dotty_core::ids::SymbolId;
use dotty_core::symbols::SymbolInfo;

use crate::{
    JvmCodegenError, JvmCompilationUnit, JvmLocalBinding, JvmLocalLayout, JvmLocalSlot,
    JvmMethodMetadata, descriptor,
};

/// `max_locals` is a u16 count, so valid indexes are `0..u16::MAX`.
const MAX_LOCAL_COUNT: usize = u16::MAX as usize;

pub(super) struct JvmLocalAllocator {
    method: SymbolId,
    pub(super) next_slot: usize,
    pub(super) locals: Vec<JvmLocalSlot>,
    pub(super) by_symbol: HashMap<SymbolId, usize>,
}

impl JvmLocalAllocator {
    pub(super) fn empty(method: SymbolId) -> Self {
        Self {
            method,
            next_slot: 1,
            locals: Vec::new(),
            by_symbol: HashMap::new(),
        }
    }

    fn after_parameters(method: &JvmMethodMetadata) -> Result<Self, JvmCodegenError> {
        // All currently accepted methods are ordinary instance methods. Keep
        // the receiver in slot zero even when the method has no parameters.
        let mut next_slot = 1usize;
        for parameter in method.parameters() {
            let end = usize::from(parameter.slot())
                .checked_add(usize::from(parameter.width()))
                .ok_or(JvmCodegenError::TooManyLocalSlots {
                    method: method.symbol(),
                })?;
            next_slot = next_slot.max(end);
        }
        let mut allocator = Self::empty(method.symbol());
        allocator.next_slot = next_slot;
        Ok(allocator)
    }

    pub(super) fn reserve(
        &mut self,
        binding: JvmLocalBinding,
        ty: crate::JvmType,
    ) -> Result<(), JvmCodegenError> {
        // Check the count before publishing either the map entry or slot.
        if self.next_slot >= MAX_LOCAL_COUNT {
            return Err(JvmCodegenError::TooManyLocalSlots {
                method: self.method,
            });
        }
        let slot =
            u16::try_from(self.next_slot).map_err(|_| JvmCodegenError::TooManyLocalSlots {
                method: self.method,
            })?;
        let index = self.locals.len();
        self.locals.push(JvmLocalSlot {
            declaration: binding.declaration,
            symbol: binding.symbol,
            ty,
            slot,
            mutable: binding.mutable,
        });
        self.by_symbol.insert(binding.symbol, index);
        self.next_slot += 1; // Int and Boolean are both category one.
        Ok(())
    }

    fn finish(self) -> Result<JvmLocalLayout, JvmCodegenError> {
        let max_locals =
            u16::try_from(self.next_slot).map_err(|_| JvmCodegenError::TooManyLocalSlots {
                method: self.method,
            })?;
        Ok(JvmLocalLayout {
            method: self.method,
            locals: self.locals,
            by_symbol: self.by_symbol,
            max_locals,
        })
    }
}

pub(super) fn method_local_layout(
    unit: &JvmCompilationUnit<'_>,
    class: SymbolId,
    method: SymbolId,
    definitions: &Definitions,
) -> Result<JvmLocalLayout, JvmCodegenError> {
    let class_plan = unit
        .classes
        .iter()
        .find(|plan| plan.class == class)
        .ok_or(JvmCodegenError::MissingClassPlan { class })?;
    let method_plan = class_plan
        .methods
        .iter()
        .find(|plan| plan.method == method)
        .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan { class, method })?;
    let metadata = class_plan
        .metadata_with_typed_ast(unit.store, unit.typed_ast, definitions)?
        .methods()
        .iter()
        .find(|metadata| metadata.symbol() == method)
        .cloned()
        .ok_or(JvmCodegenError::CallerMethodNotInJvmPlan { class, method })?;

    method_local_layout_for(unit, &metadata, method_plan.locals(), definitions)
}

pub(super) fn method_local_layout_for(
    unit: &JvmCompilationUnit<'_>,
    method: &JvmMethodMetadata,
    bindings: &[JvmLocalBinding],
    definitions: &Definitions,
) -> Result<JvmLocalLayout, JvmCodegenError> {
    unit.validate()?;
    let method_symbol = method.symbol();
    let Some((class_plan, method_plan)) = unit.classes.iter().find_map(|class_plan| {
        class_plan
            .methods
            .iter()
            .find(|plan| plan.method == method_symbol)
            .map(|plan| (class_plan, plan))
    }) else {
        return Err(JvmCodegenError::LocalLayoutInputMismatch {
            method: method_symbol,
        });
    };
    if method_plan.locals() != bindings {
        return Err(JvmCodegenError::LocalLayoutInputMismatch {
            method: method_symbol,
        });
    }
    let class_metadata =
        class_plan.metadata_with_typed_ast(unit.store, unit.typed_ast, definitions)?;
    let fresh_metadata = class_metadata
        .methods()
        .iter()
        .find(|metadata| metadata.symbol() == method_symbol)
        .ok_or(JvmCodegenError::LocalLayoutInputMismatch {
            method: method_symbol,
        })?;
    if fresh_metadata != method {
        return Err(JvmCodegenError::LocalLayoutInputMismatch {
            method: method_symbol,
        });
    }

    let mut allocator = JvmLocalAllocator::after_parameters(method)?;
    for binding in bindings {
        let ty = match unit.store.symbols.info(binding.symbol) {
            SymbolInfo::Complete(ty) => *ty,
            // `unit.validate` has already checked this branch; retain a typed
            // failure if this code is changed to validate a narrower input.
            SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                return Err(JvmCodegenError::MissingSemanticInfo {
                    symbol: binding.symbol,
                });
            }
            SymbolInfo::Error => {
                return Err(JvmCodegenError::ErrorSemanticInfo {
                    symbol: binding.symbol,
                });
            }
        };
        let primitive =
            descriptor::primitive_semantic_value_type(unit.store, definitions, binding.symbol, ty)?;
        allocator.reserve(*binding, primitive)?;
    }
    allocator.finish()
}
