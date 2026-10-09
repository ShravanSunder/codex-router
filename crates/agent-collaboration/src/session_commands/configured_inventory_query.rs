//! Reject common picker queries that cannot be expressed with known source-owned inventory facts.
use crate::presentation::session_picker::{
    SessionsPickerDataQuery, SessionsPickerRoot, SourceInventoryRejection,
};
use crate::sessions::SessionsProvider;

pub(super) fn qualify_configured_query(
    query: &SessionsPickerDataQuery,
) -> Result<(), SourceInventoryRejection> {
    if query.root != SessionsPickerRoot::Any || query.provider != SessionsProvider::Any {
        // Caller checkout roots and provider settings are not facts about the source machine.
        // NEW's defaultRemoteCwd is likewise not an inventory scope.
        return Err(SourceInventoryRejection::UnsupportedViewOrScope);
    }
    if !query.search.trim().is_empty() {
        // The native query is one name/title substring, whereas the picker expression matches
        // more fields and ANDs terms. Sending it unchanged or omitting it would change meaning.
        return Err(SourceInventoryRejection::UnsupportedQuery);
    }
    Ok(())
}
