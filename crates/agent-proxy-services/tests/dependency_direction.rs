use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Option<Resolution>,
}
#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
}
#[derive(Deserialize)]
struct Resolution {
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
    kind: Option<NonNormalKind>,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum NonNormalKind {
    Build,
    Dev,
}
fn closure(root: &str, nodes: &BTreeMap<String, Vec<String>>) -> Option<BTreeSet<String>> {
    let mut visited = BTreeSet::new();
    let mut pending = vec![root.to_owned()];
    while let Some(package) = pending.pop() {
        if visited.insert(package.clone()) {
            pending.extend(nodes.get(&package)?.iter().cloned());
        }
    }
    Some(visited)
}
#[test]
fn production_dependency_closures_keep_proxy_core_and_descriptor_domain_boundaries() {
    let output = Command::new("cargo")
        .args([
            "+1.98.1",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--offline",
        ])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("locked offline metadata");
    assert!(
        output.status.success(),
        "metadata must resolve without lock changes: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Metadata = serde_json::from_slice(&output.stdout).expect("actual resolved graph");
    let ids = metadata
        .packages
        .iter()
        .map(|package| (package.name.as_str(), package.id.as_str()))
        .collect::<BTreeMap<_, _>>();
    let nodes = metadata
        .resolve
        .expect("real resolution")
        .nodes
        .into_iter()
        .map(|node| {
            (
                node.id,
                node.deps
                    .into_iter()
                    .filter(|dependency| {
                        dependency
                            .dep_kinds
                            .iter()
                            .any(|kind| kind.kind != Some(NonNormalKind::Dev))
                    })
                    .map(|dependency| dependency.pkg)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (owner, forbidden) in [
        (
            "agent-proxy-services",
            vec![
                "codex-router-cli",
                "codex-router-host",
                "codex-router-keeper",
            ],
        ),
        (
            "codex-router-proxy",
            vec![
                "agent-proxy-services",
                "codex-router-keeper",
                "codex-router-keeper-protocol",
            ],
        ),
        (
            "codex-router-descriptor-boundary",
            vec![
                "agent-proxy-services",
                "codex-router-cli",
                "codex-router-host",
                "codex-router-keeper",
                "codex-router-keeper-protocol",
                "codex-native-integration",
                "codex-router-core",
            ],
        ),
    ] {
        let reached = closure(ids.get(owner).expect("declared owner exists"), &nodes)
            .expect("every resolved dependency has a node");
        for name in forbidden {
            assert!(
                !reached.contains(*ids.get(name).expect("forbidden workspace owner exists")),
                "{owner} must not acquire {name} through normal/build/proc-macro closure"
            );
        }
        eprintln!(
            "normal/build/proc-macro owner={owner} packages={}",
            reached.len()
        );
    }
    let role = closure(ids.get("agent-proxy-services").expect("role"), &nodes)
        .expect("every resolved role dependency has a node");
    for required in [
        "codex-router-proxy",
        "codex-router-keeper-protocol",
        "codex-router-state",
        "codex-router-secret-store",
        "codex-router-auth",
        "codex-router-quota",
        "codex-router-descriptor-boundary",
    ] {
        assert!(
            role.contains(*ids.get(required).expect("real behavior owner exists")),
            "role must include actual {required} behavior"
        );
    }
}
