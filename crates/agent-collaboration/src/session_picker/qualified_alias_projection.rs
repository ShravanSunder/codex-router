//! Alias labels are a transient projection of successful full endpoint bindings.
use super::*;

enum EndpointAliasQualification<'a> {
    Matching {
        binding: SourceInventoryBinding,
        names: Vec<&'a str>,
    },
    Conflicted,
}

pub(super) fn qualified_alias_labels(
    source_order: &[PickerSourceContext],
    snapshots: &[(
        PickerSourceContext,
        Option<SourceInventoryBinding>,
        PickerRecordsSnapshot,
    )],
) -> BTreeMap<EndpointRef, String> {
    let mut groups: BTreeMap<EndpointRef, EndpointAliasQualification<'_>> = BTreeMap::new();
    for source in source_order {
        let PickerSourceContext::ConfiguredHosted(profile) = source else {
            continue;
        };
        let Some((_, Some(binding), snapshot)) =
            snapshots.iter().find(|(context, _, _)| context == source)
        else {
            continue;
        };
        if !source_snapshot_matches(source, Some(binding), snapshot) {
            continue;
        }
        for endpoint in binding.endpoints.keys() {
            let group = groups.entry(endpoint.clone()).or_insert_with(|| {
                EndpointAliasQualification::Matching {
                    binding: binding.clone(),
                    names: Vec::new(),
                }
            });
            match group {
                EndpointAliasQualification::Matching {
                    binding: observed,
                    names,
                } if observed == binding => {
                    if !names.contains(&profile.name.as_str()) {
                        names.push(profile.name.as_str());
                    }
                }
                EndpointAliasQualification::Matching { .. } => {
                    *group = EndpointAliasQualification::Conflicted
                }
                EndpointAliasQualification::Conflicted => {}
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(endpoint, group)| {
            let EndpointAliasQualification::Matching { names, .. } = group else {
                return None;
            };
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
