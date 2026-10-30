//! The tree of Operations as access sees it: each Operation's ancestors, nearest first.

use std::collections::BTreeMap;

use crate::directorates::m::roster::Roster;

/// Each known Operation and its ancestors, nearest first. An Operation the tree does not name
/// is unknown to access: no record of its own applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lineage {
    ancestors: BTreeMap<String, Vec<String>>,
}

impl Lineage {
    /// A tree of these `(Operation, ancestors nearest first)` pairs.
    pub fn new<A: Into<String>>(
        entries: impl IntoIterator<Item = (impl Into<String>, impl IntoIterator<Item = A>)>,
    ) -> Self {
        Self {
            ancestors: entries
                .into_iter()
                .map(|(operation, ancestors)| {
                    (
                        operation.into(),
                        ancestors.into_iter().map(Into::into).collect(),
                    )
                })
                .collect(),
        }
    }

    /// The tree of the roster's Operations.
    pub fn from_roster(roster: &Roster) -> Self {
        Self::new(
            roster
                .operations()
                .map(|operation| (operation.id.clone(), roster.lineage(&operation.id))),
        )
    }

    /// Whether the tree names `operation`.
    pub fn contains(&self, operation: &str) -> bool {
        self.ancestors.contains_key(operation)
    }

    /// The Operations `operation` is subordinate to, nearest first; empty for a root and for
    /// an Operation the tree does not name.
    pub fn ancestors(&self, operation: &str) -> &[String] {
        self.ancestors.get(operation).map_or(&[], Vec::as_slice)
    }

    /// Whether `operation` is `root` or sits under it.
    pub fn in_subtree(&self, operation: &str, root: &str) -> bool {
        operation == root
            || self
                .ancestors(operation)
                .iter()
                .any(|ancestor| ancestor == root)
    }

    /// Every Operation the tree names, in ID order.
    pub fn operations(&self) -> impl Iterator<Item = &str> {
        self.ancestors.keys().map(String::as_str)
    }

    /// `operation` and every Operation under it, in ID order.
    pub fn subtree(&self, root: &str) -> Vec<&str> {
        self.operations()
            .filter(|operation| self.in_subtree(operation, root))
            .collect()
    }
}
