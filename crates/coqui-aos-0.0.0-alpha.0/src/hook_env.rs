//! What runs a hook: the order a scope's hooks of one event run in, and the environment each gets
//! (`spec/v0/hooks.md`).

use alloc::collections::BTreeMap;

use crate::apply::{Environment, Facts, apply_overlay};
use crate::compose::{Cycle, composition_order, resolve};
use crate::error::Error;
use crate::hooks::{HookEvent, HookRun, HookScope, fire_event};
use crate::names::{normalize_name, root_variable, state_variable};
use crate::read::Document;

/// The order the hooks of one event run in, as `(scope, source)` pairs.
///
/// Every active overlay that has a hook (`has`) in composition order, then the user's own, then
/// the project's, whichever order `scopes` (the scopes that have one) names them in; a scope's own
/// hook has the source `""`.
///
/// # Errors
///
/// A [`Cycle`] when `requires` loops.
pub fn hook_order(
    overlays: &BTreeMap<String, Document>,
    requested: &[String],
    has: &[String],
    scopes: &[HookScope],
) -> Result<Vec<(HookScope, String)>, Cycle> {
    let active = composition_order(overlays, &resolve(overlays, requested)?.install_order);
    let mut order: Vec<(HookScope, String)> = active
        .into_iter()
        .filter(|identity| has.contains(identity))
        .map(|identity| (HookScope::Overlay, identity))
        .collect();
    order.extend(
        [HookScope::User, HookScope::Project]
            .into_iter()
            .filter(|scope| scopes.contains(scope))
            .map(|scope| (scope, String::new())),
    );
    Ok(order)
}

/// What one earlier hook of an event set, `None` unsetting.
pub type EnvSet = Vec<(String, Option<String>)>;

fn set_all(env: &mut Environment, set: &[(String, Option<String>)]) {
    for (name, value) in set {
        match value {
            Some(value) => env.set(name, value.clone(), false),
            None => env.unset(name),
        }
    }
}

fn own_variables(env: &mut Environment, chosen: &Facts) {
    env.set("AOS_OVERLAY_ROOT", chosen.root.clone(), false);
    env.set("AOS_OVERLAY_NAME", normalize_name(&chosen.identity), false);
    if let Some(state) = &chosen.state {
        env.set("AOS_OVERLAY_STATE", state.clone(), false);
    }
}

fn chosen<'a>(active: &'a [(&Document, Facts)], identity: &str) -> Result<&'a Facts, Error> {
    active
        .iter()
        .map(|(_, facts)| facts)
        .find(|facts| facts.identity == identity)
        .ok_or_else(|| Error::new(crate::Class::UnresolvedReference, identity))
}

/// The session's environment at one point of the order of application: the base, every overlay's
/// root and state variable, the `pre-env` output of the overlays before `until` (all when `None`),
/// then, past `pre-env`, every overlay's declarations up to and including `through` (all when
/// `None`) and what the `earlier` hooks set.
fn session_environment(
    event: HookEvent,
    active: &[(&Document, Facts)],
    base: &Environment,
    pre_env: &BTreeMap<String, HookRun>,
    until: Option<&str>,
    through: Option<&str>,
    earlier: &[EnvSet],
) -> Result<Environment, Error> {
    let mut env = base.clone();
    for (_, facts) in active {
        env.set(&root_variable(&facts.identity), facts.root.clone(), false);
        if let Some(state) = &facts.state {
            env.set(&state_variable(&facts.identity), state.clone(), false);
        }
    }
    for (_, facts) in active {
        if until == Some(facts.identity.as_str()) {
            break;
        }
        let Some(run) = pre_env.get(&facts.identity) else {
            continue;
        };
        let mut run = run.clone();
        run.source.clone_from(&facts.identity);
        let fired = fire_event(
            HookEvent::PreEnv,
            &[run],
            None,
            &facts.settings,
            env.windows(),
        );
        set_all(&mut env, &fired.env);
    }
    if event == HookEvent::PreEnv {
        return Ok(env);
    }
    for (manifest, facts) in active {
        apply_overlay(&mut env, manifest, facts, &mut Vec::new(), &mut Vec::new())?;
        if through == Some(facts.identity.as_str()) {
            break;
        }
    }
    for set in earlier {
        set_all(&mut env, set);
    }
    Ok(env)
}

/// The environment a hook of `event` runs with: the session's.
///
/// `active` and `base` are as [`composed_environment`](crate::composed_environment) takes them. A
/// hook gets the base, every overlay's root (and state) variable, what every overlay's `pre-env`
/// hook set, every overlay's declarations in the order of application, and what the hooks of the
/// event before it set (`earlier`, one set for each, in run order). The hook of the overlay
/// `identity` also gets its `AOS_OVERLAY_ROOT`, `AOS_OVERLAY_NAME` and `AOS_OVERLAY_STATE` (when it
/// has one); every hook gets `AOS_HOOK_EVENT`. A `pre-env` hook runs before any declaration: it
/// gets the base, the root and state variables and the output of the `pre-env` hooks before it in
/// composition order, which for a user's or a project's own hook (`identity` `None`) is every
/// overlay's.
///
/// # Errors
///
/// What an overlay's declarations raise when applied, and `unresolved-reference` when `identity`
/// is not one of `active`.
pub fn hook_environment(
    event: HookEvent,
    active: &[(&Document, Facts)],
    base: &Environment,
    pre_env: &BTreeMap<String, HookRun>,
    identity: Option<&str>,
    earlier: &[EnvSet],
) -> Result<Environment, Error> {
    let own = identity
        .map(|identity| chosen(active, identity))
        .transpose()?;
    let until = identity.filter(|_| event == HookEvent::PreEnv);
    let mut env = session_environment(event, active, base, pre_env, until, None, earlier)?;
    if let Some(own) = own {
        own_variables(&mut env, own);
    }
    env.set("AOS_HOOK_EVENT", event.as_str().to_owned(), false);
    Ok(env)
}

/// A process one overlay runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverlayProcess {
    /// Its setup.
    Setup,
    /// One of its MCP servers.
    Server,
    /// One of its commands.
    Command,
    /// One of its env scripts, which runs while the environment is being built.
    EnvScript,
}

/// The environment a process of the overlay `identity` runs with.
///
/// A setup, a server or a command gets the session's, whole; an env script gets it as it stands
/// when the script runs: every earlier overlay's declarations and its own, no later overlay's.
/// Each also gets the overlay's `AOS_OVERLAY_*` variables. Arguments are as for
/// [`hook_environment`].
///
/// # Errors
///
/// As [`hook_environment`].
pub fn process_environment(
    process: OverlayProcess,
    active: &[(&Document, Facts)],
    base: &Environment,
    pre_env: &BTreeMap<String, HookRun>,
    identity: &str,
) -> Result<Environment, Error> {
    let own = chosen(active, identity)?;
    let through = (process == OverlayProcess::EnvScript).then_some(identity);
    let mut env = session_environment(
        HookEvent::SessionStart,
        active,
        base,
        pre_env,
        None,
        through,
        &[],
    )?;
    own_variables(&mut env, own);
    Ok(env)
}
