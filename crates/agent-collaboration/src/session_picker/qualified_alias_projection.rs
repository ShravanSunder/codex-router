//! Alias labels are a transient projection of successful full endpoint bindings.
use super::*;

pub(super) fn qualified_alias_labels(
    source_order: &[PickerSourceContext],
    snapshots: &[(
        PickerSourceContext,
        Option<EndpointRef>,
        PickerRecordsSnapshot,
    )],
) -> BTreeMap<EndpointRef, String> {
    let mut groups: BTreeMap<EndpointRef, Vec<&str>> = BTreeMap::new();
    for source in source_order {
        let PickerSourceContext::ConfiguredHosted(profile) = source else {
            continue;
        };
        let Some((_, Some(endpoint), snapshot)) =
            snapshots.iter().find(|(context, _, _)| context == source)
        else {
            continue;
        };
        if !source_snapshot_matches(source, Some(endpoint), snapshot) {
            continue;
        }
        let names = groups.entry(endpoint.clone()).or_default();
        if !names.contains(&profile.name.as_str()) {
            names.push(profile.name.as_str());
        }
    }
    groups
        .into_iter()
        .filter_map(|(endpoint, names)| {
            let (first, aliases) = names.split_first()?;
            if aliases.is_empty() {
                return None;
            }
            Some((endpoint, format!("{first} (also: {})", aliases.join(", "))))
        })
        .collect()
}

#[cfg(test)]
#[path = "qualified_alias_projection_tests.rs"]
mod tests;
