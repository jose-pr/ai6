//! What a Station's check-in asks of the Registry. On a distant database every transaction costs
//! two round trips besides its statements, so the count must not grow with the roster.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::tests::test_state;
use super::*;
use crate::SqlRegistry;

const OPERATIONS: usize = 5;
const AGENTS: usize = 11;

/// A Station posting `AGENTS` Agents to each of `OPERATIONS` Operations, on a SQLite Registry.
fn station_on_sqlite(root: &Path) -> (DaemonState, Arc<SqlRegistry>) {
    let agent_ids: Vec<String> = (1..=AGENTS).map(|number| format!("1{number:02}")).collect();
    let agents: String = agent_ids
        .iter()
        .map(|id| format!("[agent.{id}]\nhandler = \"mock-agent\"\n"))
        .collect();
    let mut state = test_state(root, "", &agents);
    let roster = agent_ids
        .iter()
        .map(|id| format!("{id:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut operations = String::new();
    for number in 1..=OPERATIONS {
        let checkout = root.join(format!("checkout-{number}"));
        fs::create_dir_all(&checkout).unwrap();
        operations.push_str(&format!(
            "[operation.O{number}]\npath = {:?}\nroster = [{roster}]\n",
            checkout.to_string_lossy()
        ));
    }
    fs::write(root.join("operations.toml"), operations).unwrap();
    state.roster = Arc::new(Roster::load(root).unwrap());
    let registry = Arc::new(
        SqlRegistry::open(&format!("sqlite:{}", root.join("registry.db").display())).unwrap(),
    );
    state.intake = registry.clone();
    (state, registry)
}

#[test]
fn a_check_in_asks_the_registry_for_a_count_that_does_not_grow_with_the_roster() {
    let root = tempfile::tempdir().unwrap();
    let (state, registry) = station_on_sqlite(root.path());
    // The first round also makes what a Station finds missing in a new database.
    check_in_round(&state);

    let (transactions, statements) = registry.tally();
    check_in_round(&state);
    let (after_transactions, after_statements) = registry.tally();
    let (transactions, statements) = (
        after_transactions - transactions,
        after_statements - statements,
    );

    // Each Agent's clearance grant read in a transaction of its own, and each setting too, took
    // 59 transactions and 62 statements on this roster.
    assert!(transactions <= 3, "{transactions} transactions");
    assert!(statements <= 8, "{statements} statements");
}
