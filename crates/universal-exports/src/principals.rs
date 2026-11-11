//! Principals: the names of the people the gateway has seen sign in.
//!
//! Every signed-in request tells the gateway who the person is; it keeps what the directory
//! said (the name to show, the account name, the groups) in the Registry so an audit record,
//! an access rule or a page can name a person who is not asking now. The name is for display
//! only and decides nothing: a person never seen has no name here, and the caller shows their
//! ID. `POST /v1/user/principals` resolves IDs and `GET /v1/user/principals?q=` finds people
//! and groups by name, for the pickers.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::Instant;

use ai6::{Authorizer as _, CONFIG_ACCESS, Person, PrincipalRecord};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::tree::operation_request;
use crate::{
    ApiError, Gateway, audit_page, bearer, known_operations, operation_tree, resolve_user,
    resource_action_allowed, unix_timestamp,
};

/// What was last written for each person, so a request that changes nothing costs no Registry
/// access.
#[derive(Default)]
pub(crate) struct Sightings(Mutex<HashMap<String, (u64, Instant)>>);

fn fingerprint(person: &Person) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        &person.id,
        &person.name,
        &person.display_name,
        &person.groups,
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// Notes that `person` was seen, writing the Registry in the background when what the
/// directory says changed or the last sighting is older than `[registry.limits.principals]
/// refresh`. A failure is not the request's: the person is just not named.
pub(crate) fn note(gateway: &Gateway, person: &Person) {
    let Some(registry) = gateway.registry.clone() else {
        return;
    };
    let refresh = registry.limits().principals.refresh.get();
    let print = fingerprint(person);
    {
        let mut seen = gateway
            .sightings
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match seen.get(&person.id) {
            Some((last, at))
                if *last == print && refresh.is_none_or(|refresh| at.elapsed() < refresh) =>
            {
                return;
            }
            _ => {
                seen.insert(person.id.clone(), (print, Instant::now()));
            }
        }
    }
    let Ok(at) = unix_timestamp() else {
        return;
    };
    let mut record = PrincipalRecord::new(person.id.clone(), person.name.clone(), at)
        .in_groups(person.groups.clone());
    if let Some(display) = person
        .display_name
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        record = record.displayed_as(display);
    }
    tokio::task::spawn_blocking(move || {
        let _ = registry.note_principal(&record);
    });
}

/// The people among `ids` the Registry has a record of, by ID.
pub(crate) async fn named(
    gateway: &Gateway,
    ids: Vec<String>,
) -> Result<HashMap<String, PrincipalRecord>, ApiError> {
    let Some(registry) = gateway.registry.clone() else {
        return Ok(HashMap::new());
    };
    tokio::task::spawn_blocking(move || {
        let mut found = HashMap::new();
        for id in ids {
            if found.contains_key(&id) {
                continue;
            }
            if let Some(person) = registry.principal(&id)? {
                found.insert(id, person);
            }
        }
        Ok::<_, std::io::Error>(found)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())
}

/// The ID an audit actor names: the person, before any `via key`.
pub(crate) fn actor_person(actor: &str) -> &str {
    actor
        .split_once(" via key ")
        .map_or(actor, |(person, _)| person)
}

/// The label to show for person `id` in HTML-bound text: the name, or the ID when never seen.
pub(crate) async fn label_of(gateway: &Gateway, id: &str) -> String {
    match named(gateway, vec![id.to_owned()]).await {
        Ok(mut found) => found
            .remove(id)
            .map_or_else(|| id.to_owned(), |person| person.label().to_owned()),
        Err(_) => id.to_owned(),
    }
}

/// Whether `person` may learn the names of others: they hold `access.read`, or `audit.read`
/// on some Operation, so the names only widen what they already see by ID.
async fn may_name_others(gateway: &Gateway, person: &Person) -> Result<bool, ApiError> {
    if resource_action_allowed(
        gateway,
        person.clone(),
        CONFIG_ACCESS.to_owned(),
        "access.read",
    )
    .await?
    {
        return Ok(true);
    }
    let tree = operation_tree(gateway).await?;
    let operations = known_operations(gateway).await;
    let authorizer = gateway.authorizer()?;
    let person = person.clone();
    tokio::task::spawn_blocking(move || {
        for operation in &operations {
            let decision = authorizer.authorize(operation_request(
                person.clone(),
                "audit.read",
                operation,
                &tree,
            ));
            if decision.map_err(|_| ApiError::internal())?.allowed {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// A person as the routes show them.
#[derive(Serialize)]
pub(crate) struct PrincipalView {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    /// The name to show: the display name, else the account name.
    label: String,
    groups: Vec<String>,
    last_seen: f64,
}

impl From<PrincipalRecord> for PrincipalView {
    fn from(person: PrincipalRecord) -> Self {
        Self {
            label: person.label().to_owned(),
            id: person.id,
            name: person.name,
            display_name: person.display_name,
            groups: person.groups,
            last_seen: person.last_seen,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Principals {
    people: Vec<PrincipalView>,
    /// For a search: the groups, by name, whose name matches.
    groups: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Lookup {
    ids: Vec<String>,
}

/// `POST /v1/user/principals`: the people among `ids` the gateway has seen. The signed-in
/// person is always named; others need `access.read`, or `audit.read` on an Operation.
pub(crate) async fn lookup(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(body): Json<Lookup>,
) -> Result<Json<Principals>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let all = may_name_others(&gateway, &person).await?;
    let ids = body
        .ids
        .into_iter()
        .filter(|id| all || *id == person.id)
        .collect();
    let mut people: Vec<_> = named(&gateway, ids)
        .await?
        .into_values()
        .map(PrincipalView::from)
        .collect();
    people.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(Json(Principals {
        people,
        groups: Vec::new(),
    }))
}

#[derive(Deserialize)]
pub(crate) struct Search {
    #[serde(default)]
    q: String,
    limit: Option<usize>,
}

/// `GET /v1/user/principals?q=`: the people seen and the groups they are in whose name, account
/// name or ID contains every word of `q`; needs `access.read` at `config:access`.
pub(crate) async fn search(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<Search>,
) -> Result<Json<Principals>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    if !resource_action_allowed(&gateway, person, CONFIG_ACCESS.to_owned(), "access.read").await? {
        return Err(ApiError::forbidden());
    }
    let limit = audit_page(query.limit, &gateway.config.unex().timeline)
        .ok_or_else(ApiError::bad_request)?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let words: Vec<String> = query.q.split_whitespace().map(str::to_lowercase).collect();
    let matches = move |text: &str| {
        let text = text.to_lowercase();
        words.iter().all(|word| text.contains(word))
    };
    let (people, groups) = tokio::task::spawn_blocking(move || {
        let everyone = registry.principals()?;
        let groups: BTreeSet<String> = everyone
            .iter()
            .flat_map(|person| person.groups.iter().cloned())
            .filter(|group| matches(group))
            .take(limit)
            .collect();
        let people: Vec<PrincipalView> = everyone
            .into_iter()
            .filter(|person| matches(&format!("{} {} {}", person.label(), person.name, person.id)))
            .take(limit)
            .map(PrincipalView::from)
            .collect();
        Ok::<_, std::io::Error>((people, groups.into_iter().collect()))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(Principals { people, groups }))
}
