//! Reject common picker queries that cannot be expressed with known source-owned inventory facts.
use crate::presentation::session_picker::{
    InventoryEndpointKind, SessionsPickerDataQuery, SessionsPickerRoot, SourceInventoryBinding,
    SourceInventoryRejection,
};
use crate::sessions::{SessionsProvider, SessionsSort, SessionsSource};

pub(super) fn qualify_configured_query(
    query: &SessionsPickerDataQuery,
) -> Result<(), SourceInventoryRejection> {
    if query.root != SessionsPickerRoot::Any || query.provider != SessionsProvider::Any {
        // Caller checkout roots and provider settings are not facts about the source machine.
        // NEW's defaultRemoteCwd is likewise not an inventory scope.
        return Err(SourceInventoryRejection::UnsupportedViewOrScope);
    }
    let search = query.search.trim();
    if !search.is_empty()
        && super::session_command_options::validate_exact_uuid_session_id(search).is_err()
    {
        // A complete pasted UUID is filtered locally across source-owned pages. The native
        // query only searches name/title, so it stays unset; other expressions remain rejected.
        return Err(SourceInventoryRejection::UnsupportedQuery);
    }
    if query.sort != SessionsSort::Updated {
        return Err(SourceInventoryRejection::UnsupportedSort);
    }
    Ok(())
}

pub(super) fn qualify_bound_source_filter(
    query: &SessionsPickerDataQuery,
    binding: &SourceInventoryBinding,
) -> Result<(), SourceInventoryRejection> {
    // Stored provider inventories cannot express the native source classifications.
    // Reject the whole common query rather than omitting provider rows or broadening it.
    if query.source != SessionsSource::All
        && binding
            .endpoints
            .values()
            .any(|kind| *kind == InventoryEndpointKind::ExternalProvider)
    {
        return Err(SourceInventoryRejection::UnsupportedViewOrScope);
    }
    Ok(())
}
