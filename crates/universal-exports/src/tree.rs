//! The tree of Operations, as the Stations publish it at check-in.
//!
//! Universal Exports has no roster of its own: each Station's check-in carries
//! `operation_lineage`, every roster Operation's ancestors nearest first, and the most recent
//! check-in that carries one is taken as the tree. An Operation no check-in names has no
//! ancestors, so only rules on itself and on `*` apply to it.

use std::collections::BTreeMap;

use ai6::{AuthorizationRequest, Person};

/// Each Operation's ancestors, nearest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OperationTree {
    lineage: BTreeMap<String, Vec<String>>,
    /// The names the Operations are called by, where they have one.
    names: BTreeMap<String, String>,
}

impl OperationTree {
    /// The tree the most recent check-in carrying `operation_lineage` published; empty when
    /// none does. Entries that are not a list of IDs are left out.
    pub(crate) fn from_check_ins(records: &[ai6::StationRecord]) -> Self {
        let Some((published, names)) = records
            .iter()
            .filter_map(|record| {
                record
                    .info()
                    .get("operation_lineage")
                    .and_then(serde_json::Value::as_object)
                    .map(|lineage| (record.seen(), lineage, record.info().get("operation_names")))
            })
            .max_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, lineage, names)| (lineage, names))
        else {
            return Self::default();
        };
        let lineage = published
            .iter()
            .filter_map(|(operation, ancestors)| {
                let ancestors = ancestors
                    .as_array()?
                    .iter()
                    .map(|ancestor| ancestor.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?;
                Some((operation.clone(), ancestors))
            })
            .collect();
        let names = names
            .and_then(serde_json::Value::as_object)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|(operation, name)| {
                        Some((operation.clone(), name.as_str()?.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { lineage, names }
    }

    /// The name `operation` is called by, when it has one.
    pub(crate) fn name(&self, operation: &str) -> Option<String> {
        self.names.get(operation).cloned()
    }

    /// A tree of these lineages, for tests.
    #[cfg(test)]
    pub(crate) fn of(lineage: &[(&str, &[&str])]) -> Self {
        Self {
            names: BTreeMap::new(),
            lineage: lineage
                .iter()
                .map(|(operation, ancestors)| {
                    (
                        (*operation).to_owned(),
                        ancestors.iter().map(|id| (*id).to_owned()).collect(),
                    )
                })
                .collect(),
        }
    }

    /// The Operations `operation` is subordinate to, nearest first.
    pub(crate) fn ancestors(&self, operation: &str) -> &[String] {
        self.lineage.get(operation).map_or(&[], Vec::as_slice)
    }

    /// Every Operation the tree names, in ID order.
    pub(crate) fn operations(&self) -> impl Iterator<Item = &str> {
        self.lineage.keys().map(String::as_str)
    }

    /// Whether `operation` is `root` or sits under it.
    pub(crate) fn in_subtree(&self, operation: &str, root: &str) -> bool {
        operation == root
            || self
                .ancestors(operation)
                .iter()
                .any(|ancestor| ancestor == root)
    }

    /// The nearest of `operation`'s ancestors for which `visible` holds; `None` when there is
    /// none, so an ancestor that is not visible is never named.
    pub(crate) fn visible_parent(
        &self,
        operation: &str,
        visible: impl Fn(&str) -> bool,
    ) -> Option<String> {
        self.ancestors(operation)
            .iter()
            .find(|ancestor| visible(ancestor))
            .cloned()
    }
}

/// The authorization resource of an Operation: `operation:<id>`.
pub(crate) fn operation_resource(operation: &str) -> String {
    format!("operation:{operation}")
}

/// A request for `person` to take `action` on `operation`, naming its ancestors from `tree`
/// so a rule on a parent Operation reaches it.
pub(crate) fn operation_request(
    person: Person,
    action: &str,
    operation: &str,
    tree: &OperationTree,
) -> AuthorizationRequest {
    AuthorizationRequest {
        person,
        action: action.to_owned(),
        resource: operation_resource(operation),
        ancestors: tree
            .ancestors(operation)
            .iter()
            .map(|ancestor| operation_resource(ancestor))
            .collect(),
    }
}

/// A request for `person` to take `action` on `resource`; an Operation's (`operation:<id>`)
/// names its ancestors as [`operation_request`] does, any other resource none.
pub(crate) fn resource_request(
    person: Person,
    action: &str,
    resource: String,
    tree: &OperationTree,
) -> AuthorizationRequest {
    match resource.strip_prefix("operation:") {
        Some(operation) => operation_request(person, action, operation, tree),
        None => AuthorizationRequest {
            person,
            action: action.to_owned(),
            resource,
            ancestors: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_its_ancestors_as_operation_scopes() {
        let tree = OperationTree::of(&[("G", &["P", "ROOT"]), ("P", &["ROOT"]), ("ROOT", &[])]);
        let request = operation_request(Person::default(), "mission.read", "G", &tree);
        assert_eq!(request.resource, "operation:G");
        assert_eq!(request.ancestors, ["operation:P", "operation:ROOT"]);
        assert!(
            operation_request(Person::default(), "mission.read", "X", &tree)
                .ancestors
                .is_empty()
        );
        assert!(tree.in_subtree("G", "ROOT") && tree.in_subtree("P", "P"));
        assert!(!tree.in_subtree("ROOT", "G"));
        assert_eq!(
            tree.visible_parent("G", |id| id == "ROOT").as_deref(),
            Some("ROOT"),
            "a hidden P lifts G to its nearest visible ancestor"
        );
        assert_eq!(tree.visible_parent("G", |_| false), None);
        let roster = resource_request(
            Person::default(),
            "roster.read",
            "config:roster".into(),
            &tree,
        );
        assert!(roster.ancestors.is_empty());
        assert_eq!(
            resource_request(Person::default(), "x", "operation:P".into(), &tree).ancestors,
            ["operation:ROOT"]
        );
    }
}
