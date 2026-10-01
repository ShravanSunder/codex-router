//! Resolve dependency kinds so SDK ownership holds across indirect crate paths.
#![allow(clippy::panic_in_result_fn)]

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    process::Command,
};

use serde::Deserialize;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_members: Vec<String>,
    resolve: CargoResolve,
}

#[derive(Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct CargoResolve {
    nodes: Vec<CargoNode>,
}

#[derive(Deserialize)]
struct CargoNode {
    id: String,
    deps: Vec<CargoDependency>,
}

#[derive(Deserialize)]
struct CargoDependency {
    pkg: String,
    dep_kinds: Vec<CargoDependencyKind>,
}

#[derive(Deserialize)]
struct CargoDependencyKind {
    kind: Option<String>,
}

struct NonDevDependencyGraph {
    names: HashMap<String, String>,
    edges: HashMap<String, Vec<String>>,
    workspace_members: Vec<String>,
}

impl NonDevDependencyGraph {
    fn from_metadata(metadata: CargoMetadata) -> Self {
        let names = metadata
            .packages
            .into_iter()
            .map(|package| (package.id, package.name))
            .collect();
        let edges = metadata
            .resolve
            .nodes
            .into_iter()
            .map(|node| {
                let destinations = node
                    .deps
                    .into_iter()
                    .filter(|dependency| {
                        dependency
                            .dep_kinds
                            .iter()
                            .any(|kind| kind.kind.as_deref() != Some("dev"))
                    })
                    .map(|dependency| dependency.pkg)
                    .collect();
                (node.id, destinations)
            })
            .collect();
        Self {
            names,
            edges,
            workspace_members: metadata.workspace_members,
        }
    }

    fn name(&self, package_id: &str) -> &str {
        self.names
            .get(package_id)
            .map_or("unknown package", String::as_str)
    }

    fn workspace_id(&self, package_name: &str) -> Option<&str> {
        self.workspace_members
            .iter()
            .find(|id| self.name(id) == package_name)
            .map(String::as_str)
    }

    fn path_to(
        &self,
        start: &str,
        target_name: &str,
        blocked_owners: &[&str],
    ) -> Option<Vec<String>> {
        let mut queue = VecDeque::from([vec![start.to_owned()]]);
        let mut visited = HashSet::new();
        while let Some(path) = queue.pop_front() {
            let current = path.last()?;
            if !visited.insert(current.clone()) {
                continue;
            }
            if self.name(current) == target_name {
                return Some(
                    path.into_iter()
                        .map(|id| self.name(&id).to_owned())
                        .collect(),
                );
            }
            for destination in self.edges.get(current).into_iter().flatten() {
                if blocked_owners.contains(&self.name(destination)) {
                    continue;
                }
                let mut next_path = path.clone();
                next_path.push(destination.clone());
                queue.push_back(next_path);
            }
        }
        None
    }
}

fn resolved_graph() -> Result<NonDevDependencyGraph, Box<dyn std::error::Error + Send + Sync>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("workspace root unavailable")?
        .to_owned();
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(workspace_root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(NonDevDependencyGraph::from_metadata(
        serde_json::from_slice(&output.stdout)?,
    ))
}

#[test]
fn acp_client_cannot_reach_router_service_or_protocol_crates() -> TestResult {
    let graph = resolved_graph()?;
    let client = graph
        .workspace_id("acp-client-runtime")
        .ok_or("client crate missing")?;
    for member in &graph.workspace_members {
        let name = graph.name(member);
        if name.starts_with("collaboration-") || name.starts_with("codex-router-") {
            let path = graph.path_to(client, name, &[]);
            assert!(
                path.is_none(),
                "ACP client reaches Router crate {name} through non-dev edges: {path:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn only_acp_role_owners_have_direct_production_sdk_edges() -> TestResult {
    let graph = resolved_graph()?;
    for member in &graph.workspace_members {
        let name = graph.name(member);
        if matches!(name, "acp-client-runtime" | "codex-acp-adapter") {
            continue;
        }
        let destinations = graph.edges.get(member).into_iter().flatten();
        for destination in destinations {
            assert_ne!(
                graph.name(destination),
                "agent-client-protocol",
                "{name} has a direct non-dev ACP SDK edge"
            );
        }
    }
    Ok(())
}

#[test]
fn every_indirect_sdk_path_crosses_an_acp_role_owner() -> TestResult {
    let graph = resolved_graph()?;
    for member in &graph.workspace_members {
        let name = graph.name(member);
        if matches!(name, "acp-client-runtime" | "codex-acp-adapter") {
            continue;
        }
        let path = graph.path_to(
            member,
            "agent-client-protocol",
            &["acp-client-runtime", "codex-acp-adapter"],
        );
        assert!(
            path.is_none(),
            "{name} reaches the ACP SDK without crossing an ACP role owner: {path:?}"
        );
    }
    Ok(())
}

#[test]
fn graph_guard_detects_indirect_bypass_and_ignores_dev_edges() {
    let metadata = CargoMetadata {
        packages: [
            ("host", "codex-router-host"),
            ("shim", "session-shim"),
            ("owner", "acp-client-runtime"),
            ("sdk", "agent-client-protocol"),
        ]
        .into_iter()
        .map(|(id, name)| CargoPackage {
            id: id.to_owned(),
            name: name.to_owned(),
        })
        .collect(),
        workspace_members: vec!["host".to_owned(), "shim".to_owned(), "owner".to_owned()],
        resolve: CargoResolve {
            nodes: vec![
                CargoNode {
                    id: "host".to_owned(),
                    deps: vec![
                        CargoDependency {
                            pkg: "shim".to_owned(),
                            dep_kinds: vec![CargoDependencyKind { kind: None }],
                        },
                        CargoDependency {
                            pkg: "sdk".to_owned(),
                            dep_kinds: vec![CargoDependencyKind {
                                kind: Some("dev".to_owned()),
                            }],
                        },
                    ],
                },
                CargoNode {
                    id: "shim".to_owned(),
                    deps: vec![CargoDependency {
                        pkg: "sdk".to_owned(),
                        dep_kinds: vec![CargoDependencyKind {
                            kind: Some("build".to_owned()),
                        }],
                    }],
                },
                CargoNode {
                    id: "owner".to_owned(),
                    deps: vec![CargoDependency {
                        pkg: "sdk".to_owned(),
                        dep_kinds: vec![CargoDependencyKind { kind: None }],
                    }],
                },
            ],
        },
    };
    let graph = NonDevDependencyGraph::from_metadata(metadata);
    assert_eq!(
        graph.path_to("host", "agent-client-protocol", &["acp-client-runtime"]),
        Some(vec![
            "codex-router-host".to_owned(),
            "session-shim".to_owned(),
            "agent-client-protocol".to_owned(),
        ])
    );
    assert_eq!(
        graph.path_to("owner", "agent-client-protocol", &[]),
        Some(vec![
            "acp-client-runtime".to_owned(),
            "agent-client-protocol".to_owned(),
        ])
    );
}

#[test]
fn public_protocol_error_uses_client_owned_version() {
    let error = acp_client_runtime::ExternalProviderRuntimeError::UnsupportedProtocol {
        actual: acp_client_runtime::AcpProtocolVersion::new(0),
    };
    assert_eq!(
        error.to_string(),
        "provider selected unsupported ACP protocol version ProtocolVersion(0)"
    );
}
