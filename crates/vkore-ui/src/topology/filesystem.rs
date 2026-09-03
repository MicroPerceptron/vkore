//! The permission graph: which agent reaches which path, and how.
//!
//! Two kinds of edge carry two different claims. Containment says a path sits
//! inside another, and is a fact about the workspace. A grant says an agent
//! holds authority over a path, and is a fact about the session. Authority
//! flows down containment: a grant on a directory covers everything beneath
//! it, which is why selecting an agent lights more than the paths it was
//! granted directly.
//!
//! Paths are discovered from the tool calls that touched them. Nothing is
//! assumed to exist because a workspace usually has it.

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
            if event["kind"] != "tool_call" {
                continue;
            }
            let Some(call) = crate::decision_payload(event) else {
                continue;
            };
            let Some(path) = call["path"]
                .as_str()
                .or_else(|| call["arguments"]["path"].as_str())
            else {
                continue;
            };
            let index = filesystem.intern(path);
            let generation = event["generation"].as_u64().unwrap_or_default();
            let Some(agent) = graph
                .agents
                .iter()
                .position(|agent| agent.generation == generation)
            else {
                continue;
            };
            let rights = call["tool"]
                .as_str()
                .or_else(|| call["command"].as_str())
                .unwrap_or("access")
                .to_owned();
            if !filesystem
                .grants
                .iter()
                .any(|grant| grant.agent == agent && grant.path == index)
            {
                filesystem.grants.push(Grant {
                    agent,
                    path: index,
                    rights,
                });
            }
        }
        filesystem
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
        let mut agents = Vec::new();
        let mut cursor = 0.0;
        for (index, _) in graph.agents.iter().enumerate() {
            let broker = index == 0;
            agents.push(Placement {
                x: if broker { 0.0 } else { cursor },
                y: if broker { 0.0 } else { BAND_WORKERS },
                width: AGENT_WIDTH,
                height: AGENT_HEIGHT,
            });
            if !broker {
                cursor += AGENT_WIDTH + AGENT_GAP;
            }
        }
        // The broker sits centred over the workers it spawned.
        if agents.len() > 1 {
            let span = cursor - AGENT_GAP - AGENT_WIDTH;
            agents[0].x = span / 2.0;
        }

        let mut paths = vec![
            Placement {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: PATH_HEIGHT,
            };
            self.paths.len()
        ];
        let mut external_cursor = 0.0;
        let mut depth_cursor: Vec<f32> = Vec::new();
        for (index, node) in self.paths.iter().enumerate() {
            let width = node_width(&node.name);
            paths[index].width = width;
            if node.kind == Kind::External {
                paths[index].x = external_cursor;
                paths[index].y = BAND_EXTERNAL;
                external_cursor += width + PATH_GAP;
                continue;
            }
            let depth = self.depth(index);
            if depth_cursor.len() <= depth {
                depth_cursor.resize(depth + 1, 0.0);
            }
            paths[index].x = depth_cursor[depth];
            paths[index].y = BAND_WORKSPACE + depth as f32 * (PATH_HEIGHT + ROW_GAP);
            depth_cursor[depth] += width + PATH_GAP;
        }
        (agents, paths)
    }

    fn depth(&self, index: usize) -> usize {
        let mut depth = 0;
        let mut current = self.paths.get(index).and_then(|node| node.parent);
        while let Some(parent) = current {
            depth += 1;
            current = self.paths.get(parent).and_then(|node| node.parent);
        }
        depth
    }
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
