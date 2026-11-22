use super::*;

fn commit(sha: &str, parents: &[&str]) -> Commit {
    Commit {
        sha: sha.to_owned(),
        short_sha: sha.to_owned(),
        parents: parents.iter().map(|parent| parent.to_string()).collect(),
        subject: String::new(),
        author: String::new(),
        relative_date: String::new(),
        refs: Vec::new(),
    }
}

#[test]
fn linear_history_stays_in_one_lane() {
    let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
    assert!(rows.iter().all(|row| row.lane == 0 && row.width == 1));
    assert_eq!(rows[0].segments, vec![Segment::OutOfNode { color: 0 }]);
    assert_eq!(
        rows[1].segments,
        vec![
            Segment::IntoNode { color: 0 },
            Segment::OutOfNode { color: 0 }
        ]
    );
    assert_eq!(rows[2].segments, vec![Segment::IntoNode { color: 0 }]);
    assert!(rows[2].lanes_below.is_empty());
}

#[test]
fn merge_opens_a_lane_for_the_feature_branch_and_closes_it() {
    // m merges feature (f2 -> f1) into main (b), both branched from a.
    let rows = layout(&[
        commit("m", &["b", "f2"]),
        commit("f2", &["f1"]),
        commit("b", &["a"]),
        commit("f1", &["a"]),
        commit("a", &[]),
    ]);

    let merge = &rows[0];
    assert!(merge.is_merge);
    assert_eq!(merge.lane, 0);
    assert!(merge.segments.contains(&Segment::BranchOut {
        to_lane: 1,
        color: 1
    }));
    assert_eq!(merge.lanes_below, vec![(0, 0), (1, 1)]);

    // The feature commits sit in lane 1 while main's line crosses lane 0.
    assert_eq!(rows[1].lane, 1);
    assert!(
        rows[1]
            .segments
            .contains(&Segment::Through { lane: 0, color: 0 })
    );
    assert_eq!(rows[2].lane, 0);
    assert!(
        rows[2]
            .segments
            .contains(&Segment::Through { lane: 1, color: 1 })
    );
    assert_eq!(rows[3].lane, 1);

    // The fork point pulls the feature lane back into main.
    let fork = &rows[4];
    assert_eq!(fork.lane, 0);
    assert!(fork.segments.contains(&Segment::MergeIn {
        from_lane: 1,
        color: 1
    }));
    assert!(fork.lanes_below.is_empty());
    assert_eq!(fork.width, 2);
}

#[test]
fn separate_branch_tips_get_their_own_lanes_and_colors() {
    // Two branches (x and y) whose tips are both listed, joining at a.
    let rows = layout(&[commit("x", &["a"]), commit("y", &["a"]), commit("a", &[])]);
    assert_eq!((rows[0].lane, rows[0].color), (0, 0));
    assert_eq!((rows[1].lane, rows[1].color), (1, 1));
    assert!(
        rows[1]
            .segments
            .contains(&Segment::Through { lane: 0, color: 0 })
    );
    assert!(!rows[1].segments.contains(&Segment::IntoNode { color: 1 }));
    assert!(rows[2].segments.contains(&Segment::MergeIn {
        from_lane: 1,
        color: 1
    }));
}

#[test]
fn freed_lanes_are_reused() {
    let rows = layout(&[
        commit("m", &["b", "f"]),
        commit("f", &["a"]),
        commit("b", &["a"]),
        commit("a", &["z"]),
        commit("t", &["z"]),
        commit("z", &[]),
    ]);
    // After the feature lane closes at a, the next tip reuses lane 1.
    assert_eq!(rows[4].lane, 1);
}
