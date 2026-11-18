//! Projects structural default-getter sites in this template to entered symbols.

use dotty_core::SymbolId;
use dotty_tasty::tasty::{DEFDEF_TAG, PARAM_TAG, template_default_getters};

use crate::ast_view::{AstView, address};
use crate::error::{DefinitionReason, UnpickleError};
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    pub(crate) fn link_template_default_getters(
        &mut self,
        ast: &AstView<'_>,
        template: u32,
        owner: SymbolId,
    ) -> Result<(), UnpickleError> {
        let nodes = ast
            .children(template)
            .iter()
            .filter(|child| child.tag == DEFDEF_TAG)
            .map(|child| ast.node(address(child.offset)))
            .collect::<Result<Vec<_>, _>>()?;
        for site in template_default_getters(self.file.names(), nodes)? {
            let at = address(site.getter);
            let symbol_at = |at| {
                self.index
                    .symbol_at(at)
                    .ok_or(UnpickleError::MissingDefinition { address: at })
            };
            let getter = symbol_at(at)?;
            let method_at = address(site.method);
            let method = symbol_at(method_at)?;
            let param = ast
                .children(method_at)
                .iter()
                .filter(|p| p.tag == PARAM_TAG)
                .nth(site.ordinal)
                .ok_or(UnpickleError::MissingDefinition { address: method_at })?;
            let parameter = symbol_at(address(param.offset))?;
            let scope = self
                .index
                .scope_of(owner)
                .ok_or(UnpickleError::MissingClassScope {
                    address: template,
                    symbol: owner,
                })?;
            if self
                .store
                .scopes
                .get(scope)
                .lookup_all(&self.store.symbols.get(getter).name)
                == [getter]
            {
                self.store
                    .record_default_getter(method, site.ordinal, parameter, getter)
                    .map_err(|_| UnpickleError::MalformedDefinition {
                        address: at,
                        reason: DefinitionReason::ConflictingDefaultGetter,
                    })?;
            }
        }
        Ok(())
    }
}
