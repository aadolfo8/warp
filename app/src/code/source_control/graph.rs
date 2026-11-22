//! Lays out the commit graph drawn next to the history, like VS Code's
//! Source Control Graph: every branch line gets a lane (a column) and a
//! color, and each row says which lines cross it and where they bend.
//!
//! Commits must be in topological order (children before parents), as
//! `git log --topo-order` prints them.

use super::git::Commit;

/// A line drawn in one row of the graph. Lanes are column indexes; the commit
/// node sits in the middle of the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// A line crossing the row in `lane` without touching the node.
    Through { lane: usize, color: usize },
    /// The line arriving from the row above into the node, in the node's lane.
    IntoNode { color: usize },
    /// The line leaving the node to the row below, in the node's lane.
    OutOfNode { color: usize },
    /// A branch line from `from_lane` above bending into the node (a branch
    /// that started at this commit).
    MergeIn { from_lane: usize, color: usize },
    /// A line from the node bending down into `to_lane` (a merge's other
    /// parent).
    BranchOut { to_lane: usize, color: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub is_merge: bool,
    pub segments: Vec<Segment>,
    /// Lanes (with colors) that continue below this row. Rows inserted under
    /// it, like the files of an expanded commit, draw these straight through.
    pub lanes_below: Vec<(usize, usize)>,
    /// How many lanes this row uses, for its width.
    pub width: usize,
}

/// How many distinct colors lanes cycle through.
pub const COLOR_COUNT: usize = 5;

#[derive(Clone)]
struct Lane {
    /// The commit this lane's line is heading to.
    sha: String,
    color: usize,
}

pub fn layout(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0;
    let mut new_color = || {
        let color = next_color % COLOR_COUNT;
        next_color += 1;
        color
    };
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let lanes_above = lanes.clone();
        // Lanes that were waiting for this commit.
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.as_ref().is_some_and(|lane| lane.sha == commit.sha))
            .map(|(index, _)| index)
            .collect();

        let (lane, color, has_line_above) = match waiting.first() {
            Some(&first) => (first, lanes[first].as_ref().map_or(0, |l| l.color), true),
            None => {
                // A branch tip: take the first free lane.
                let index = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                (index, new_color(), false)
            }
        };
        if lane == lanes.len() {
            lanes.push(None);
        }

        let mut segments = Vec::new();
        if has_line_above {
            segments.push(Segment::IntoNode { color });
        }
        for &other in waiting.iter().skip(1) {
            let other_color = lanes[other].as_ref().map_or(color, |l| l.color);
            segments.push(Segment::MergeIn {
                from_lane: other,
                color: other_color,
            });
            lanes[other] = None;
        }

        // The first parent continues in this lane.
        lanes[lane] = commit.parents.first().map(|parent| Lane {
            sha: parent.clone(),
            color,
        });
        if lanes[lane].is_some() {
            segments.push(Segment::OutOfNode { color });
        }

        // Other parents of a merge: join a lane already heading there, or open one.
        for parent in commit.parents.iter().skip(1) {
            let existing = lanes
                .iter()
                .position(|lane| lane.as_ref().is_some_and(|lane| &lane.sha == parent));
            let (to_lane, parent_color) = match existing {
                Some(index) => (index, lanes[index].as_ref().map_or(0, |l| l.color)),
                None => {
                    let index = lanes
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or(lanes.len());
                    let parent_color = new_color();
                    let new_lane = Some(Lane {
                        sha: parent.clone(),
                        color: parent_color,
                    });
                    if index == lanes.len() {
                        lanes.push(new_lane);
                    } else {
                        lanes[index] = new_lane;
                    }
                    (index, parent_color)
                }
            };
            segments.push(Segment::BranchOut {
                to_lane,
                color: parent_color,
            });
        }

        // Lines that cross this row untouched: busy above and below, not
        // involved with this commit.
        for (index, above) in lanes_above.iter().enumerate() {
            let Some(above) = above else { continue };
            if index == lane || waiting.contains(&index) {
                continue;
            }
            let continues = lanes
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|below| below.sha == above.sha);
            if continues {
                segments.push(Segment::Through {
                    lane: index,
                    color: above.color,
                });
            }
        }

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let lanes_below: Vec<(usize, usize)> = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| lane.as_ref().map(|lane| (index, lane.color)))
            .collect();
        let width = lanes_above.len().max(lanes.len()).max(lane + 1).max(
            segments
                .iter()
                .map(|segment| match segment {
                    Segment::Through { lane, .. } => lane + 1,
                    Segment::MergeIn { from_lane, .. } => from_lane + 1,
                    Segment::BranchOut { to_lane, .. } => to_lane + 1,
                    Segment::IntoNode { .. } | Segment::OutOfNode { .. } => 0,
                })
                .max()
                .unwrap_or(0),
        );

        rows.push(GraphRow {
            lane,
            color,
            is_merge: commit.parents.len() > 1,
            segments,
            lanes_below,
            width,
        });
    }
    rows
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
