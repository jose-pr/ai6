//! `ai6ctl access` and `ai6ctl accounts`: the access policy and the accounts, through the
//! gateway's routes. A refusal prints the gateway's reason.

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::plain;

#[derive(Debug, Subcommand)]
pub(crate) enum AccessCommand {
    /// Show the access policy (needs access.read): roles, ordered rules, the delegation cap
    /// and the chief list; or an Operation's own policy.
    Show {
        /// Show this Operation's own policy instead of the global one.
        #[arg(long)]
        operation: Option<String>,
    },
    /// Set or remove a role.
    Role {
        #[command(subcommand)]
        command: RoleCommand,
    },
    /// Export or import the ordered rule list as TOML.
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Set the delegation cap (global only): the Operation actions an Operation's roles may hold.
    Delegable {
        #[command(subcommand)]
        command: DelegableCommand,
    },
    /// Set the stored chief list (chief only).
    Chief {
        #[command(subcommand)]
        command: ChiefCommand,
    },
    /// Explain whether a subject may take an action on a resource, and by which rule.
    Check {
        /// account:NAME, person:ID or group:NAME.
        subject: String,
        /// The action, such as mission.create.
        action: String,
        /// operation:ID, station:ID, config:NAME or *.
        resource: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RoleCommand {
    /// Make or replace a role (needs access.manage).
    Set {
        /// The role's name.
        name: String,
        /// Its actions, comma separated.
        #[arg(long, value_delimiter = ',', required = true)]
        actions: Vec<String>,
        /// What it is for.
        #[arg(long)]
        description: Option<String>,
        /// Change this Operation's own policy instead of the global one.
        #[arg(long)]
        operation: Option<String>,
    },
    /// Delete a role no rule names.
    Remove {
        /// The role's name.
        name: String,
        /// Change this Operation's own policy instead of the global one.
        #[arg(long)]
        operation: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RulesCommand {
    /// Write the rule list, with the revision it was read at, to FILE.
    Export {
        /// The file to write.
        file: std::path::PathBuf,
        /// This Operation's rules instead of the global ones.
        #[arg(long)]
        operation: Option<String>,
    },
    /// Replace the whole rule list with FILE's, refused when the policy changed since it was
    /// exported.
    Import {
        /// A file `rules export` wrote, edited.
        file: std::path::PathBuf,
        /// This Operation's rules instead of the global ones.
        #[arg(long)]
        operation: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DelegableCommand {
    /// Set the cap to these actions, comma separated.
    Set {
        /// The Operation actions.
        #[arg(value_delimiter = ',', required = true)]
        actions: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ChiefCommand {
    /// Make exactly these subjects the stored chief list.
    Set {
        /// Subjects: account:NAME, person:ID or group:NAME.
        #[arg(required = true)]
        subjects: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AccountsCommand {
    /// List the accounts (needs account.manage).
    List,
    /// Make an account; its password is printed once.
    Create {
        /// The account name.
        name: String,
        /// Its groups, comma separated (also needs access.manage).
        #[arg(long, value_delimiter = ',')]
        groups: Vec<String>,
    },
    /// Refuse an account.
    Disable {
        /// The account name.
        name: String,
    },
    /// Accept a disabled account again.
    Enable {
        /// The account name.
        name: String,
    },
    /// Replace an account's groups (also needs access.manage).
    Groups {
        /// The account name.
        name: String,
        /// The groups, comma separated.
        #[arg(value_delimiter = ',')]
        groups: Vec<String>,
    },
    /// Set a new password; it is printed once, to stdout only.
    ResetPassword {
        /// The account name.
        name: String,
    },
}

/// The rule list as a file holds it.
#[derive(Debug, Serialize, Deserialize)]
struct RulesFile {
    revision: String,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleEntry {
    subject: String,
    scope: String,
    role: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    propagate: bool,
}

const fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(value: &bool) -> bool {
    *value
}

fn url(server: &Url, segments: &[&str]) -> Result<Url, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend(["v1", "user"])
        .extend(segments);
    Ok(url)
}

/// The route of the global policy, or of one Operation's.
fn policy_path<'a>(operation: Option<&'a str>, tail: &[&'a str]) -> Vec<&'a str> {
    let mut path = match operation {
        Some(operation) => vec!["operations", operation, "access"],
        None => vec!["access"],
    };
    path.extend(tail);
    path
}

/// Sends one request; a refusal is the gateway's own reason.
async fn exchange(
    client: &Client,
    token: &str,
    method: Method,
    url: Url,
    revision: Option<&str>,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client.request(method, url).bearer_auth(token);
    if let Some(revision) = revision {
        request = request.header("if-match", format!("\"{revision}\""));
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    let reason = value
        .get("error")
        .and_then(Value::as_str)
        .map_or_else(String::new, |reason| format!(": {}", plain(reason)));
    Err(format!("Universal Exports returned HTTP {status}{reason}"))
}

fn text(value: &Value) -> String {
    value.as_str().map_or_else(String::new, plain)
}

fn list(value: &Value) -> String {
    value
        .as_array()
        .map(|items| items.iter().map(text).collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

fn print_policy(view: &Value, operation: Option<&str>) {
    println!("revision\t{}", text(&view["revision"]));
    if let Some(operation) = operation {
        println!("operation\t{}", plain(operation));
    }
    if let Some(by) = view.get("managed_by").and_then(Value::as_str) {
        println!("managed_by\t{}", plain(by));
    }
    if let Some(reason) = view.get("invalid").and_then(Value::as_str) {
        println!("invalid\t{}", plain(reason));
    }
    println!("writable\t{}", view["writable"]);
    for role in view["roles"].as_array().into_iter().flatten() {
        let mark = if role["editable"] == false {
            " (built in, fixed)"
        } else {
            ""
        };
        println!(
            "role\t{}{}\t{}",
            text(&role["name"]),
            mark,
            list(&role["actions"])
        );
    }
    for (index, rule) in view["rules"].as_array().into_iter().flatten().enumerate() {
        let reach = if rule["propagate"] == false {
            " (this scope only)"
        } else {
            ""
        };
        println!(
            "rule {index}\t{}\t{}\t{}{}",
            text(&rule["subject"]),
            text(&rule["scope"]),
            text(&rule["role"]),
            reach
        );
    }
    if operation.is_none() {
        let cap = &view["delegable"];
        let note = if cap["default"] == true {
            " (default)"
        } else {
            ""
        };
        println!("delegable\t{}{}", list(&cap["actions"]), note);
        let weakened = list(&cap["weakenings"]);
        if !weakened.is_empty() {
            println!("delegable weakens\t{weakened}");
        }
        println!("chief stored\t{}", list(&view["chief"]["stored"]));
        println!("chief host\t{}", list(&view["chief"]["host"]));
    } else {
        for role in view["inherited"].as_array().into_iter().flatten() {
            println!(
                "inherited\t{}\t{}\t{}",
                text(&role["role"]),
                text(&role["from"]),
                list(&role["actions"])
            );
        }
        for binding in view["bindings"].as_array().into_iter().flatten() {
            println!(
                "binding\t{}\t{}\t{}",
                text(&binding["subject"]),
                text(&binding["role"]),
                text(&binding["tier"])
            );
        }
        for orphan in view["orphaned"].as_array().into_iter().flatten() {
            println!(
                "orphaned\t{}\t{}",
                text(&orphan["item"]),
                text(&orphan["reason"])
            );
        }
    }
}

/// Runs an `access` command.
pub(crate) async fn run_access(
    client: &Client,
    server: &Url,
    token: &str,
    command: AccessCommand,
) -> Result<(), String> {
    let read = |operation: Option<String>| {
        let client = client.clone();
        let (server, token) = (server.clone(), token.to_owned());
        async move {
            let path = policy_path(operation.as_deref(), &[]);
            exchange(
                &client,
                &token,
                Method::GET,
                url(&server, &path)?,
                None,
                None,
            )
            .await
        }
    };
    match command {
        AccessCommand::Show { operation } => {
            let view = read(operation.clone()).await?;
            print_policy(&view, operation.as_deref());
        }
        AccessCommand::Role { command } => {
            let (operation, name, body, verb) = match command {
                RoleCommand::Set {
                    name,
                    actions,
                    description,
                    operation,
                } => {
                    let mut body = json!({ "actions": actions });
                    if let Some(description) = description {
                        body["description"] = json!(description);
                    }
                    (operation, name, Some(body), Method::PUT)
                }
                RoleCommand::Remove { name, operation } => (operation, name, None, Method::DELETE),
            };
            let revision = text(&read(operation.clone()).await?["revision"]);
            let path = policy_path(operation.as_deref(), &["roles", &name]);
            exchange(
                client,
                token,
                verb,
                url(server, &path)?,
                Some(&revision),
                body,
            )
            .await?;
            println!("role {} changed", plain(&name));
        }
        AccessCommand::Rules { command } => match command {
            RulesCommand::Export { file, operation } => {
                let view = read(operation).await?;
                let rules = view["rules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|rule| RuleEntry {
                        subject: text(&rule["subject"]),
                        scope: text(&rule["scope"]),
                        role: text(&rule["role"]),
                        propagate: rule["propagate"] != false,
                    })
                    .collect();
                let document = RulesFile {
                    revision: text(&view["revision"]),
                    rules,
                };
                let text = toml::to_string(&document).map_err(|error| error.to_string())?;
                std::fs::write(&file, text)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
                println!(
                    "{} rules written to {}",
                    document.rules.len(),
                    file.display()
                );
            }
            RulesCommand::Import { file, operation } => {
                let source = std::fs::read_to_string(&file)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
                let document: RulesFile = toml::from_str(&source)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
                let path = policy_path(operation.as_deref(), &["rules"]);
                let body = json!({ "rules": document.rules });
                exchange(
                    client,
                    token,
                    Method::PUT,
                    url(server, &path)?,
                    Some(&document.revision),
                    Some(body),
                )
                .await?;
                println!("{} rules imported", document.rules.len());
            }
        },
        AccessCommand::Delegable {
            command: DelegableCommand::Set { actions },
        } => {
            let revision = text(&read(None).await?["revision"]);
            exchange(
                client,
                token,
                Method::PUT,
                url(server, &["access", "delegable"])?,
                Some(&revision),
                Some(json!({ "actions": actions })),
            )
            .await?;
            println!("delegable set");
        }
        AccessCommand::Chief {
            command: ChiefCommand::Set { subjects },
        } => {
            let revision = text(&read(None).await?["revision"]);
            exchange(
                client,
                token,
                Method::PUT,
                url(server, &["access", "chief"])?,
                Some(&revision),
                Some(json!({ "chief": subjects })),
            )
            .await?;
            println!("chief list set");
        }
        AccessCommand::Check {
            subject,
            action,
            resource,
        } => {
            let mut target = url(server, &["access", "check"])?;
            target
                .query_pairs_mut()
                .append_pair("subject", &subject)
                .append_pair("action", &action)
                .append_pair("resource", &resource);
            let answer = exchange(client, token, Method::GET, target, None, None).await?;
            let allowed = if answer["allowed"] == true {
                "allowed"
            } else {
                "denied"
            };
            println!(
                "{allowed}\ttier {}\trule {}\trole {}",
                answer.get("tier").map_or("-".to_owned(), text),
                answer
                    .get("rule_index")
                    .map_or("-".to_owned(), ToString::to_string),
                answer.get("role").map_or("-".to_owned(), text)
            );
        }
    }
    Ok(())
}

/// Runs an `accounts` command.
pub(crate) async fn run_accounts(
    client: &Client,
    server: &Url,
    token: &str,
    command: Option<AccountsCommand>,
) -> Result<(), String> {
    let account_url = |name: &str, tail: &[&str]| {
        let mut path = vec!["accounts", name];
        path.extend(tail);
        url(server, &path)
    };
    let show_secret = |answer: &Value| {
        if let Some(secret) = answer.get("secret").and_then(Value::as_str) {
            println!("password\t{secret}");
        }
    };
    match command.unwrap_or(AccountsCommand::List) {
        AccountsCommand::List => {
            let answer = exchange(
                client,
                token,
                Method::GET,
                url(server, &["accounts"])?,
                None,
                None,
            )
            .await?;
            println!("NAME\tGROUPS\tDISABLED");
            for account in answer.as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}",
                    text(&account["name"]),
                    list(&account["groups"]),
                    account["disabled"]
                );
            }
        }
        AccountsCommand::Create { name, groups } => {
            let body = if groups.is_empty() {
                json!({})
            } else {
                json!({ "groups": groups })
            };
            let answer = exchange(
                client,
                token,
                Method::PUT,
                account_url(&name, &[])?,
                None,
                Some(body),
            )
            .await?;
            println!("account {} created", plain(&name));
            show_secret(&answer);
        }
        AccountsCommand::Disable { name } => {
            set_disabled(client, token, account_url(&name, &[])?, true).await?;
            println!("account {} disabled", plain(&name));
        }
        AccountsCommand::Enable { name } => {
            set_disabled(client, token, account_url(&name, &[])?, false).await?;
            println!("account {} enabled", plain(&name));
        }
        AccountsCommand::Groups { name, groups } => {
            exchange(
                client,
                token,
                Method::PUT,
                account_url(&name, &[])?,
                None,
                Some(json!({ "groups": groups })),
            )
            .await?;
            println!("groups of {} set", plain(&name));
        }
        AccountsCommand::ResetPassword { name } => {
            let answer = exchange(
                client,
                token,
                Method::POST,
                account_url(&name, &["secret"])?,
                None,
                None,
            )
            .await?;
            show_secret(&answer);
        }
    }
    Ok(())
}

async fn set_disabled(
    client: &Client,
    token: &str,
    url: Url,
    disabled: bool,
) -> Result<(), String> {
    exchange(
        client,
        token,
        Method::PUT,
        url,
        None,
        Some(json!({ "disabled": disabled })),
    )
    .await
    .map(|_| ())
}
