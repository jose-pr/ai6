//! Operation policies.

/// A rule or role of an Operation's record that is not applied, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    /// `rule <index>` or `role <name>`.
    pub item: String,
    /// Why it is skipped.
    pub reason: String,
}
