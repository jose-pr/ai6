//! An account's certificate pins and explicit map, through the gateway.
//!
//! Both are kept in the Registry (see `ai6::directorates::m::access::CertificateDirectory`). Every route needs
//! `account.manage` at `config:accounts`, and every change is audited before it is made.

use ai6::{AccountPins, CONFIG_ACCOUNTS, Editor, PinChange, PinMatch, PinStore};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use coqui_access_kit::pins::{PinSet, pin_matches};
use serde::{Deserialize, Serialize};

use crate::access_admin::{AdminError, live};
use crate::accounts::{audit, checked_name, chief_guard, listed, provider};
use crate::{ApiError, Gateway, authorize_resource_action, bearer, resolve_user};

/// What an account holds, as the routes show it.
#[derive(Serialize)]
pub(crate) struct PinsView {
    pins: Vec<String>,
    matches: Vec<PinMatch>,
    max_pins: usize,
}

impl PinsView {
    fn new(held: AccountPins, max_pins: usize) -> Self {
        Self {
            pins: held.pins,
            matches: held.matches,
            max_pins,
        }
    }
}

/// A change to an account's pins and explicit map; an absent list changes nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PinsBody {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    match_add: Vec<PinMatch>,
    #[serde(default)]
    match_remove: Vec<PinMatch>,
}

/// Whether `text` is a pin: `sha256:` or `spki-sha256:` and 64 hex digits.
fn is_pin(text: &str) -> bool {
    pin_matches(text, &PinSet::default()).is_some()
}

/// Whether `entry` may name an account: a lowercase claim name and a value with no control
/// characters.
fn is_map_entry(entry: &PinMatch) -> bool {
    !entry.field.is_empty()
        && entry.field.len() <= 64
        && entry.field.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        && !entry.value.is_empty()
        && entry.value.len() <= 512
        && !entry.value.chars().any(char::is_control)
}

fn registry(gateway: &Gateway) -> Result<std::sync::Arc<dyn ai6::RegistryBackend>, AdminError> {
    gateway
        .registry
        .clone()
        .ok_or_else(|| AdminError::Api(ApiError::unavailable()))
}

/// `GET /v1/user/accounts/{name}/pins`: the pins and explicit map entries the account holds.
pub(crate) async fn pins_of(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<PinsView>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person,
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    checked_name(&gateway, &name)?;
    let registry = registry(&gateway)?;
    let held = tokio::task::spawn_blocking(move || registry.account_pins(&name))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::unavailable()))?;
    Ok(Json(PinsView::new(held, gateway.config.login.max_pins)))
}

/// `POST /v1/user/accounts/{name}/pins`: takes away what `remove` and `match_remove` name, then
/// adds what `add` and `match_add` name. An account holds at most `[login] max_pins` pins and
/// as many entries; a chief's account is changed only by a chief, as its groups are.
pub(crate) async fn change_pins(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<PinsBody>,
) -> Result<Json<PinsView>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    checked_name(&gateway, &name)?;
    let pins = body.add.iter().chain(&body.remove);
    let entries = body.match_add.iter().chain(&body.match_remove);
    if pins.clone().count() + entries.clone().count() == 0
        || !pins.into_iter().all(|pin| is_pin(pin))
        || !entries.into_iter().all(is_map_entry)
    {
        return Err(AdminError::Refused(
            "name a pin as sha256:<64 hex> or spki-sha256:<64 hex>, and a map entry as a lowercase field and a value"
                .to_owned(),
        ));
    }
    let accounts = provider(&gateway)?;
    let live = live(&gateway)?;
    let effective = live.effective();
    let editor = Editor::new(person.clone(), &effective);
    let Some(existing) = listed(accounts)
        .await?
        .into_iter()
        .find(|info| info.name == name)
    else {
        return Err(AdminError::Api(ApiError::not_found()));
    };
    if let Err(error) = chief_guard(
        &effective,
        &editor,
        Some(&existing),
        &name,
        &existing.groups,
    ) {
        audit(
            &gateway,
            &person,
            "account.refused",
            &format!("{name}: chief"),
        )
        .await?;
        return Err(error);
    }
    for (action, targets) in [
        ("account.pin.remove", &body.remove),
        ("account.pin.add", &body.add),
    ] {
        for pin in targets {
            audit(&gateway, &person, action, &format!("{name}: {pin}")).await?;
        }
    }
    for (action, targets) in [
        ("account.pin.map.remove", &body.match_remove),
        ("account.pin.map.add", &body.match_add),
    ] {
        for target in targets {
            let text = format!("{name}: {}={}", target.field, target.value);
            audit(&gateway, &person, action, &text).await?;
        }
    }
    let registry = registry(&gateway)?;
    let max = gateway.config.login.max_pins;
    let change = PinChange {
        add: body.add,
        remove: body.remove,
        match_add: body.match_add,
        match_remove: body.match_remove,
    };
    let account = name.clone();
    let result = tokio::task::spawn_blocking(move || registry.change_pins(&account, &change, max))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?;
    match result {
        Ok(held) => Ok(Json(PinsView::new(held, max))),
        Err(error) if error.kind() == std::io::ErrorKind::QuotaExceeded => {
            audit(
                &gateway,
                &person,
                "account.refused",
                &format!("{name}: more than max_pins"),
            )
            .await?;
            Err(AdminError::Refused(format!(
                "an account holds at most {max} pins and {max} map entries ([login] max_pins)"
            )))
        }
        Err(_) => Err(AdminError::Api(ApiError::unavailable())),
    }
}
