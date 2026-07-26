//! `TabGroupLayoutNode` (`types.ts:768-777`) and `pruneGroupLayout`
//! (`O:30-49` of `workspace-session-terminal-tab-close.ts`).
//!
//! N2: a collapse (one child dies, one survives) REPLACES the split with the
//! surviving child verbatim (`O:42-47`, `return second` / `return first`) —
//! the split's own `direction`/`ratio` are silently discarded, never
//! re-wrapped around the survivor. Only when BOTH children survive does the
//! function build a new split node, and even then it reuses the ORIGINAL
//! node's `direction`/`ratio` via `{ ...node, first, second }` (`O:48`) — it
//! never invents new ones. If both children die, the split itself
//! disappears and the recursion returns `undefined` up to the parent, whose
//! own collapse rule then takes over. Child order (`first` stays `first`) is
//! never touched — no reordering, no rebalancing, no sorting.

use std::collections::HashSet;

/// `types.ts:766`, `'horizontal' | 'vertical'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabGroupSplitDirection {
    Horizontal,
    Vertical,
}

/// `types.ts:768-777`. The `ratio` field is `number | undefined` in the TS
/// source ("Flex ratio of the first child (0-1). Defaults to 0.5 if
/// absent.") — modeled as `Option<f64>` so an absent ratio round-trips as
/// `None`, never a synthesized `0.5`.
#[derive(Debug, Clone, PartialEq)]
pub enum TabGroupLayoutNode {
    Leaf {
        group_id: String,
    },
    Split {
        direction: TabGroupSplitDirection,
        first: Box<TabGroupLayoutNode>,
        second: Box<TabGroupLayoutNode>,
        ratio: Option<f64>,
    },
}

/// `pruneGroupLayout` (`O:30-49`). Recurses depth-first; a leaf survives iff
/// its `groupId` is in `valid_group_ids` (`O:38`). A split first prunes both
/// children (`O:40-41`), then: neither survives -> the whole split vanishes
/// (falls out of both `if`s to the final `None` below the match, mirroring
/// `!first` returning `second` which is itself `undefined`); exactly one
/// survives -> that child REPLACES the split outright, dropping
/// `direction`/`ratio` (`O:42-47`); both survive -> a new split is built
/// reusing the original `direction`/`ratio` (`O:48`).
pub fn prune_group_layout(
    node: Option<&TabGroupLayoutNode>,
    valid_group_ids: &HashSet<String>,
) -> Option<TabGroupLayoutNode> {
    let node = node?;
    match node {
        TabGroupLayoutNode::Leaf { group_id } => {
            if valid_group_ids.contains(group_id) {
                Some(node.clone())
            } else {
                None
            }
        }
        TabGroupLayoutNode::Split {
            direction,
            first,
            second,
            ratio,
        } => {
            let pruned_first = prune_group_layout(Some(first), valid_group_ids);
            let pruned_second = prune_group_layout(Some(second), valid_group_ids);
            // O:42-44: `if (!first) return second` — checked BEFORE the
            // second-child check, so `(None, None)` also lands here and
            // correctly yields `None` (not a fabricated leaf).
            if pruned_first.is_none() {
                return pruned_second;
            }
            // O:45-47: `if (!second) return first` — `first` is guaranteed
            // `Some` at this point.
            if pruned_second.is_none() {
                return pruned_first;
            }
            Some(TabGroupLayoutNode::Split {
                direction: *direction,
                first: Box::new(pruned_first.unwrap()),
                second: Box::new(pruned_second.unwrap()),
                ratio: *ratio,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(group_id: &str) -> TabGroupLayoutNode {
        TabGroupLayoutNode::Leaf {
            group_id: group_id.to_string(),
        }
    }

    fn split(
        direction: TabGroupSplitDirection,
        first: TabGroupLayoutNode,
        second: TabGroupLayoutNode,
        ratio: Option<f64>,
    ) -> TabGroupLayoutNode {
        TabGroupLayoutNode::Split {
            direction,
            first: Box::new(first),
            second: Box::new(second),
            ratio,
        }
    }

    fn ids(members: &[&str]) -> HashSet<String> {
        members.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn n2_both_children_survive_keeps_split_direction_and_ratio() {
        let tree = split(
            TabGroupSplitDirection::Vertical,
            leaf("group-a"),
            leaf("group-b"),
            Some(0.3),
        );
        let pruned = prune_group_layout(Some(&tree), &ids(&["group-a", "group-b"])).unwrap();
        assert_eq!(pruned, tree);
    }

    #[test]
    fn n2_single_child_collapse_loses_direction_and_ratio() {
        let tree = split(
            TabGroupSplitDirection::Horizontal,
            leaf("group-a"),
            leaf("group-b"),
            Some(0.75),
        );
        let pruned = prune_group_layout(Some(&tree), &ids(&["group-b"]));
        // The surviving leaf REPLACES the split outright — no split wrapper,
        // no `direction`/`ratio` anywhere in the result.
        assert_eq!(pruned, Some(leaf("group-b")));
    }

    #[test]
    fn n2_both_children_die_removes_the_split_itself() {
        let tree = split(
            TabGroupSplitDirection::Horizontal,
            leaf("group-a"),
            leaf("group-b"),
            Some(0.5),
        );
        assert_eq!(prune_group_layout(Some(&tree), &ids(&[])), None);
    }

    #[test]
    fn n2_root_dying_yields_none_so_caller_deletes_the_layout_key() {
        let tree = leaf("group-a");
        assert_eq!(prune_group_layout(Some(&tree), &ids(&[])), None);
    }

    #[test]
    fn n2_three_level_nested_tree_prunes_the_dead_branch_and_keeps_order() {
        // root
        //  ├── split(inner: leaf(a) | leaf(b))
        //  └── leaf(c)
        // "a" dies -> inner split collapses to leaf(b) (losing inner's own
        // direction/ratio), which then becomes `first` of the root split
        // (root's OWN direction/ratio survive since both root children live).
        let inner = split(
            TabGroupSplitDirection::Vertical,
            leaf("group-a"),
            leaf("group-b"),
            Some(0.4),
        );
        let root = split(
            TabGroupSplitDirection::Horizontal,
            inner,
            leaf("group-c"),
            Some(0.6),
        );
        let pruned = prune_group_layout(Some(&root), &ids(&["group-b", "group-c"])).unwrap();
        assert_eq!(
            pruned,
            split(
                TabGroupSplitDirection::Horizontal,
                leaf("group-b"),
                leaf("group-c"),
                Some(0.6)
            )
        );
    }

    #[test]
    fn n2_child_order_is_preserved_first_stays_first_when_both_survive() {
        let tree = split(
            TabGroupSplitDirection::Vertical,
            leaf("group-a"),
            leaf("group-b"),
            None,
        );
        let pruned = prune_group_layout(Some(&tree), &ids(&["group-a", "group-b"])).unwrap();
        match pruned {
            TabGroupLayoutNode::Split { first, second, .. } => {
                assert_eq!(*first, leaf("group-a"));
                assert_eq!(*second, leaf("group-b"));
            }
            _ => panic!("expected a split"),
        }
    }

    #[test]
    fn none_node_prunes_to_none() {
        assert_eq!(prune_group_layout(None, &ids(&["group-a"])), None);
    }
}
