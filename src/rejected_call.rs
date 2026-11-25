//! Source evidence retained for rejected ordinary calls.
use crate::NamedClassSource;
use crate::core::SourceSpan;
use crate::parser::NamedCallArgument;

pub(crate) fn written_label(
    file: &NamedClassSource,
    site: SourceSpan,
) -> Option<&NamedCallArgument> {
    let range = site.span().range();
    let list = file.named().syntax().call_arguments.at(range.start())?;
    let written = list.argument_index(range.start())?;
    if site.source() != file.source() || list.arguments.get(written) != Some(&range) {
        return None;
    }
    list.named_arguments
        .iter()
        .find(|label| label.written == written)
}
