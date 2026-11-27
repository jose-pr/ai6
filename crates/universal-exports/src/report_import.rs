//! `POST /v1/reports/import`: a Report document read back in.
//!
//! The body is the Markdown `GET /v1/reports/{id}/document` writes: front matter for the fields,
//! the body for the text. Without an `id` it files a new Report; with one it changes the stored
//! Report, from the `revision` the document was read at. The gateway owns `id`, `revision`,
//! `created`, `updated`, `filer` and what follows from them: a document that repeats them must
//! repeat what is stored, and a new Report may not carry them. Everything is checked as the
//! person importing it, by the same rules as filing, revising or marking the Report by hand.
//! `?dry_run=true` reads and checks the document against the stored Report and stores nothing.

use ai6::directorates::m::report_document::{self, ReportDocument};
use serde_json::Map;

use super::*;

/// What the gateway keeps: a document gives it back unchanged or leaves it out.
const OWNED: [&str; 15] = [
    "id",
    "revision",
    "created",
    "updated",
    "updated_by",
    "filer",
    "source",
    "reliability",
    "sight",
    "untrusted",
    "exposed",
    "proposes",
    "state",
    "assessment",
    "resolution",
];

/// What a document may set. `credibility` is the assessment's, not a document's.
const SETTABLE: [&str; 15] = [
    "scope",
    "kind",
    "title",
    "summary",
    "topics",
    "severity",
    "priority",
    "deadline",
    "from",
    "sub_source",
    "classification",
    "handling",
    "compartments",
    "tearline",
    "links",
];

/// Of those, what a known Report's next revision may change.
const REVISED: [&str; 4] = ["title", "summary", "topics", "compartments"];

/// `?dry_run=true`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Importing {
    #[serde(default)]
    dry_run: bool,
}

/// `POST /v1/reports/import` with the document as the body: `201` filed, `200` changed or
/// unchanged, `202` a proposal waiting for a verdict, each with the Report as the person sees it
/// and `change` saying which. `400` for a document that does not read or sets what is not its
/// to set, `409` for a revision that has moved on (with the one it is at), `404` for a Report
/// the person may not read.
pub(crate) async fn import(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(importing): Query<Importing>,
    body: String,
) -> Result<(StatusCode, Json<Value>), Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    let document =
        report_document::read(&body).map_err(|reason| refused(StatusCode::BAD_REQUEST, reason))?;
    let at = unix_timestamp()?;
    blocking(move || {
        let (status, answer) = if document.fields.contains_key("id") {
            update(registry.as_ref(), &reader, document, importing.dry_run)?
        } else {
            create(registry.as_ref(), &reader, document, importing.dry_run, at)?
        };
        Ok((status, Json(answer)))
    })
    .await
}

fn bad(reason: impl Into<String>) -> Refused {
    refused(StatusCode::BAD_REQUEST, reason)
}

fn labelled(mut answer: Value, change: &str) -> Value {
    if let Value::Object(map) = &mut answer {
        map.insert("change".to_owned(), Value::from(change));
    }
    answer
}

/// A number is its value, whatever way the document wrote it.
fn normal(value: &Value) -> Value {
    match value {
        Value::Number(number) => number.as_f64().map_or_else(|| value.clone(), Value::from),
        Value::Array(items) => Value::Array(items.iter().map(normal).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), normal(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::Object(map)) => map.is_empty(),
        Some(_) => false,
    }
}

/// Whether two values say the same: nothing and an empty value are one thing, as an export
/// leaves out what is empty.
fn same(left: Option<&Value>, right: Option<&Value>) -> bool {
    (blank(left) && blank(right))
        || left
            .zip(right)
            .is_some_and(|(left, right)| normal(left) == normal(right))
}

fn unknown_keys(fields: &Map<String, Value>) -> Result<(), Refused> {
    let unknown: Vec<&str> = fields
        .keys()
        .map(String::as_str)
        .filter(|key| !OWNED.contains(key) && !SETTABLE.contains(key))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(bad(format!(
            "{} {} not a field of a Report.",
            unknown.join(", "),
            if unknown.len() == 1 { "is" } else { "are" }
        )))
    }
}

fn words(fields: &Map<String, Value>, key: &str) -> Result<Vec<String>, Refused> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| bad(format!("{key} is a list of words."))),
        Some(_) => Err(bad(format!("{key} is a list of words."))),
    }
}

fn line(fields: &Map<String, Value>, key: &str) -> Result<Option<String>, Refused> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{key} is text."))),
    }
}

/// A new Report: filed as the person, who needs what filing it by hand needs.
fn create(
    registry: &dyn RegistryBackend,
    reader: &PersonReader,
    document: ReportDocument,
    dry_run: bool,
    at: f64,
) -> Result<(StatusCode, Value), Refused> {
    let ReportDocument { mut fields, text } = document;
    unknown_keys(&fields)?;
    let owned: Vec<&str> = OWNED
        .into_iter()
        .filter(|key| fields.contains_key(*key))
        .collect();
    if !owned.is_empty() {
        return Err(bad(format!(
            "{} belong to the gateway, which sets them when a Report is filed: leave them out of a new one.",
            owned.join(", ")
        )));
    }
    let scope = match fields.remove("scope") {
        Some(Value::String(scope)) => scope_of(&scope)?,
        _ => {
            return Err(bad(
                "A report names its scope: operation:ID, station:ID, person:ID, global or m.",
            ));
        }
    };
    let mut filing = Map::new();
    for (key, value) in fields {
        let key = if key == "from" {
            "information_from".to_owned()
        } else {
            key
        };
        filing.insert(key, value);
    }
    filing.insert("text".to_owned(), Value::from(text));
    let filing: ReportFiling =
        serde_json::from_value(Value::Object(filing)).map_err(|error| bad(error.to_string()))?;
    if let Some(kind) = filing.kind.clone().filter(|kind| is_known_kind(kind)) {
        let found = registry
            .reports(&ReportFilter::all().in_scope(scope.clone()).of_kind(kind))
            .map_err(|_| Refused::from(ApiError::internal()))?
            .into_iter()
            .find(|each| {
                each.title == filing.title
                    && each.state == ReportState::Standing
                    && reader.reads_report(each)
                    && sight(reader, each, &reader.rules()) == ReportSight::Content
            });
        if let Some(found) = found {
            return Err(Refused {
                status: StatusCode::CONFLICT,
                body: json!({
                    "error": format!(
                        "{} is already filed in {scope} as {}: name its id and revision to change it.",
                        filing.title, found.id
                    ),
                    "id": found.id,
                    "revision": found.revision,
                }),
            });
        }
    }
    if dry_run {
        let known = filing.kind.as_deref().is_some_and(is_known_kind);
        if known && reader.writes_scope(&scope) == KnowledgeRight::Refused {
            return Err(refused(
                StatusCode::FORBIDDEN,
                format!("You may not change what is known in {scope}."),
            ));
        }
        if !known && !reader.files_in(&scope) {
            return Err(refused(
                StatusCode::FORBIDDEN,
                format!("You may not file reports in {scope}."),
            ));
        }
        if !known {
            let filer = ReportFiler::Person {
                person: reader.person.id.clone(),
            };
            filing
                .into_report(
                    scope,
                    filer,
                    [],
                    at,
                    &registry.limits().reports,
                    &reader.settings,
                )
                .map_err(bad)?;
        }
        return Ok((StatusCode::OK, json!({"change": "created"})));
    }
    let (status, Json(answer)) = file_report(registry, reader, scope, filing, at)?;
    let change = if status == StatusCode::ACCEPTED {
        "proposed"
    } else {
        "created"
    };
    Ok((status, labelled(answer, change)))
}

/// A stored Report: the document must agree with everything the gateway owns, and what it
/// changes must be what that kind of Report lets a person change.
fn update(
    registry: &dyn RegistryBackend,
    reader: &PersonReader,
    document: ReportDocument,
    dry_run: bool,
) -> Result<(StatusCode, Value), Refused> {
    let ReportDocument { fields, text } = document;
    unknown_keys(&fields)?;
    let id = fields
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("id is the text of a Report's id."))?
        .to_owned();
    if ai6::directorates::m::guides::find(&id).is_some() {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "A built-in guide is read only: file a Report with its title in this deployment to override it.",
        ));
    }
    let missing = || refused(StatusCode::NOT_FOUND, "No such report.");
    let report = registry
        .report(&id)
        .ok()
        .flatten()
        .filter(|report| reader.reads_report(report))
        .ok_or_else(missing)?;
    if report.is_known() && sight(reader, &report, &reader.rules()) != ReportSight::Content {
        return Err(missing());
    }
    let current = shown(registry, reader, &report);
    if current["sight"] != "content" {
        return Err(Refused {
            status: StatusCode::FORBIDDEN,
            body: json!({
                "error": "You are not read in to this report, or not cleared for it.",
                "report": current,
            }),
        });
    }
    let revision = fields
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("Name the revision you read, as revision."))?;
    if revision != report.revision {
        return Err(Refused {
            status: StatusCode::CONFLICT,
            body: json!({
                "error": "The report has changed since this document was read.",
                "revision": report.revision,
            }),
        });
    }
    for key in OWNED {
        if key == "revision" {
            continue;
        }
        if let Some(value) = fields.get(key)
            && !same(Some(value), current.get(key))
        {
            return Err(bad(format!(
                "{key} is the gateway's, and the stored Report's is not that."
            )));
        }
    }
    // What a document leaves out is empty, except `scope` and `kind`: those stay as filed.
    let mut changed: Vec<&str> = SETTABLE
        .into_iter()
        .filter(|key| {
            (!matches!(*key, "scope" | "kind") || fields.contains_key(*key))
                && !same(fields.get(*key), current.get(*key))
        })
        .collect();
    if !same(Some(&Value::from(text.as_str())), current.get("text")) {
        changed.push("text");
    }
    if changed.is_empty() {
        return Ok((StatusCode::OK, labelled(current, "unchanged")));
    }
    let revisable = report.is_known()
        && changed
            .iter()
            .all(|key| REVISED.contains(key) || *key == "text");
    if !revisable {
        let what = changed.join(", ");
        return Err(bad(if report.is_known() {
            format!(
                "Importing changes a {}'s title, summary, topics, compartments and text; not {what}.",
                report.kind
            )
        } else {
            format!(
                "A filed Report is not rewritten by importing it ({what}): its markings change at PUT /v1/reports/ID/markings and its verdict at PUT /v1/reports/ID/assessment."
            )
        }));
    }
    let title = line(&fields, "title")?.ok_or_else(|| bad("A report has a title."))?;
    if dry_run {
        return Ok((
            StatusCode::OK,
            json!({"change": "changed", "id": id, "fields": changed}),
        ));
    }
    let mut change = KnowledgeChange::new(KnowledgeTarget::Existing { id, revision }, title, text)
        .on_topics(words(&fields, "topics")?)
        .with_compartments(Some(words(&fields, "compartments")?));
    change.summary = line(&fields, "summary")?;
    let (status, Json(answer)) = written(
        knowledge::write_report(registry, reader, &reader.rules(), change)?,
        false,
    )?;
    let outcome = if status == StatusCode::ACCEPTED {
        "proposed"
    } else {
        "changed"
    };
    Ok((status, labelled(answer, outcome)))
}
