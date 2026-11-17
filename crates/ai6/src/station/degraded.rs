//! What the Station reports as degraded at check-in: the libraries it refused, the Sources it
//! cannot poll or write to, and Runs that make no progress.

use crate::directorates::m::placement::Degradation;
use crate::directorates::m::roster::Agent;
use crate::providers::FoundLibrary;

/// A degradation for each library found on the plugin paths that a pin set refused (a record
/// names the file and does not accept these bytes), naming its kind and what depends on it: the
/// Agents none of whose routes runs on a loaded handler, for a handler library.
pub(crate) fn refused_libraries(
    found: &[FoundLibrary],
    agents: &[Agent],
    loaded_handlers: &[String],
) -> Vec<Degradation> {
    found
        .iter()
        .filter(|library| library.verdict.starts_with("refused"))
        .map(|library| {
            let kinds = library.kinds.join(", ");
            let depends = if library.kinds.iter().any(|kind| kind.contains("agent_handler")) {
                let stranded: Vec<&str> = agents
                    .iter()
                    .filter(|agent| !agent.runs_on(loaded_handlers))
                    .map(|agent| agent.id.as_str())
                    .collect();
                vec![format!("Agents without a loaded handler: {}", stranded.join(", "))]
            } else {
                vec![format!("providers of kind {kinds}")]
            };
            Degradation::new(
                "library",
                library.file.clone(),
                format!("not loaded, {} ({kinds})", library.verdict),
                depends,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(file: &str, verdict: &str, kind: &str) -> FoundLibrary {
        FoundLibrary {
            file: file.to_owned(),
            digest: "sha256:0".to_owned(),
            kinds: vec![kind.to_owned()],
            declared: Vec::new(),
            verdict: verdict.to_owned(),
            signed: false,
            publisher: None,
        }
    }

    #[test]
    fn a_refused_library_is_a_degradation_naming_it_its_kind_and_what_needs_it() {
        let mut agent = Agent::new("007");
        agent.handler = Some("claude".to_owned());
        let found = [
            library("claude.dll", "refused: digest differs from its pin", "ai6_agent_handler"),
            library("spare.dll", "unpinned", "ai6_terminal"),
        ];
        let degraded = refused_libraries(&found, &[agent], &["codex".to_owned()]);
        assert_eq!(degraded.len(), 1, "an unpinned library awaits approval; it is not refused");
        assert_eq!(degraded[0].subject, "claude.dll");
        assert!(degraded[0].reason.contains("ai6_agent_handler"), "{degraded:?}");
        assert!(degraded[0].depends[0].contains("007"), "{degraded:?}");
    }
}
