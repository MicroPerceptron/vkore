//! The permission graph: which agent reaches which path, and how.
//!
//! Two kinds of edge carry two different claims. Containment says a path sits
//! inside another, and is a fact about the workspace. A grant says an agent
//! holds authority over a path, and is a fact about the session. Authority
//! flows down containment: a grant on a directory covers everything beneath
//! it, which is why selecting an agent lights more than the paths it was
//! granted directly.
//!
//! Paths are discovered from explicit resource leases and typed tool-call
//! arguments. Nothing is assumed to exist because a workspace usually has it.

use crate::topology::layout::Placement;
use crate::topology::model::Graph;
use serde_json::Value;

/// Band offsets, from the design.
const BAND_WORKERS: f32 = 116.0;
const BAND_EXTERNAL: f32 = 236.0;
const BAND_WORKSPACE: f32 = 366.0;
/// Agent node size in this view.
const AGENT_WIDTH: f32 = 176.0;
const AGENT_HEIGHT: f32 = 46.0;
const AGENT_GAP: f32 = 26.0;
/// Path node metrics.
const PATH_HEIGHT: f32 = 38.0;
const PATH_GAP: f32 = 22.0;
const ROW_GAP: f32 = 58.0;

/// What a path is, which decides how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A directory inside the workspace.
    Directory,
    /// A file inside the workspace.
    File,
    /// A path outside the workspace entirely.
    External,
}

impl Kind {
    /// The short label the design right-aligns in a node's title row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "dir",
            Self::File => "file",
            Self::External => "ext",
        }
    }
}

/// One path the session touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathNode {
    /// Full path as the runtime named it.
    pub path: String,
    /// Last segment, which is what the node shows.
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// Containing path, when one is also present.
    pub parent: Option<usize>,
}

/// An agent's direct authority over a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// Index into the agent graph.
    pub agent: usize,
    /// Index into [`Filesystem::paths`].
    pub path: usize,
    /// Rights the grant carries, already flattened.
    pub rights: String,
}

/// The derived permission graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filesystem {
    /// Every path the session touched, roots first.
    pub paths: Vec<PathNode>,
    /// Direct grants. Coverage is derived from these, not stored.
    pub grants: Vec<Grant>,
}

impl Filesystem {
    /// Builds the permission graph the events describe.
    pub fn from_events(events: &[Value], graph: &Graph) -> Self {
        let mut filesystem = Self::default();
        for event in events {
            let generation = event["generation"]
                .as_u64()
                .or_else(|| event["payload"]["generation"].as_u64())
                .unwrap_or_default();
            let Some(agent) = graph
                .agents
                .iter()
                .position(|agent| agent.generation == generation)
            else {
                continue;
            };
            match event["kind"].as_str() {
                Some("capability") => {
                    for (path, rights) in leased_paths(event) {
                        filesystem.grant(agent, &path, &rights);
                    }
                }
                Some("tool_call") => {
                    let Some(call) = crate::decision_payload(event) else {
                        continue;
                    };
                    let rights = call["tool"]
                        .as_str()
                        .or_else(|| call["command"].as_str())
                        .unwrap_or("access");
                    for path in crate::call_path_arguments(&call) {
                        filesystem.grant(agent, &path, rights);
                    }
                }
                _ => {}
            }
        }
        filesystem
    }

    fn grant(&mut self, agent: usize, path: &str, rights: &str) {
        let path = normalize_path(path);
        if path.is_empty() {
            return;
        }
        let index = self.intern(&path);
        if !self
            .grants
            .iter()
            .any(|grant| grant.agent == agent && grant.path == index)
        {
            self.grants.push(Grant {
                agent,
                path: index,
                rights: rights.to_owned(),
            });
        }
    }

    /// Returns the index for `path`, adding it and its ancestors if needed.
    fn intern(&mut self, path: &str) -> usize {
        if let Some(index) = self.paths.iter().position(|node| node.path == path) {
            return index;
        }
        let external = path.starts_with('/') || path.starts_with('~');
        let (parent_path, name) = match path.rsplit_once('/') {
            Some((head, tail)) if !head.is_empty() && !external => (Some(head), tail),
            Some((_, tail)) => (None, tail),
            None => (None, path),
        };
        // An ancestor must exist before the child that names it.
        let parent = parent_path.map(|head| self.intern(head));
        self.paths.push(PathNode {
            path: path.to_owned(),
            name: name.to_owned(),
            kind: if external {
                Kind::External
            } else if parent_path.is_some() || path.contains('.') {
                Kind::File
            } else {
                Kind::Directory
            },
            parent: parent.filter(|_| !external),
        });
        self.paths.len() - 1
    }

    /// Whether `agent` reaches `path`, directly or through containment.
    ///
    /// A grant on a directory covers everything beneath it, so this walks up
    /// from the path rather than down from the grant.
    pub fn covers(&self, agent: usize, path: usize) -> bool {
        let mut current = Some(path);
        // Bounded by the depth of the tree, and every parent index is lower
        // than its child's, so this terminates.
        while let Some(index) = current {
            if self
                .grants
                .iter()
                .any(|grant| grant.agent == agent && grant.path == index)
            {
                return true;
            }
            current = self.paths.get(index).and_then(|node| node.parent);
        }
        false
    }

    /// Every path `agent` reaches.
    pub fn covered(&self, agent: usize) -> Vec<usize> {
        (0..self.paths.len())
            .filter(|path| self.covers(agent, *path))
            .collect()
    }

    /// Agents holding authority over `path`, and whether it was direct.
    pub fn holders(&self, path: usize) -> Vec<(usize, bool)> {
        let mut holders: Vec<(usize, bool)> = Vec::new();
        for grant in &self.grants {
            if grant.path == path && !holders.iter().any(|(a, _)| *a == grant.agent) {
                holders.push((grant.agent, true));
            }
        }
        for grant in &self.grants {
            if self.covers(grant.agent, path) && !holders.iter().any(|(a, _)| *a == grant.agent) {
                holders.push((grant.agent, false));
            }
        }
        holders
    }

    /// Places agents and paths into the design's four bands.
    ///
    /// Returns agent placements first, then path placements, so a caller can
    /// index each with the same indices it already holds.
    pub fn place(&self, graph: &Graph) -> (Vec<Placement>, Vec<Placement>) {
        let mut paths = vec![
            Placement {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: PATH_HEIGHT,
            };
            self.paths.len()
        ];

        // The handoff lays the workspace out as a tidy variable-width tree:
        // leaves advance one shared cursor and parents centre over their
        // outermost children. This keeps containment readable at every depth.
        let mut children = vec![Vec::new(); self.paths.len()];
        let mut roots = Vec::new();
        for (index, node) in self.paths.iter().enumerate() {
            if node.kind == Kind::External {
                continue;
            }
            match node.parent {
                Some(parent) if parent < index => children[parent].push(index),
                _ => roots.push(index),
            }
        }
        fn place_tree(
            index: usize,
            depth: usize,
            cursor: &mut f32,
            nodes: &[PathNode],
            children: &[Vec<usize>],
            placements: &mut [Placement],
        ) -> f32 {
            let width = node_width(&nodes[index].name);
            placements[index].width = width;
            placements[index].y = BAND_WORKSPACE + depth as f32 * (PATH_HEIGHT + ROW_GAP);
            let center = if children[index].is_empty() {
                let center = *cursor + width / 2.0;
                *cursor += width + PATH_GAP;
                center
            } else {
                let first = place_tree(
                    children[index][0],
                    depth + 1,
                    cursor,
                    nodes,
                    children,
                    placements,
                );
                let mut last = first;
                for child in children[index].iter().skip(1) {
                    last = place_tree(*child, depth + 1, cursor, nodes, children, placements);
                }
                (first + last) / 2.0
            };
            placements[index].x = center - width / 2.0;
            center
        }
        let mut tree_cursor = 0.0;
        for root in roots {
            place_tree(
                root,
                0,
                &mut tree_cursor,
                &self.paths,
                &children,
                &mut paths,
            );
        }
        let tree_width = (tree_cursor - PATH_GAP).max(0.0);
        let worker_count = graph.agents.len().saturating_sub(1);
        let worker_width =
            worker_count as f32 * AGENT_WIDTH + worker_count.saturating_sub(1) as f32 * AGENT_GAP;
        let external_count = self
            .paths
            .iter()
            .filter(|node| node.kind == Kind::External)
            .count();
        let external_width = external_count as f32 * AGENT_WIDTH
            + external_count.saturating_sub(1) as f32 * AGENT_GAP;
        let content_width = tree_width.max(worker_width).max(external_width);
        let tree_offset = (content_width - tree_width) / 2.0;
        for (index, node) in self.paths.iter().enumerate() {
            if node.kind != Kind::External {
                paths[index].x += tree_offset;
            }
        }

        let mut external_index = 0;
        for (index, node) in self.paths.iter().enumerate() {
            if node.kind == Kind::External {
                paths[index] = Placement {
                    x: (content_width - external_width) / 2.0
                        + external_index as f32 * (AGENT_WIDTH + AGENT_GAP),
                    y: BAND_EXTERNAL,
                    width: AGENT_WIDTH,
                    height: PATH_HEIGHT,
                };
                external_index += 1;
            }
        }

        let mut agents = Vec::with_capacity(graph.agents.len());
        let worker_offset = (content_width - worker_width) / 2.0;
        for (index, _) in graph.agents.iter().enumerate() {
            agents.push(Placement {
                x: if index == 0 {
                    (content_width - AGENT_WIDTH) / 2.0
                } else {
                    worker_offset + (index - 1) as f32 * (AGENT_WIDTH + AGENT_GAP)
                },
                y: if index == 0 { 0.0 } else { BAND_WORKERS },
                width: AGENT_WIDTH,
                height: AGENT_HEIGHT,
            });
        }
        (agents, paths)
    }
}

fn normalize_path(path: &str) -> String {
    path.strip_prefix("/workspace/")
        .or_else(|| path.strip_prefix("workspace/"))
        .unwrap_or(path)
        .trim_end_matches('/')
        .to_owned()
}

fn leased_paths(event: &Value) -> Vec<(String, String)> {
    let payload = &event["payload"];
    if !matches!(
        payload["state"]
            .as_str()
            .unwrap_or("installed")
            .to_ascii_lowercase()
            .as_str(),
        "installed" | "granted" | "active"
    ) {
        return Vec::new();
    }
    let mut paths = payload["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|capability| {
            let kind = capability["kind"].as_str()?;
            if !matches!(kind.to_ascii_lowercase().as_str(), "object" | "view") {
                return None;
            }
            Some((capability["label"].as_str()?.to_owned(), kind.to_owned()))
        })
        .collect::<Vec<_>>();
    if payload["kind"]
        .as_str()
        .is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "object" | "view"))
        && let Some(path) = payload["label"].as_str()
    {
        paths.push((
            path.to_owned(),
            payload["kind"].as_str().unwrap_or("access").to_owned(),
        ));
    }
    paths
}

/// The design's node width: wide enough for the name, floored at 120.
fn node_width(name: &str) -> f32 {
    (120.0f32).max(40.0 + name.chars().count() as f32 * 8.4) + 42.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn events() -> Vec<Value> {
        vec![
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace", "path": "src/main.rs"}}),
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace", "path": "src/lib.rs"}}),
            json!({"kind": "tool_call", "generation": 2, "payload": {"tool": "read", "path": "~/.cargo/registry"}}),
        ]
    }

    fn built() -> (Graph, Filesystem) {
        let events = events();
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        (graph, filesystem)
    }

    #[test]
    fn a_touched_path_brings_its_ancestors_with_it() {
        let (_, filesystem) = built();
        let names: Vec<&str> = filesystem.paths.iter().map(|p| p.path.as_str()).collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"src/main.rs"));
        assert!(names.contains(&"src/lib.rs"));
        // The ancestor is interned before the child that named it.
        let src = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src")
            .unwrap();
        let main = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src/main.rs")
            .unwrap();
        assert!(src < main);
        assert_eq!(filesystem.paths[main].parent, Some(src));
    }

    #[test]
    fn dynamic_resource_leases_and_typed_path_arguments_enter_the_graph() {
        let events = vec![
            json!({"kind":"capability","generation":4,"payload":{"state":"installed","capabilities":[
                {"kind":"view","label":"workspace/releases"},
                {"kind":"executable","label":"publish_release"}
            ]}}),
            json!({"kind":"tool_call","generation":4,"payload":{"command":"publish_release","arguments":[
                {"kind":"path","value":"/workspace/releases/v1/manifest.json"},
                {"kind":"utf8","value":"release, candidate"}
            ]}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        let worker = graph.index_of("agent-4").unwrap();
        let releases = filesystem
            .paths
            .iter()
            .position(|path| path.path == "releases")
            .unwrap();
        let manifest = filesystem
            .paths
            .iter()
            .position(|path| path.path == "releases/v1/manifest.json")
            .unwrap();

        assert!(filesystem.covers(worker, releases));
        assert!(filesystem.covers(worker, manifest));
        assert_eq!(filesystem.grants.len(), 2);
        assert!(
            filesystem
                .paths
                .iter()
                .all(|path| !path.path.contains("release, candidate"))
        );
    }

    #[test]
    fn a_path_outside_the_workspace_is_marked_external_and_has_no_parent() {
        let (_, filesystem) = built();
        let external = filesystem
            .paths
            .iter()
            .find(|p| p.path == "~/.cargo/registry")
            .unwrap();
        assert_eq!(external.kind, Kind::External);
        assert_eq!(external.parent, None);
        assert_eq!(external.name, "registry");
    }

    #[test]
    fn a_directory_and_a_file_are_told_apart() {
        let (_, filesystem) = built();
        let src = filesystem.paths.iter().find(|p| p.path == "src").unwrap();
        let main = filesystem
            .paths
            .iter()
            .find(|p| p.path == "src/main.rs")
            .unwrap();
        assert_eq!(src.kind, Kind::Directory);
        assert_eq!(main.kind, Kind::File);
    }

    #[test]
    fn a_grant_on_a_directory_covers_what_is_beneath_it() {
        let events = vec![
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read", "path": "src"}}),
            json!({"kind": "tool_call", "generation": 2, "payload": {"tool": "read", "path": "src/deep/file.rs"}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        let src = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src")
            .unwrap();
        let deep = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src/deep/file.rs")
            .unwrap();
        let first = graph.index_of("agent-1").unwrap();

        // Granted the directory, so everything under it is reached.
        assert!(filesystem.covers(first, src));
        assert!(filesystem.covers(first, deep));
        assert!(filesystem.covered(first).contains(&deep));
    }

    #[test]
    fn coverage_does_not_flow_upwards() {
        let events = vec![
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read", "path": "src/main.rs"}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        let src = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src")
            .unwrap();
        let agent = graph.index_of("agent-1").unwrap();

        // Reaching a file says nothing about its directory.
        assert!(!filesystem.covers(agent, src));
    }

    #[test]
    fn holders_report_direct_authority_before_inherited() {
        let events = vec![
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read", "path": "src"}}),
            json!({"kind": "tool_call", "generation": 2, "payload": {"tool": "read", "path": "src/main.rs"}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        let main = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src/main.rs")
            .unwrap();

        let holders = filesystem.holders(main);
        assert_eq!(holders.len(), 2);
        assert!(holders[0].1, "a direct grant is reported first");
        assert!(!holders[1].1, "an inherited one follows");
    }

    #[test]
    fn a_repeated_call_does_not_duplicate_a_grant() {
        let events = vec![
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read", "path": "src"}}),
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read", "path": "src"}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        assert_eq!(filesystem.grants.len(), 1);
    }

    #[test]
    fn bands_place_agents_above_the_paths_they_reach() {
        let (graph, filesystem) = built();
        let (agents, paths) = filesystem.place(&graph);

        assert_eq!(agents.len(), graph.agents.len());
        assert_eq!(paths.len(), filesystem.paths.len());
        assert_eq!(agents[0].y, 0.0, "the broker leads");
        assert!(agents[1..].iter().all(|a| a.y == BAND_WORKERS));
        let external = filesystem
            .paths
            .iter()
            .position(|p| p.kind == Kind::External)
            .unwrap();
        assert_eq!(paths[external].y, BAND_EXTERNAL);
        let workspace = filesystem
            .paths
            .iter()
            .position(|p| p.kind != Kind::External)
            .unwrap();
        assert!(paths[workspace].y >= BAND_WORKSPACE);
    }

    #[test]
    fn a_node_is_never_narrower_than_the_floor() {
        assert_eq!(node_width("a"), 120.0 + 42.0);
        assert!(node_width("a-very-long-path-segment-indeed") > 200.0);
    }
}
