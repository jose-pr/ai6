//! Outer Office routes: a closed set of app pages, each with typed optional fields.
//!
//! A route is the page a person sees and its filters, and nothing else: no action, dialog,
//! form value, token or outside URL. It has two forms that carry the same fields. The JSON
//! form is an object tagged by `page` (`{"page": "missions", "operation_id": "G", "state":
//! ["failed"]}`); the link form is `app:/<page>?k=v&...`, its query in the page's field
//! order, `state` comma-joined and every value percent-encoded. Both forms refuse unknown
//! pages and fields. The app's `OoRoute` parses the same forms and gives the same reasons;
//! `clients/outer-office/test/fixtures/assistant_routes.json` holds the cases both test.

use serde_json::Value;

use super::checks::identifier;

/// The longest `text` filter by default, in bytes.
pub(crate) const DEFAULT_TEXT_BYTES: usize = 128;

/// The bounds a route is checked against; [`super::AssistantConfig::with_route_text_bytes`]
/// sets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The longest `text` filter, in bytes.
    pub(crate) text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            text_bytes: DEFAULT_TEXT_BYTES,
        }
    }
}

/// The states of a Mission or Session on the board, in the board's order.
pub(crate) const STATES: [&str; 9] = [
    "queued",
    "working",
    "blocked",
    "stalled",
    "parked",
    "quarantined",
    "done",
    "failed",
    "abandoned",
];

const MISSION_SORTS: &[&str] = &["updated", "created", "state", "operation", "mission"];
const SESSION_SORTS: &[&str] = &[
    "updated",
    "created",
    "state",
    "operation",
    "mission",
    "session",
];
const ORDERS: &[&str] = &["asc", "desc"];
const STATION_TABS: &[&str] = &["overview", "theaters", "settings", "placement"];
const AGENT_TABS: &[&str] = &["agents", "roles"];
const GREEN_INK_TABS: &[&str] = &["inbox", "classes"];
const BRIEFING_TABS: &[&str] = &["proposals", "held"];
const ROSTER_KINDS: &[&str] = &["operation", "agent", "role", "cover", "assistant"];

/// What a field holds.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// An identifier, by the views' rule.
    Identifier,
    /// Free text of 1 to [`Limits::text_bytes`] bytes, without control or invisible characters.
    Text,
    /// 1 to 9 distinct board states: a JSON array, comma-joined in a link.
    States,
    /// One of these words.
    OneOf(&'static [&'static str]),
}

/// A page of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Overview,
    Operations,
    Missions,
    Sessions,
    Stations,
    Agents,
    GreenInk,
    Briefings,
    Roster,
    Pools,
    Timeline,
    Audit,
    Memory,
}

impl Page {
    const ALL: [Self; 13] = [
        Self::Overview,
        Self::Operations,
        Self::Missions,
        Self::Sessions,
        Self::Stations,
        Self::Agents,
        Self::GreenInk,
        Self::Briefings,
        Self::Roster,
        Self::Pools,
        Self::Timeline,
        Self::Audit,
        Self::Memory,
    ];

    /// The page's name in both forms.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Operations => "operations",
            Self::Missions => "missions",
            Self::Sessions => "sessions",
            Self::Stations => "stations",
            Self::Agents => "agents",
            Self::GreenInk => "green_ink",
            Self::Briefings => "briefings",
            Self::Roster => "roster",
            Self::Pools => "pools",
            Self::Timeline => "timeline",
            Self::Audit => "audit",
            Self::Memory => "memory",
        }
    }

    fn named(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|page| page.name() == name)
            .ok_or_else(|| format!("unknown page {name}"))
    }

    /// The page's fields, in link order.
    fn fields(self) -> &'static [(&'static str, Kind)] {
        use Kind::{Identifier, OneOf, States, Text};
        match self {
            Self::Overview | Self::Memory => &[],
            Self::Operations => &[("operation_id", Identifier)],
            Self::Missions => &[
                ("operation_id", Identifier),
                ("state", States),
                ("station_id", Identifier),
                ("agent", Identifier),
                ("text", Text),
                ("sort", OneOf(MISSION_SORTS)),
                ("order", OneOf(ORDERS)),
                ("mission_id", Identifier),
            ],
            Self::Sessions => &[
                ("operation_id", Identifier),
                ("state", OneOf(&STATES)),
                ("station_id", Identifier),
                ("agent", Identifier),
                ("sort", OneOf(SESSION_SORTS)),
                ("order", OneOf(ORDERS)),
                ("session_id", Identifier),
            ],
            Self::Stations => &[
                ("station_id", Identifier),
                ("tab", OneOf(STATION_TABS)),
                ("theater_id", Identifier),
            ],
            Self::Agents => &[
                ("tab", OneOf(AGENT_TABS)),
                ("operation_id", Identifier),
                ("agent", Identifier),
                ("role", Identifier),
            ],
            Self::GreenInk => &[
                ("tab", OneOf(GREEN_INK_TABS)),
                ("operation_id", Identifier),
                ("request_id", Identifier),
            ],
            Self::Briefings => &[
                ("tab", OneOf(BRIEFING_TABS)),
                ("operation_id", Identifier),
                ("item_id", Identifier),
            ],
            Self::Roster => &[("kind", OneOf(ROSTER_KINDS)), ("key", Identifier)],
            Self::Pools => &[("operation_id", Identifier), ("pool", Identifier)],
            Self::Timeline => &[("operation_id", Identifier), ("mission_id", Identifier)],
            Self::Audit => &[("operation_id", Identifier), ("station_id", Identifier)],
        }
    }

    fn field(self, key: &str) -> Result<(&'static str, Kind), String> {
        self.fields()
            .iter()
            .find(|(name, _)| *name == key)
            .copied()
            .ok_or_else(|| format!("{} has no {key}", self.name()))
    }
}

/// A field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Field {
    Text(String),
    States(Vec<String>),
}

/// A checked route: a page and its fields, in the page's field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    page: Page,
    fields: Vec<(&'static str, Field)>,
}

impl Route {
    /// Parses and checks the JSON form.
    pub(crate) fn from_json(json: &Value, limits: Limits) -> Result<Self, String> {
        const NOT_A_ROUTE: &str = "a route is an object naming its page";
        let object = json.as_object().ok_or(NOT_A_ROUTE)?;
        let page = Page::named(
            object
                .get("page")
                .and_then(Value::as_str)
                .ok_or(NOT_A_ROUTE)?,
        )?;
        // Sorted, so that of several faults both parsers name the same one first.
        let mut keys: Vec<&String> = object.keys().filter(|key| *key != "page").collect();
        keys.sort();
        let mut fields = Vec::new();
        for key in keys {
            let value = &object[key];
            let (name, kind) = page.field(key)?;
            let field = match (kind, value) {
                (Kind::States, Value::Array(items)) => Field::States(
                    items
                        .iter()
                        .map(|item| item.as_str().map(str::to_owned))
                        .collect::<Option<_>>()
                        .ok_or("state is a list of board states")?,
                ),
                (Kind::States, _) => return Err("state is a list of board states".to_owned()),
                (_, Value::String(text)) => Field::Text(text.clone()),
                (_, _) => return Err(format!("{name} is a string")),
            };
            fields.push((name, field));
        }
        Self::checked(page, fields, limits)
    }

    /// The JSON form.
    #[cfg(test)]
    pub(crate) fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("page".to_owned(), Value::from(self.page.name()));
        for (name, field) in &self.fields {
            let value = match field {
                Field::Text(text) => Value::from(text.as_str()),
                Field::States(states) => Value::from(states.clone()),
            };
            object.insert((*name).to_owned(), value);
        }
        Value::Object(object)
    }

    /// Parses and checks the link form, `app:/<page>?k=v&...`; fields may come in any order.
    pub(crate) fn parse_uri(uri: &str, limits: Limits) -> Result<Self, String> {
        let rest = uri
            .strip_prefix("app:/")
            .ok_or("a route link starts with app:/")?;
        let (path, query) = match rest.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (rest, None),
        };
        let page = Page::named(path)?;
        let mut fields = Vec::new();
        for pair in query.map(|query| query.split('&')).into_iter().flatten() {
            let (key, value) = pair
                .split_once('=')
                .filter(|(key, _)| !key.is_empty())
                .ok_or("a query parameter is key=value")?;
            let (name, kind) = page.field(key)?;
            let field = match kind {
                Kind::States => Field::States(
                    value
                        .split(',')
                        .map(|state| decode(name, state))
                        .collect::<Result<_, _>>()?,
                ),
                _ => Field::Text(decode(name, value)?),
            };
            fields.push((name, field));
        }
        Self::checked(page, fields, limits)
    }

    /// The text value of field `name`, when the route sets it.
    fn text(&self, name: &str) -> Option<&str> {
        self.fields.iter().find_map(|(field, value)| match value {
            Field::Text(text) if *field == name => Some(text.as_str()),
            _ => None,
        })
    }

    /// Whether this Missions route's page shows `mission`, an item of `GET /v1/missions`; the
    /// app's `routeMissions` decides the same, and `assistant_routes.json` holds the cases both
    /// test. A state filter keeps the Sessions in those states (a Session without one is
    /// working) and the Mission shows when one is kept; Station, Agent and text then look at
    /// the kept Sessions, the text also at the Operation, the Mission and the brief's prompt,
    /// without regard to case. Any other page shows none.
    pub(crate) fn shows_mission(&self, mission: &Value) -> bool {
        if self.page != Page::Missions {
            return false;
        }
        let states = self.fields.iter().find_map(|(_, value)| match value {
            Field::States(states) => Some(states),
            Field::Text(_) => None,
        });
        let sessions: Vec<&Value> = mission["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|session| {
                states.is_none_or(|states| {
                    let state = session["state"].as_str().unwrap_or("working");
                    states.iter().any(|wanted| wanted == state)
                })
            })
            .collect();
        if sessions.is_empty() {
            return false;
        }
        let equals = |key: &str, value: &Value| self.text(key).is_none_or(|wanted| value == wanted);
        let any_session = |key: &str, field: &str| {
            self.text(key)
                .is_none_or(|wanted| sessions.iter().any(|session| session[field] == wanted))
        };
        let text = self.text("text").is_none_or(|wanted| {
            let wanted = wanted.to_lowercase();
            [
                &mission["operation_id"],
                &mission["mission_id"],
                &mission["brief"]["prompt"],
            ]
            .into_iter()
            .chain(sessions.iter().map(|session| &session["session_id"]))
            .filter_map(Value::as_str)
            .any(|item| item.to_lowercase().contains(&wanted))
        });
        equals("operation_id", &mission["operation_id"])
            && equals("mission_id", &mission["mission_id"])
            && any_session("station_id", "station_id")
            && any_session("agent", "agent")
            && text
    }

    /// The link form, its query in the page's field order.
    pub(crate) fn to_uri(&self) -> String {
        let query = self
            .fields
            .iter()
            .map(|(name, field)| {
                let value = match field {
                    Field::Text(text) => encode(text),
                    Field::States(states) => states
                        .iter()
                        .map(|state| encode(state))
                        .collect::<Vec<_>>()
                        .join(","),
                };
                format!("{name}={value}")
            })
            .collect::<Vec<_>>()
            .join("&");
        if query.is_empty() {
            format!("app:/{}", self.page.name())
        } else {
            format!("app:/{}?{query}", self.page.name())
        }
    }

    /// Checks each field against its kind and the page's rules, and puts them in field order.
    fn checked(
        page: Page,
        mut fields: Vec<(&'static str, Field)>,
        limits: Limits,
    ) -> Result<Self, String> {
        for (index, (name, _)) in fields.iter().enumerate() {
            if fields[..index].iter().any(|(earlier, _)| earlier == name) {
                return Err(format!("{name} appears twice"));
            }
        }
        fields.sort_by_key(|(name, _)| {
            page.fields()
                .iter()
                .position(|(field, _)| field == name)
                .unwrap_or(usize::MAX)
        });
        for (name, field) in &fields {
            let (_, kind) = page.field(name)?;
            match (kind, field) {
                (Kind::Identifier, Field::Text(text)) => identifier(name, text)?,
                (Kind::Text, Field::Text(text)) => {
                    if text.is_empty()
                        || text.len() > limits.text_bytes
                        || text.chars().any(|c| {
                            c.is_control() || ai6::directorates::m::vetting::is_invisible(c)
                        })
                    {
                        return Err(format!(
                            "text is 1 to {} bytes without control or invisible \
                             characters",
                            limits.text_bytes
                        ));
                    }
                }
                (Kind::OneOf(words), Field::Text(text)) => {
                    if !words.contains(&text.as_str()) {
                        return Err(format!("{name} is one of {}", words.join(", ")));
                    }
                }
                (Kind::States, Field::States(states)) => {
                    for state in states {
                        if !STATES.contains(&state.as_str()) {
                            return Err(format!("{state} is not a board state"));
                        }
                    }
                    let distinct = states
                        .iter()
                        .enumerate()
                        .all(|(index, state)| !states[..index].contains(state));
                    if states.is_empty() || states.len() > STATES.len() || !distinct {
                        return Err(format!(
                            "state lists 1 to {} distinct board states",
                            STATES.len()
                        ));
                    }
                }
                (Kind::States, Field::Text(_)) => {
                    return Err("state is a list of board states".to_owned());
                }
                (_, Field::States(_)) => return Err(format!("{name} is a string")),
            }
        }
        let has = |key: &str| fields.iter().any(|(name, _)| *name == key);
        match page {
            Page::Missions if has("mission_id") && !has("operation_id") => {
                return Err("mission_id needs operation_id".to_owned());
            }
            Page::Stations if has("theater_id") && !has("station_id") => {
                return Err("theater_id needs station_id".to_owned());
            }
            Page::Audit if has("operation_id") && has("station_id") => {
                return Err("audit takes operation_id or station_id, not both".to_owned());
            }
            _ => {}
        }
        Ok(Self { page, fields })
    }
}

/// The link form and every page with its fields, for Moneypenny.
pub(crate) fn describe() -> String {
    let pages = Page::ALL
        .into_iter()
        .map(|page| {
            let fields = page
                .fields()
                .iter()
                .map(|(name, kind)| match kind {
                    Kind::Identifier | Kind::Text => (*name).to_owned(),
                    Kind::States => format!("{name}: board states, comma-separated"),
                    Kind::OneOf(words) if words.iter().eq(STATES.iter()) => {
                        format!("{name}: one board state")
                    }
                    Kind::OneOf(words) => format!("{name}: {}", words.join("|")),
                })
                .collect::<Vec<_>>();
            if fields.is_empty() {
                page.name().to_owned()
            } else {
                format!("{} ({})", page.name(), fields.join(", "))
            }
        })
        .collect::<Vec<_>>();
    format!(
        "An app page link is app:/<page>?key=value&..., each value percent-encoded, every key \
         optional: {}. mission_id needs operation_id, theater_id needs station_id, and audit \
         takes operation_id or station_id. Board states: {}. Example: \
         [failed Missions of G](app:/missions?operation_id=G&state=failed).",
        pages.join("; "),
        STATES.join(", ")
    )
}

/// Percent-encodes every byte but the unreserved characters of RFC 3986, section 2.3.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Decodes `%XX` sequences; everything else is taken as written. `+` is not a space.
fn decode(name: &str, text: &str) -> Result<String, String> {
    let refused = || format!("{name} is not percent-encoded UTF-8");
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes
                .get(at + 1..at + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .ok_or_else(refused)?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| refused())?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| refused())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Valid routes in both forms and invalid ones with their reason; the app's route tests
    /// read the same file.
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../../clients/outer-office/test/fixtures/assistant_routes.json"
        ))
        .unwrap()
    }

    #[test]
    fn missions_routes_show_the_fixture_missions_the_app_shows() {
        let fixture = fixture();
        let missions = fixture["missions"]["board"].as_array().unwrap();
        for case in fixture["missions"]["cases"].as_array().unwrap() {
            let route = Route::from_json(&case["route"], Limits::default()).unwrap();
            let shown: Vec<&str> = missions
                .iter()
                .filter(|mission| route.shows_mission(mission))
                .map(|mission| mission["mission_id"].as_str().unwrap())
                .collect();
            let expected: Vec<&str> = case["shows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap())
                .collect();
            assert_eq!(shown, expected, "{}", case["route"]);
        }
        let other = Route::from_json(&json!({"page": "sessions"}), Limits::default()).unwrap();
        assert!(
            !other.shows_mission(&missions[0]),
            "only a Missions route shows Missions"
        );
    }

    #[test]
    fn routes_of_the_fixture_round_trip_between_both_forms() {
        let fixture = fixture();
        let valid = fixture["valid"].as_array().unwrap();
        assert!(valid.len() >= 13, "every page has a case");
        for case in valid {
            let from_json = Route::from_json(&case["json"], Limits::default())
                .unwrap_or_else(|reason| panic!("{}: {reason}", case["json"]));
            let uri = case["uri"].as_str().unwrap();
            assert_eq!(from_json.to_uri(), uri);
            let from_uri = Route::parse_uri(uri, Limits::default())
                .unwrap_or_else(|reason| panic!("{uri}: {reason}"));
            assert_eq!(from_uri, from_json, "{uri}");
            assert_eq!(from_uri.to_json(), case["json"], "{uri}");
        }
    }

    #[test]
    fn routes_of_the_fixture_that_are_invalid_are_refused_with_their_reason() {
        let fixture = fixture();
        let invalid = fixture["invalid"].as_array().unwrap();
        assert!(!invalid.is_empty());
        for case in invalid {
            let refused = match (case.get("json"), case.get("uri")) {
                (Some(json), None) => Route::from_json(json, Limits::default()).unwrap_err(),
                (None, Some(uri)) => {
                    Route::parse_uri(uri.as_str().unwrap(), Limits::default()).unwrap_err()
                }
                _ => panic!("a case has json or uri: {case}"),
            };
            let reason = case["reason"].as_str().unwrap();
            assert_eq!(refused, reason, "{case}");
        }
    }

    #[test]
    fn routes_accept_fields_in_any_order_and_write_them_in_the_page_order() {
        let route = Route::parse_uri(
            "app:/missions?state=failed,done&operation_id=G",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            route.to_uri(),
            "app:/missions?operation_id=G&state=failed,done"
        );
    }

    #[test]
    fn routes_percent_encode_what_is_not_unreserved() {
        let route = Route::from_json(
            &json!({"page": "missions", "text": "a&b=c, d)é", "operation_id": "G/1"}),
            Limits::default(),
        )
        .unwrap();
        let uri = route.to_uri();
        assert_eq!(
            uri,
            "app:/missions?operation_id=G%2F1&text=a%26b%3Dc%2C%20d%29%C3%A9"
        );
        assert_eq!(Route::parse_uri(&uri, Limits::default()).unwrap(), route);
        assert_eq!(
            Route::parse_uri("app:/missions?text=a+b", Limits::default())
                .unwrap()
                .to_json()["text"],
            "a+b",
            "a plus sign is itself"
        );
    }

    #[test]
    fn the_text_limit_is_a_setting() {
        let text = "a".repeat(200);
        let route = serde_json::json!({"page": "missions", "text": text});
        assert_eq!(
            Route::from_json(&route, Limits::default()).unwrap_err(),
            "text is 1 to 128 bytes without control or invisible characters"
        );
        assert!(Route::from_json(&route, Limits { text_bytes: 200 }).is_ok());
        assert_eq!(
            Route::parse_uri("app:/missions?text=abcde", Limits { text_bytes: 4 }).unwrap_err(),
            "text is 1 to 4 bytes without control or invisible characters"
        );
    }
}
