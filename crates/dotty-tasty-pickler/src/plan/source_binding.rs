use super::*;

impl Validator<'_, '_> {
    pub(super) fn span(&self, position: Option<SourceSpan>) -> Result<(), PickleError> {
        if let Some(position) = position
            && position.source() != self.plan.source.id
        {
            return Err(PickleError::SourceSpan {
                expected: self.plan.source.id,
                actual: position.source(),
            });
        }
        Ok(()) // Span/TextRange constructors already guarantee local range invariants.
    }

    /// Checks the authoritative source-to-typed identity for one expression.
    /// Nested validation uses the frontend's exact mapping, never a name or a
    /// structurally similar tree.
    pub(super) fn source_typed_binding(
        &self,
        source: TreeId<Untyped>,
        typed: TreeId<Typed>,
    ) -> Result<(), PickleError> {
        let source_id = self.plan.source.id;
        let actual = self.plan.typed_bindings.typed_at(source_id, source);
        if actual != Some(typed) {
            return Err(PickleError::BodyBinding {
                source: source_id,
                tree: source,
                expected: typed,
                actual,
            });
        }
        Ok(())
    }

    /// Validates both source spans and equality for a paired nested node.
    pub(super) fn paired_source_typed_span(
        &self,
        source_span: Option<SourceSpan>,
        typed_span: Option<SourceSpan>,
        typed_tree: TreeId<Typed>,
    ) -> Result<(), PickleError> {
        self.span(source_span)?;
        self.span(typed_span)?;
        if let (Some(source), Some(typed)) = (source_span, typed_span)
            && source != typed
        {
            return Err(PickleError::BodySpan { tree: typed_tree });
        }
        Ok(())
    }

    /// Preserves the root-body span contract: validate the typed span's source
    /// and compare only when both sides carry a span.
    pub(super) fn root_source_typed_span(
        &self,
        source_span: Option<SourceSpan>,
        typed_span: Option<SourceSpan>,
        typed_tree: TreeId<Typed>,
    ) -> Result<(), PickleError> {
        self.span(typed_span)?;
        if let (Some(source), Some(typed)) = (source_span, typed_span)
            && source != typed
        {
            return Err(PickleError::BodySpan { tree: typed_tree });
        }
        Ok(())
    }
}
