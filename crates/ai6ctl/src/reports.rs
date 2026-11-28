//! `ai6ctl reports`: file, list and show Reports through the gateway's `/v1/reports` routes. A
//! Report you are not read in to, or not cleared for, lists by its ID and markings alone.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use reqwest::{Client, Method, StatusCode, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text};
use crate::plain;

#[derive(Debug, Subcommand)]
pub(crate) enum ReportsCommand {
    /// File a Report (needs mission.create on the Operation, or station.configure on the
    /// Station or on config:reports for m). It creates no Mission.
    File(Box<FileArgs>),
    /// List Reports of a scope, or those tied to a Mission, most urgent first; or, with --words,
    /// --topics or --title, search every scope you may read for what is known and observed,
    /// best first (id, kind, title and summary).
    List {
        /// operation:ID, station:ID, person:ID, global or m.
        #[arg(
            long,
            required_unless_present_any = ["mission", "words", "topics", "title", "kind"]
        )]
        scope: Option<String>,
        /// Only those tied to this Mission.
        #[arg(long)]
        mission: Option<String>,
        /// open, assessed, closed or standing.
        #[arg(long)]
        state: Option<String>,
        /// Search: words of the title, summary, topics and text.
        #[arg(long)]
        words: Option<String>,
        /// Search: only Reports on each of these topics, comma separated.
        #[arg(long)]
        topics: Option<String>,
        /// Search: only Reports whose title holds each of these words.
        #[arg(long)]
        title: Option<String>,
        /// Search: only this kind (memory, tradecraft, rule, contact, finding, ...).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Write a memory, tradecraft, rule or contact as its next revision, from the revision you
    /// read (the write is refused once it has moved on).
    Revise {
        /// Its ID.
        id: String,
        /// The revision you read.
        #[arg(long)]
        revision: u64,
        /// One line naming it.
        #[arg(long)]
        title: String,
        /// Its whole new text.
        #[arg(long)]
        text: String,
        /// One line saying what it is for.
        #[arg(long)]
        summary: Option<String>,
        /// Topics, comma separated.
        #[arg(long, value_delimiter = ',')]
        topics: Vec<String>,
    },
    /// Forget a memory with its revisions, or retire tradecraft, a rule or a contact (closed,
    /// resolution retired, kept). Where you may only propose it, it waits for a verdict.
    Forget {
        /// Its ID.
        id: String,
        /// The revision you read, when it must still be that one.
        #[arg(long)]
        revision: Option<u64>,
    },
    /// What a memory, tradecraft, rule or contact held before its later revisions.
    Versions {
        /// Its ID.
        id: String,
    },
    /// Show one Report as far as you may read it.
    Show {
        /// Its ID.
        id: String,
        /// `md` prints its Markdown document: every field but the text as front matter.
        #[arg(short = 'o', long, value_parser = ["text", "md"], default_value = "text")]
        output: String,
    },
    /// Write every Report a search or scope lists as a Markdown document, one file each under
    /// `DIR/KIND/`. What you may not read is not written; Reports listed by their markings alone
    /// are counted, since the listing already told you they exist.
    Export {
        /// The directory the files go in (made when missing).
        dir: PathBuf,
        /// operation:ID, station:ID, person:ID, global or m.
        #[arg(
            long,
            required_unless_present_any = ["mission", "words", "topics", "title", "kind"]
        )]
        scope: Option<String>,
        /// Only those tied to this Mission.
        #[arg(long)]
        mission: Option<String>,
        /// open, assessed, closed or standing.
        #[arg(long)]
        state: Option<String>,
        /// Words of the title, summary, topics and text.
        #[arg(long)]
        words: Option<String>,
        /// Only Reports on each of these topics, comma separated.
        #[arg(long)]
        topics: Option<String>,
        /// Only Reports whose title holds each of these words.
        #[arg(long)]
        title: Option<String>,
        /// Only this kind.
        #[arg(long)]
        kind: Option<String>,
    },
    /// File or change Reports from Markdown documents (`ai6ctl apply -f` reads them the same
    /// way): without an `id` in the front matter a new Report, with one the stored Report at
    /// the `revision` read. The front matter holds the fields, the body the text.
    Import {
        /// Documents, or directories of `.md` files.
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Check them against the stored Reports and say what would change; store nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Assess an open Report (needs mission.control on its Operation and reading it): `mission`
    /// (a Source item is queued as a brief; any other names its Mission with --mission),
    /// `duplicate` or `related` (name what with --of), `no_action`, `awaiting` (--question and
    /// --wait-seconds, its deadline), `later` (--wait-seconds, --condition or both) or `decision`
    /// (--question, filed as green ink of kind decision). Nothing becomes a Mission without this
    /// or the Operation's charge assessing it.
    Assess(Box<AssessArgs>),
    /// Open an assessed or closed Report again (the same authority as assessing it): the answer
    /// to what it awaited arrived, or the condition it waited for is met.
    Reopen {
        /// The Report's ID.
        id: String,
        /// Why it comes back.
        #[arg(long)]
        reason: String,
    },
    /// A source's reliability, how it was earned and the closed Reports behind it, by the key a
    /// Report shows as its source (`source:NAME`, `agent:CODENAME`, `person:ID`, `probe:NAME`,
    /// `review` or `watchdog`).
    Source {
        /// The source's key.
        key: String,
    },
    /// Who is cleared for which classification: list, grant or revoke (access.read, and
    /// access.manage to change, on config:reports). A change holds from the next request.
    Clearance {
        #[command(subcommand)]
        command: ClearanceCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClearanceCommand {
    /// List each subject and the classification it reads up to.
    List,
    /// Clear a subject up to a classification, replacing what it had.
    Grant {
        /// person:ID, group:NAME or agent:CODENAME.
        subject: String,
        /// One of the deployment's classifications.
        classification: String,
    },
    /// Take a subject's clearance away.
    Revoke {
        /// person:ID, group:NAME or agent:CODENAME.
        subject: String,
    },
}

/// What `ai6ctl reports assess` takes.
#[derive(Debug, Args)]
pub(crate) struct AssessArgs {
    /// The Report's ID.
    id: String,
    /// What it becomes: for what an Agent proposed of a known kind, `accepted` (it stands, or
    /// becomes its target's next revision) or `no_action`.
    #[arg(value_parser = ["mission", "duplicate", "related", "no_action", "awaiting", "later", "decision", "accepted"])]
    assessment: String,
    /// Why, kept apart from the Report's own words.
    #[arg(long)]
    reason: String,
    /// For duplicate or related: the Report, Mission or evidence.
    #[arg(long, required_if_eq_any([("assessment", "duplicate"), ("assessment", "related")]))]
    of: Option<String>,
    /// For mission: the Mission proposed or amended for it.
    #[arg(long)]
    mission: Option<String>,
    /// For mission: the Agent role the first run of a Source item takes, one of the roster's.
    #[arg(long)]
    role: Option<String>,
    /// For awaiting: what is asked. For decision: what those who sign are asked.
    #[arg(long, required_if_eq_any([("assessment", "awaiting"), ("assessment", "decision")]))]
    question: Option<String>,
    /// For awaiting: who or what is asked.
    #[arg(long)]
    ask: Option<String>,
    /// For awaiting: how far the question goes, a handling mark the Report's own allows.
    #[arg(long)]
    via: Option<String>,
    /// For later: what it waits for.
    #[arg(long)]
    condition: Option<String>,
    /// For awaiting (its deadline) and later (its date): seconds until it comes back.
    #[arg(long, required_if_eq("assessment", "awaiting"))]
    wait_seconds: Option<u64>,
    /// How far the information is believed, 1 (confirmed) to 6 (cannot be judged).
    #[arg(long)]
    credibility: Option<u8>,
    /// From the deployment's priorities.
    #[arg(long)]
    priority: Option<String>,
    /// How likely what it says is, in the probability yardstick's words (remote chance,
    /// highly unlikely, unlikely, realistic possibility, likely, highly likely, almost
    /// certain); give --confidence with it.
    #[arg(long, requires = "confidence")]
    likelihood: Option<String>,
    /// high, moderate or low.
    #[arg(long, requires = "likelihood")]
    confidence: Option<String>,
}

/// What `ai6ctl reports file` takes.
#[derive(Debug, Args)]
pub(crate) struct FileArgs {
    /// operation:ID, station:ID, person:ID, global or m.
    scope: String,
    /// One line saying what it is for, which a search lists beside the title.
    #[arg(long)]
    summary: Option<String>,
    /// Topics a search and a facet use, comma separated.
    #[arg(long, value_delimiter = ',')]
    topics: Vec<String>,
    /// One line naming it.
    #[arg(long)]
    title: String,
    /// What was observed.
    #[arg(long)]
    text: String,
    /// A short label; finding by default. memory, tradecraft, rule and contact are what is
    /// known: they stand at once where you may write the scope, otherwise they wait for a verdict.
    #[arg(long)]
    kind: Option<String>,
    /// info, warning or critical; info by default.
    #[arg(long)]
    severity: Option<String>,
    /// From the deployment's priorities (critical, high, normal, low by default).
    #[arg(long)]
    priority: Option<String>,
    /// When it needs an answer by, as Unix seconds.
    #[arg(long)]
    deadline: Option<f64>,
    /// From the deployment's classifications; unset is the lowest.
    #[arg(long)]
    classification: Option<String>,
    /// From the deployment's handling marks.
    #[arg(long)]
    handling: Option<String>,
    /// Compartments a reader must each be read in to, comma separated.
    #[arg(long, value_delimiter = ',')]
    compartments: Vec<String>,
    /// A sanitised part, shown in its place to those cleared only for its marking.
    #[arg(long)]
    tearline: Option<String>,
    /// The tearline's classification, no higher than the Report's.
    #[arg(long, requires = "tearline")]
    tearline_classification: Option<String>,
    /// What it is tied to, as KIND:ID (mission, report, decision or evidence); repeatable.
    #[arg(long = "link")]
    links: Vec<String>,
}

fn address(server: &Url, segments: &[&str]) -> Result<Url, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .push("v1")
        .extend(segments);
    Ok(url)
}

/// `/v1/reports` with each filter that is given.
fn list_url(server: &Url, filters: [(&str, Option<String>); 7]) -> Result<Url, String> {
    let mut url = address(server, &["reports"])?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in filters {
            if let Some(value) = value {
                query.append_pair(key, &value);
            }
        }
    }
    Ok(url)
}

/// `text` with what is not safe in a file name replaced.
fn file_name(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A Report's document, as the Markdown alone.
async fn fetch_document(
    client: &Client,
    token: &str,
    url: Url,
) -> Result<(StatusCode, String), String> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .header("accept", "text/markdown")
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

/// The `.md` files `paths` name: each file, and a directory's, in name order, folders and all.
pub(crate) fn markdown_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if metadata.is_dir() {
            let mut inside = Vec::new();
            for entry in
                std::fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?
            {
                let inner = entry
                    .map_err(|error| format!("{}: {error}", path.display()))?
                    .path();
                if inner.is_dir() || is_markdown(&inner) {
                    inside.push(inner);
                }
            }
            inside.sort();
            found.extend(markdown_files(&inside)?);
        } else {
            found.push(path.clone());
        }
    }
    Ok(found)
}

/// Whether `path` names a Markdown document.
pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        })
}

/// Imports each document in turn, printing its ID, what became of it and its title. A document
/// the gateway refuses stops the run, after the ones before it were stored.
pub(crate) async fn import_documents(
    client: &Client,
    server: &Url,
    token: &str,
    files: &[PathBuf],
    dry_run: bool,
) -> Result<(), String> {
    let files = markdown_files(files)?;
    if files.is_empty() {
        return Err("no Markdown document to import".to_owned());
    }
    for file in files {
        let document = std::fs::read_to_string(&file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let mut url = address(server, &["reports", "import"])?;
        if dry_run {
            url.query_pairs_mut().append_pair("dry_run", "true");
        }
        let response = client
            .post(url)
            .bearer_auth(token)
            .header("content-type", "text/markdown")
            .body(document)
            .send()
            .await
            .map_err(|_| "could not contact Universal Exports".to_owned())?;
        let status = response.status();
        let answer: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let reason = answer
                .get("error")
                .and_then(Value::as_str)
                .map_or_else(String::new, |reason| format!(": {}", plain(reason)));
            return Err(format!(
                "{}: Universal Exports returned HTTP {status}{reason}",
                file.display()
            ));
        }
        let verb = if dry_run {
            format!("would be {}", text(&answer["change"]))
        } else {
            text(&answer["change"])
        };
        println!(
            "{}\t{verb}\t{}",
            text(&answer["id"]),
            text(&answer["title"])
        );
    }
    Ok(())
}

fn link_of(text: &str) -> Result<Value, String> {
    let (kind, id) = text
        .split_once(':')
        .filter(|(kind, id)| !kind.is_empty() && !id.is_empty())
        .ok_or_else(|| format!("{text:?} is not a link: use KIND:ID"))?;
    Ok(json!({"kind": kind, "id": id}))
}

fn print_listed(listed: &Value) {
    println!("ID\tSCOPE\tPRIORITY\tSTATE\tSEVERITY\tMARKINGS\tTITLE");
    for report in listed["reports"].as_array().into_iter().flatten() {
        let title = match report["sight"].as_str() {
            Some("content") => text(&report["title"]),
            Some("tearline") => "(tearline)".to_owned(),
            _ => "(withheld)".to_owned(),
        };
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            text(&report["id"]),
            text(&report["scope"]),
            text(&report["priority"]),
            text(&report["state"]),
            text(&report["severity"]),
            markings(report),
            title,
        );
    }
}

fn print_found(found: &Value) {
    println!("ID\tSCOPE\tKIND\tSTATE\tTITLE\tSUMMARY");
    for report in found["reports"].as_array().into_iter().flatten() {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            text(&report["id"]),
            text(&report["scope"]),
            text(&report["kind"]),
            text(&report["state"]),
            text(&report["title"]),
            text(&report["summary"]),
        );
    }
    println!("{} found", number(&found["total"]));
}

fn markings(report: &Value) -> String {
    let mut marks = Vec::new();
    for key in ["classification", "handling"] {
        if let Some(mark) = report[key].as_str() {
            marks.push(plain(mark));
        }
    }
    let compartments = list(&report["compartments"]);
    if !compartments.is_empty() {
        marks.push(compartments);
    }
    marks.join("/")
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: ReportsCommand,
) -> Result<(), String> {
    match command {
        ReportsCommand::File(args) => {
            let FileArgs {
                scope,
                summary,
                topics,
                title,
                text: body,
                kind,
                severity,
                priority,
                deadline,
                classification,
                handling,
                compartments,
                tearline,
                tearline_classification,
                links,
            } = *args;
            let links = links
                .iter()
                .map(|link| link_of(link))
                .collect::<Result<Vec<_>, _>>()?;
            let mut filing = json!({
                "scope": scope,
                "title": title,
                "text": body,
                "summary": summary,
                "topics": topics,
                "kind": kind,
                "severity": severity,
                "priority": priority,
                "deadline": deadline,
                "classification": classification,
                "handling": handling,
                "compartments": compartments,
                "links": links,
            });
            if let Some(tearline) = tearline {
                filing["tearline"] =
                    json!({"text": tearline, "classification": tearline_classification});
            }
            let filed = exchange(
                client,
                token,
                Method::POST,
                address(server, &["reports"])?,
                None,
                Some(filing),
            )
            .await?;
            println!("{}\t{}", text(&filed["id"]), text(&filed["scope"]));
        }
        ReportsCommand::List {
            scope,
            mission,
            state,
            words,
            topics,
            title,
            kind,
        } => {
            let searching =
                words.is_some() || topics.is_some() || title.is_some() || kind.is_some();
            let url = list_url(
                server,
                [
                    ("scope", scope),
                    ("mission", mission),
                    ("state", state),
                    ("words", words),
                    ("topics", topics),
                    ("title", title),
                    ("kind", kind),
                ],
            )?;
            let listed = exchange(client, token, Method::GET, url, None, None).await?;
            if searching {
                print_found(&listed);
            } else {
                print_listed(&listed);
            }
        }
        ReportsCommand::Revise {
            id,
            revision,
            title,
            text: body,
            summary,
            topics,
        } => {
            let written = exchange(
                client,
                token,
                Method::PUT,
                address(server, &["reports", &id])?,
                None,
                Some(json!({
                    "revision": revision,
                    "title": title,
                    "text": body,
                    "summary": summary,
                    "topics": topics,
                })),
            )
            .await?;
            println!(
                "{}\t{}\t{}",
                text(&written["id"]),
                text(&written["state"]),
                number(&written["revision"])
            );
        }
        ReportsCommand::Forget { id, revision } => {
            let mut url = address(server, &["reports", &id])?;
            if let Some(revision) = revision {
                url.query_pairs_mut()
                    .append_pair("revision", &revision.to_string());
            }
            let gone = exchange(client, token, Method::DELETE, url, None, None).await?;
            if gone["forgotten"].is_string() {
                println!("{}\tforgotten", text(&gone["forgotten"]));
            } else {
                println!("{}\t{}", text(&gone["id"]), text(&gone["state"]));
            }
        }
        ReportsCommand::Versions { id } => {
            let held = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", &id, "versions"])?,
                None,
                None,
            )
            .await?;
            println!("REVISION\tUPDATED_BY\tTITLE");
            for version in held["versions"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}",
                    number(&version["revision"]),
                    text(&version["updated_by"]),
                    text(&version["title"])
                );
            }
        }
        ReportsCommand::Assess(args) => {
            let AssessArgs {
                id,
                assessment,
                reason,
                of,
                mission,
                role,
                question,
                ask,
                via,
                condition,
                wait_seconds,
                credibility,
                priority,
                likelihood,
                confidence,
            } = *args;
            let mut body = json!({"assessment": assessment, "reason": reason});
            for (key, value) in [
                ("of", of),
                ("mission", mission),
                ("role", role),
                ("question", question),
                ("ask", ask),
                ("via", via),
                ("condition", condition),
                ("priority", priority),
                ("likelihood", likelihood),
                ("confidence", confidence),
            ] {
                if let Some(value) = value {
                    body[key] = value.into();
                }
            }
            if let Some(wait) = wait_seconds {
                body["wait_seconds"] = wait.into();
            }
            if let Some(credibility) = credibility {
                body["credibility"] = credibility.into();
            }
            let decided = exchange(
                client,
                token,
                Method::PUT,
                address(server, &["reports", &id, "assessment"])?,
                None,
                Some(body),
            )
            .await?;
            println!(
                "{}\t{}\t{}",
                text(&decided["id"]),
                text(&decided["state"]),
                text(&decided["assessment"]["verdict"])
            );
        }
        ReportsCommand::Reopen { id, reason } => {
            let opened = exchange(
                client,
                token,
                Method::POST,
                address(server, &["reports", &id, "reopen"])?,
                None,
                Some(json!({"reason": reason})),
            )
            .await?;
            println!("{}\t{}", text(&opened["id"]), text(&opened["state"]));
        }
        ReportsCommand::Source { key } => {
            let source = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", "sources", &key])?,
                None,
                None,
            )
            .await?;
            for name in ["source", "reliability", "counted", "confirmed", "rated"] {
                println!("{name}\t{}", text(&source[name]));
            }
            print_listed(&json!({ "reports": source["reports"] }));
        }
        ReportsCommand::Clearance { command } => {
            run_clearance(client, server, token, command).await?;
        }
        ReportsCommand::Show { id, output } if output == "md" => {
            let (status, markdown) = fetch_document(
                client,
                token,
                address(server, &["reports", &id, "document"])?,
            )
            .await?;
            if !status.is_success() {
                return Err(format!("Universal Exports returned HTTP {status}"));
            }
            print!("{markdown}");
            if !markdown.ends_with('\n') {
                println!();
            }
        }
        ReportsCommand::Export {
            dir,
            scope,
            mission,
            state,
            words,
            topics,
            title,
            kind,
        } => {
            let url = list_url(
                server,
                [
                    ("scope", scope),
                    ("mission", mission),
                    ("state", state),
                    ("words", words),
                    ("topics", topics),
                    ("title", title),
                    ("kind", kind),
                ],
            )?;
            let listed = exchange(client, token, Method::GET, url, None, None).await?;
            let (mut written, mut markings_only) = (0usize, 0usize);
            for report in listed["reports"].as_array().into_iter().flatten() {
                let id = text(&report["id"]);
                let (status, markdown) = fetch_document(
                    client,
                    token,
                    address(server, &["reports", &id, "document"])?,
                )
                .await?;
                if status == StatusCode::FORBIDDEN {
                    markings_only += 1;
                    continue;
                }
                if !status.is_success() {
                    continue;
                }
                let kind = report["kind"].as_str().filter(|kind| !kind.is_empty());
                let folder = dir.join(file_name(kind.unwrap_or("report")));
                std::fs::create_dir_all(&folder)
                    .map_err(|error| format!("{}: {error}", folder.display()))?;
                let path = folder.join(format!("{}.md", file_name(&id)));
                std::fs::write(&path, markdown)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                written += 1;
            }
            println!("{written} written to {}", dir.display());
            if markings_only > 0 {
                println!("{markings_only} listed by their markings alone, not written");
            }
        }
        ReportsCommand::Import { files, dry_run } => {
            import_documents(client, server, token, &files, dry_run).await?;
        }
        ReportsCommand::Show { id, .. } => {
            let report = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", &id])?,
                None,
                None,
            )
            .await?;
            for key in [
                "id", "scope", "kind", "severity", "state", "priority", "source",
            ] {
                println!("{key}\t{}", text(&report[key]));
            }
            if !list(&report["topics"]).is_empty() {
                println!("topics\t{}", list(&report["topics"]));
            }
            for key in ["summary", "updated_by"] {
                if !report[key].is_null() {
                    println!("{key}\t{}", text(&report[key]));
                }
            }
            if let Some(revision) = report["revision"].as_u64() {
                println!("revision\t{revision}");
            }
            if report["exposed"] == true {
                println!("exposed\ttrue");
            }
            println!("markings\t{}", markings(&report));
            println!("reliability\t{}", text(&report["reliability"]));
            if report["untrusted"] == true {
                println!("untrusted\ttrue");
            }
            if report["sight"] == "tearline" {
                println!("shown\ttearline");
            } else {
                println!("title\t{}", text(&report["title"]));
            }
            println!();
            println!("{}", plain(report["text"].as_str().unwrap_or_default()));
        }
    }
    Ok(())
}

async fn run_clearance(
    client: &Client,
    server: &Url,
    token: &str,
    command: ClearanceCommand,
) -> Result<(), String> {
    match command {
        ClearanceCommand::List => {
            let listed = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", "clearances"])?,
                None,
                None,
            )
            .await?;
            println!("SUBJECT\tCLASSIFICATION\tBY");
            for clearance in listed["clearances"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}",
                    text(&clearance["subject"]),
                    text(&clearance["classification"]),
                    text(&clearance["by"]),
                );
            }
        }
        ClearanceCommand::Grant {
            subject,
            classification,
        } => {
            let granted = exchange(
                client,
                token,
                Method::PUT,
                address(server, &["reports", "clearances", &subject])?,
                None,
                Some(json!({"classification": classification})),
            )
            .await?;
            println!(
                "granted\t{}\t{}",
                text(&granted["subject"]),
                text(&granted["classification"])
            );
        }
        ClearanceCommand::Revoke { subject } => {
            let revoked = exchange(
                client,
                token,
                Method::DELETE,
                address(server, &["reports", "clearances", &subject])?,
                None,
                None,
            )
            .await?;
            let word = if revoked["revoked"] == true {
                "revoked"
            } else {
                "none held"
            };
            println!("{word}\t{}", plain(&subject));
        }
    }
    Ok(())
}

/// A number as it prints; empty when it is not one.
fn number(value: &Value) -> String {
    value
        .as_u64()
        .map_or_else(String::new, |value| value.to_string())
}
