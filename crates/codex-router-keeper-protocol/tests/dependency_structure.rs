//! Resolve every normal/build/proc-macro edge, not only direct manifest dependencies.
use codex_router_descriptor_boundary::DescriptorGate;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    resolve: Resolve,
}
#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}
#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dependency>,
}
#[derive(Deserialize)]
struct Dependency {
    pkg: String,
    dep_kinds: Vec<DependencyKind>,
}
#[derive(Deserialize)]
struct DependencyKind {
    kind: Option<String>,
}
#[tokio::test]
async fn descriptor_boundary_has_no_domain_reachability_and_protocol_has_no_role_edge()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace root absent")?;
    let mut command = tokio::process::Command::new("cargo");
    command
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = DescriptorGate::global()
        .spawn_child(&mut command)
        .await?
        .wait_with_output()
        .await?;
    // output pipes must be retained by Command explicitly for this metadata observation.
    if !output.status.success() {
        return Err("locked/offline metadata failed".into());
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    let names: HashMap<_, _> = metadata
        .packages
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let edges: HashMap<_, _> = metadata
        .resolve
        .nodes
        .into_iter()
        .map(|node| {
            (
                node.id,
                node.deps
                    .into_iter()
                    .filter(|d| d.dep_kinds.iter().any(|k| k.kind.as_deref() != Some("dev")))
                    .map(|d| d.pkg)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let workspace: HashSet<_> = metadata.workspace_members.into_iter().collect();
    for owner in [
        "codex-router-descriptor-boundary",
        "codex-router-keeper-protocol",
        "codex-router-keeper",
    ] {
        let start = names
            .iter()
            .find(|(_, name)| name.as_str() == owner)
            .map(|(id, _)| id.clone())
            .ok_or("owner missing")?;
        if owner == "codex-router-keeper" || owner == "codex-router-keeper-protocol" {
            let allowed_direct = if owner == "codex-router-keeper" {
                [
                    "codex-router-keeper-protocol",
                    "codex-router-descriptor-boundary",
                    "codex-native-integration",
                ]
            } else {
                [
                    "collaboration-protocol",
                    "codex-router-descriptor-boundary",
                    "codex-native-integration",
                ]
            };
            for dependency in edges.get(&start).into_iter().flatten() {
                let name = names.get(dependency).ok_or("direct dependency absent")?;
                if workspace.contains(dependency) && !allowed_direct.contains(&name.as_str()) {
                    return Err(format!("forbidden direct {owner} edge to {name}").into());
                }
            }
        }
        let mut queue = VecDeque::from([start]);
        let mut visited = HashSet::new();
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let name = names.get(&id).ok_or("resolved name missing")?;
            if owner == "codex-router-descriptor-boundary"
                && workspace.contains(&id)
                && name != owner
            {
                return Err(format!("descriptor domain edge to {name}").into());
            }
            // Exact resolved workspace closure of the selected existing EndpointId edge.
            // message-board is reached by agent-automation, not imported or re-owned by keeper.
            if owner != "codex-router-descriptor-boundary"
                && workspace.contains(&id)
                && ![
                    owner,
                    "codex-router-keeper-protocol",
                    "codex-router-descriptor-boundary",
                    "codex-native-integration",
                    "collaboration-protocol",
                    "session-event-model",
                    "agent-automation",
                    "message-board",
                ]
                .contains(&name.as_str())
            {
                return Err(format!("unexpected scalar workspace closure to {name}").into());
            }
            if (name.starts_with("collaboration-") && name != "collaboration-protocol")
                || [
                    "codex-router-host",
                    "codex-router-proxy",
                    "codex-acp-adapter",
                    "acp-client-runtime",
                    "agent-provider-services",
                    "agent-proxy-services",
                    "agent-collaboration-services",
                ]
                .contains(&name.as_str())
            {
                return Err(format!("forbidden transport role edge to {name}").into());
            }
            for dependency in edges.get(&id).into_iter().flatten() {
                queue.push_back(dependency.clone());
            }
        }
    }
    Ok(())
}
