//! What she takes after `/` in the app's composer: the skills the person is given, and the
//! commands her harness announced for the conversation (`EVENT_COMMANDS`).

use std::path::Path;

use ai6::AssistantCommand;
use ai6::directorates::q::overlays::skill_summary;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use super::assistant_settings;
use crate::{ApiError, Gateway, bearer, resolve_user};

/// The roster handler that keeps no list of allowed skills: the Station installs none for it,
/// so the menu offers none.
const CODEX: &str = "codex";

/// Whether her Agent can run a skill: not when every route it has is Codex's.
pub(super) async fn handler_runs_skills(gateway: &Gateway, agent_id: &str) -> bool {
    let Some(source) = gateway.roster.clone() else {
        return true;
    };
    let agent_id = agent_id.to_owned();
    tokio::task::spawn_blocking(move || {
        let Ok(snapshot) = source.load("") else {
            return true;
        };
        let Ok(agent) = snapshot.roster.agent(None, &agent_id) else {
            return true;
        };
        let routes = agent.route_list();
        routes.is_empty() || routes.iter().any(|route| route.handler != CODEX)
    })
    .await
    .unwrap_or(true)
}

/// The message to hand her harness for `message`, when it opens with `/` and a skill of
/// `skills`: it tells her to run that skill with the rest as its arguments. The person's
/// own text is what the conversation shows.
pub(super) fn invocation(message: &str, skills: &[String]) -> Option<String> {
    let rest = message.trim_start().strip_prefix('/')?;
    let (name, arguments) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(name, arguments)| (name, arguments.trim()));
    if !skills.iter().any(|skill| skill == name) {
        return None;
    }
    Some(if arguments.is_empty() {
        format!(
            "The person ran /{name}, your skill ai6:{name}. Run it now with the Skill tool, \
             with no arguments."
        )
    } else {
        format!(
            "The person ran /{name}, your skill ai6:{name}. Run it now with the Skill tool; \
             its arguments are what follows.\n\n{arguments}"
        )
    })
}

#[derive(Deserialize)]
pub(crate) struct CommandsQuery {
    /// The conversation whose harness's commands are wanted too.
    conversation: Option<String>,
}

/// `GET /v1/user/moneypenny/commands[?conversation=ID]`: the commands the composer offers
/// after `/`, as `{"commands": [{"name", "description", "argument_hint", "source"}]}`. A
/// `skill` is one of the catalog the person is given (less what her handler cannot run and
/// what the skill keeps for the model); a `harness` command is one the conversation's harness
/// announced. A skill and a harness command of one name show once, as the skill.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<CommandsQuery>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let assistant = gateway.assistant.clone().ok_or_else(ApiError::not_found)?;
    let settings = assistant_settings(&gateway)
        .await
        .map_err(|_| ApiError::unavailable())?;
    let mut commands: Vec<Value> = Vec::new();
    if handler_runs_skills(&gateway, &assistant.config.agent_id).await {
        let paths: Vec<(String, String)> = settings
            .skills_for(&person.id, &person.groups)
            .into_iter()
            .filter_map(|name| {
                let path = settings.skills().get(&name)?.path().to_owned();
                Some((name, path))
            })
            .collect();
        let read = tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .filter_map(|(name, path)| {
                    let summary = skill_summary(Path::new(&path))?;
                    summary.user_invocable.then_some((name, summary))
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| ApiError::unavailable())?;
        commands.extend(read.into_iter().map(|(name, summary)| {
            command(&name, &summary.description, summary.argument_hint, "skill")
        }));
    }
    if let Some(id) = query.conversation.as_deref() {
        let announced: Vec<AssistantCommand> = {
            let mut desk = assistant.desk();
            desk.store
                .owned(id, &person.id)
                .map(|conversation| conversation.commands.clone())
                .ok_or_else(ApiError::not_found)?
        };
        for announced in announced {
            if commands.iter().all(|shown| shown["name"] != announced.name) {
                commands.push(command(
                    &announced.name,
                    &announced.description,
                    announced.input_hint,
                    "harness",
                ));
            }
        }
    }
    Ok(Json(json!({"commands": commands})))
}

fn command(name: &str, description: &str, argument_hint: Option<String>, source: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "argument_hint": argument_hint,
        "source": source,
    })
}

#[cfg(test)]
mod tests {
    use super::invocation;

    #[test]
    fn only_a_skill_she_has_is_run() {
        let skills = ["charts".to_owned()];
        assert!(
            invocation("/charts", &skills)
                .unwrap()
                .contains("ai6:charts")
        );
        let with = invocation("  /charts   last week ", &skills).unwrap();
        assert!(with.ends_with("last week"), "{with}");
        assert_eq!(invocation("/other", &skills), None);
        assert_eq!(invocation("charts", &skills), None);
        assert_eq!(invocation("/chartsx", &skills), None);
        assert_eq!(invocation("/", &skills), None);
    }
}
