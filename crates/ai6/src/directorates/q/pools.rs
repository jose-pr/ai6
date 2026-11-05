//! Account pools: shared provider accounts a Session draws one credential from.
//!
//! A pool (`[pool.<id>]` in `loadouts.toml`, or [`AccountPool::new`] in code) serves one
//! AgentHandler and lists accounts (`[pool.<id>.account.<name>]`). Each account says how the
//! harness reads its credential ([`AccountKind`]) and which loadout secret holds it, so every
//! secret backend and its rules apply. Which Operations may spend a pool is decided three
//! ways: the pool's `operations` globs, the Operation's `pools` list, and the pool's handler;
//! an Agent may pin one pool.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// How a harness reads an account's credential.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountKind {
    /// A Claude Code token from `claude setup-token`, in `CLAUDE_CODE_OAUTH_TOKEN`.
    ClaudeOauthToken,
    /// An Anthropic API key, in `ANTHROPIC_API_KEY`.
    AnthropicApiKey,
    /// An Anthropic bearer token, in `ANTHROPIC_AUTH_TOKEN`.
    AnthropicAuthToken,
    /// An OpenAI API key, logged in with `codex login --with-api-key` into a `CODEX_HOME`
    /// the Station makes for the Mission.
    OpenaiApiKey,
    /// A Codex access token, logged in with `codex login --with-access-token` into a
    /// `CODEX_HOME` the Station makes for the Mission.
    CodexAccessToken,
    /// The directory of an existing login (`CLAUDE_CONFIG_DIR` or `CODEX_HOME`), used as it
    /// is: the way to share a subscription that issues no token.
    ConfigDir,
    /// The login the harness keeps for itself. With no [`Account::files`] it is the login it
    /// already has where it runs (`grok login`, `agy`): AI6 hands over nothing and only keeps
    /// count of its use and its limits, so several routes can share one budget. With them, the
    /// login is sealed in the Registry to named Stations, written under a home of the Session's
    /// own, and what the harness renews there comes back.
    HarnessLogin,
}

/// Every kind, in configuration order.
const KINDS: [AccountKind; 7] = [
    AccountKind::ClaudeOauthToken,
    AccountKind::AnthropicApiKey,
    AccountKind::AnthropicAuthToken,
    AccountKind::OpenaiApiKey,
    AccountKind::CodexAccessToken,
    AccountKind::ConfigDir,
    AccountKind::HarnessLogin,
];

/// The variables that carry a provider credential to a harness. A pooled Session gets
/// exactly one account's, so every other one is removed from its environment.
pub const CREDENTIAL_VARIABLES: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "OPENAI_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_HOME",
];

impl AccountKind {
    /// The configuration spelling (`claude-oauth-token`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeOauthToken => "claude-oauth-token",
            Self::AnthropicApiKey => "anthropic-api-key",
            Self::AnthropicAuthToken => "anthropic-auth-token",
            Self::OpenaiApiKey => "openai-api-key",
            Self::CodexAccessToken => "codex-access-token",
            Self::ConfigDir => "config-dir",
            Self::HarnessLogin => "harness-login",
        }
    }

    /// The kind spelled `name`, or `None`.
    pub fn parse(name: &str) -> Option<Self> {
        KINDS.into_iter().find(|kind| kind.as_str() == name)
    }

    /// The variable an environment kind's value goes in; `None` for kinds that hand the
    /// harness a directory.
    pub fn env(self) -> Option<&'static str> {
        match self {
            Self::ClaudeOauthToken => Some("CLAUDE_CODE_OAUTH_TOKEN"),
            Self::AnthropicApiKey => Some("ANTHROPIC_API_KEY"),
            Self::AnthropicAuthToken => Some("ANTHROPIC_AUTH_TOKEN"),
            Self::OpenaiApiKey | Self::CodexAccessToken | Self::ConfigDir | Self::HarnessLogin => {
                None
            }
        }
    }

    /// The `codex login` flag that reads this kind's value from standard input, for the
    /// kinds logged in to a Mission's own `CODEX_HOME`.
    pub fn login_flag(self) -> Option<&'static str> {
        match self {
            Self::OpenaiApiKey => Some("--with-api-key"),
            Self::CodexAccessToken => Some("--with-access-token"),
            _ => None,
        }
    }

    /// Whether the value is a loadout secret (every kind but `config-dir` and `harness-login`).
    pub fn reads_secret(self) -> bool {
        !matches!(self, Self::ConfigDir | Self::HarnessLogin)
    }

    /// Whether a provider Session can move to another account of this kind and keep its
    /// history: only the environment kinds, whose history stays where the harness keeps it
    /// whatever the token.
    pub fn carries_session(self) -> bool {
        self.env().is_some()
    }

    /// The handler this kind belongs to, when it belongs to one.
    fn handler(self) -> Option<&'static str> {
        match self {
            Self::ClaudeOauthToken | Self::AnthropicApiKey | Self::AnthropicAuthToken => {
                Some("claude")
            }
            Self::OpenaiApiKey | Self::CodexAccessToken => Some("codex"),
            Self::ConfigDir | Self::HarnessLogin => None,
        }
    }
}

impl fmt::Display for AccountKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a pool picks among accounts with room.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PoolStrategy {
    /// The account with the most quota left (lowest reported window use, unknown counting
    /// as half), then fewest active Sessions, then least recently used, then priority.
    #[default]
    Headroom,
    /// The least recently used account.
    RoundRobin,
    /// The highest-priority account with room.
    Priority,
}

impl PoolStrategy {
    /// The configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Headroom => "headroom",
            Self::RoundRobin => "round_robin",
            Self::Priority => "priority",
        }
    }

    /// The strategy spelled `name`, or `None`.
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Headroom, Self::RoundRobin, Self::Priority]
            .into_iter()
            .find(|strategy| strategy.as_str() == name)
    }
}

/// One provider account of a pool: `[pool.<id>.account.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    name: String,
    kind: AccountKind,
    secret: Option<String>,
    dir: Option<PathBuf>,
    env: Option<String>,
    priority: i64,
    max_concurrent: Option<usize>,
    labels: BTreeMap<String, String>,
    files: Vec<String>,
    renew_command: Vec<String>,
    renew_env: BTreeMap<String, String>,
}

impl Account {
    /// An account of `kind` named `name`, priority 0, no concurrency limit. A secret kind
    /// reads the loadout secret of the same name until [`Account::secret`] names another; a
    /// `config-dir` account needs [`Account::dir`].
    pub fn new(name: impl Into<String>, kind: AccountKind) -> Self {
        Self {
            name: name.into(),
            kind,
            secret: None,
            dir: None,
            env: None,
            priority: 0,
            max_concurrent: None,
            labels: BTreeMap::new(),
            files: Vec::new(),
            renew_command: Vec::new(),
            renew_env: BTreeMap::new(),
        }
    }

    /// The files, relative to a home, a `harness-login` account's harness keeps its login in. The
    /// login is then sealed in the Registry rather than left on a host.
    #[must_use]
    pub fn files(mut self, files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.files = files.into_iter().map(Into::into).collect();
        self
    }

    /// The program and arguments that renew the login with no model turn, run with its home as
    /// `HOME` and working directory. None: it is left to its next real use.
    #[must_use]
    pub fn renew_command(mut self, command: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.renew_command = command.into_iter().map(Into::into).collect();
        self
    }

    /// A variable the renewal command is given, such as how early its CLI should renew.
    #[must_use]
    pub fn renew_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.renew_env.insert(name.into(), value.into());
        self
    }

    /// Reads the credential from the loadout secret `name` (`[secret.<name>]`).
    #[must_use]
    pub fn secret(mut self, name: impl Into<String>) -> Self {
        self.secret = Some(name.into());
        self
    }

    /// The login directory a `config-dir` account hands the harness.
    #[must_use]
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Puts the credential (or directory) in `variable` instead of the kind's own.
    #[must_use]
    pub fn env(mut self, variable: impl Into<String>) -> Self {
        self.env = Some(variable.into());
        self
    }

    /// The priority the `priority` strategy and ties use; higher first.
    #[must_use]
    pub fn priority(mut self, priority: i64) -> Self {
        self.priority = priority;
        self
    }

    /// The most Sessions that may use the account at once, across every Station.
    #[must_use]
    pub fn max_concurrent(mut self, limit: usize) -> Self {
        self.max_concurrent = Some(limit);
        self
    }

    /// Adds a label shown with the account.
    #[must_use]
    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// The account's name within its pool.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How the harness reads its credential.
    pub fn kind(&self) -> AccountKind {
        self.kind
    }

    /// The loadout secret holding the credential; `None` for a `config-dir` account.
    pub fn secret_name(&self) -> Option<&str> {
        self.kind
            .reads_secret()
            .then(|| self.secret.as_deref().unwrap_or(&self.name))
    }

    /// The login directory of a `config-dir` account.
    pub fn login_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The priority; higher first.
    pub fn priority_value(&self) -> i64 {
        self.priority
    }

    /// The concurrency limit across every Station, if any.
    pub fn concurrency(&self) -> Option<usize> {
        self.max_concurrent
    }

    /// The account's labels.
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// The files its harness keeps its login in, relative to a home.
    pub fn login_files(&self) -> &[String] {
        &self.files
    }

    /// Whether its login is kept sealed in the Registry.
    pub fn keeps_login_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// The program and arguments that renew its login with no model turn.
    pub fn renewal(&self) -> &[String] {
        &self.renew_command
    }

    /// The variables the renewal command is given.
    pub fn renewal_env(&self) -> &BTreeMap<String, String> {
        &self.renew_env
    }

    /// The variable the harness reads the credential or directory from, given the pool's
    /// handler: the override, else the kind's own, else for `config-dir` `CLAUDE_CONFIG_DIR`
    /// (handler `claude`) or `CODEX_HOME` (`codex`). `None` when none applies, and for a
    /// `harness-login` account, which hands over nothing.
    pub fn variable(&self, handler: &str) -> Option<String> {
        if self.kind == AccountKind::HarnessLogin {
            return None;
        }
        if let Some(env) = &self.env {
            return Some(env.clone());
        }
        if let Some(env) = self.kind.env() {
            return Some(env.to_owned());
        }
        let variable = match (self.kind, handler) {
            (AccountKind::ConfigDir, "claude") => "CLAUDE_CONFIG_DIR",
            (AccountKind::ConfigDir, "codex")
            | (AccountKind::OpenaiApiKey | AccountKind::CodexAccessToken, _) => "CODEX_HOME",
            _ => return None,
        };
        Some(variable.to_owned())
    }

    /// Why this account is looser than an environment credential, if it is: a login kept on
    /// disk, or one written there for the Mission.
    pub fn weakening(&self, pool: &str) -> Option<String> {
        let name = &self.name;
        match self.kind {
            AccountKind::ConfigDir => Some(format!(
                "account {pool}/{name} hands the harness a login kept on disk in {}, which \
                 every process of its user can read",
                self.dir
                    .as_deref()
                    .map_or_else(|| "?".to_owned(), |dir| dir.display().to_string())
            )),
            AccountKind::OpenaiApiKey | AccountKind::CodexAccessToken => Some(format!(
                "account {pool}/{name} is logged in to a CODEX_HOME on the Station's disk for \
                 the Mission's life, since Codex reads its login only from there"
            )),
            AccountKind::HarnessLogin if self.keeps_login_files() => Some(format!(
                "account {pool}/{name}'s login is written under a home of each Session's own, \
                 which that Session's commands can read"
            )),
            _ => None,
        }
    }
}

/// A pool of accounts for one AgentHandler: `[pool.<id>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPool {
    id: String,
    handler: String,
    operations: Vec<String>,
    strategy: PoolStrategy,
    accounts: BTreeMap<String, Account>,
    login_command: Vec<String>,
    named_only: bool,
}

impl AccountPool {
    /// An empty pool `id` for the handler `handler`, open to every Operation, balanced by
    /// [`PoolStrategy::Headroom`], logging Codex accounts in with `codex`.
    pub fn new(id: impl Into<String>, handler: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            handler: handler.into(),
            operations: Vec::new(),
            strategy: PoolStrategy::default(),
            accounts: BTreeMap::new(),
            login_command: vec!["codex".to_owned()],
            named_only: false,
        }
    }

    /// Serves only an Agent or route that names the pool, so a pool of one source is not drawn
    /// by every other Agent of its handler.
    #[must_use]
    pub fn named_only(mut self, named_only: bool) -> Self {
        self.named_only = named_only;
        self
    }

    /// Whether only an Agent or route that names the pool draws from it.
    pub fn is_named_only(&self) -> bool {
        self.named_only
    }

    /// Opens the pool only to Operations matching one of these globs.
    #[must_use]
    pub fn operations(mut self, globs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.operations = globs.into_iter().map(Into::into).collect();
        self
    }

    /// Sets how the pool picks among accounts with room.
    #[must_use]
    pub fn strategy(mut self, strategy: PoolStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Adds an account, replacing one of the same name.
    #[must_use]
    pub fn account(mut self, account: Account) -> Self {
        self.accounts.insert(account.name.clone(), account);
        self
    }

    /// The program and leading arguments that log a Codex account in (`login` and the
    /// kind's flag are appended); `codex` by default.
    #[must_use]
    pub fn login_command(mut self, command: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.login_command = command.into_iter().map(Into::into).collect();
        self
    }

    /// The pool ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The AgentHandler provider ID the pool serves.
    pub fn handler(&self) -> &str {
        &self.handler
    }

    /// The Operation globs; empty means every Operation.
    pub fn operation_globs(&self) -> &[String] {
        &self.operations
    }

    /// How the pool picks among accounts.
    pub fn balancing(&self) -> PoolStrategy {
        self.strategy
    }

    /// The accounts in name order.
    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    /// The account `name`.
    pub fn get(&self, name: &str) -> Option<&Account> {
        self.accounts.get(name)
    }

    /// The program and leading arguments that log a Codex account in.
    pub fn login(&self) -> &[String] {
        &self.login_command
    }

    /// Whether the globs admit `operation`, by its ID or the ID of any Operation it is
    /// subordinate to (`ancestors`); a Mission outside any Operation is admitted only by a
    /// pool open to every Operation.
    pub fn admits(&self, operation: Option<&str>, ancestors: &[String]) -> bool {
        if self.operations.is_empty() {
            return true;
        }
        operation.is_some_and(|operation| {
            self.operations.iter().any(|pattern| {
                globset::Glob::new(pattern).is_ok_and(|glob| {
                    let matcher = glob.compile_matcher();
                    std::iter::once(operation)
                        .chain(ancestors.iter().map(String::as_str))
                        .any(|id| matcher.is_match(id))
                })
            })
        })
    }

    /// Checks the pool as a configuration file is checked.
    ///
    /// # Errors
    ///
    /// Names the pool, the account where it applies, and the first setting that is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if !valid_name(id) {
            return Err(format!(
                "pool {id:?}: the ID may use letters, digits, '.', '_' and '-'"
            ));
        }
        if !valid_name(&self.handler) {
            return Err(format!(
                "pool {id:?}: handler {:?} is not a provider ID",
                self.handler
            ));
        }
        if let Some(pattern) = self
            .operations
            .iter()
            .find(|pattern| pattern.trim().is_empty() || globset::Glob::new(pattern).is_err())
        {
            return Err(format!(
                "pool {id:?}: operations entry {pattern:?} is not a glob"
            ));
        }
        if self.accounts.is_empty() {
            return Err(format!("pool {id:?} has no accounts"));
        }
        if self.login_command.is_empty() || self.login_command.iter().any(|part| part.is_empty()) {
            return Err(format!("pool {id:?}: login_command needs a program"));
        }
        for (name, account) in &self.accounts {
            let at = |reason: &str| format!("pool {id:?} account {name:?}: {reason}");
            if !valid_name(name) || name != &account.name {
                return Err(at("the name may use letters, digits, '.', '_' and '-'"));
            }
            if let Some(owner) = account.kind.handler()
                && matches!(self.handler.as_str(), "claude" | "codex")
                && owner != self.handler
            {
                return Err(at(&format!(
                    "kind {} is for handler {owner:?}, not {:?}",
                    account.kind, self.handler
                )));
            }
            if account.kind == AccountKind::HarnessLogin {
                if account.secret.is_some() || account.dir.is_some() || account.env.is_some() {
                    return Err(at(
                        "a harness-login account hands over nothing: no `secret`, `dir` or `env`",
                    ));
                }
                if let Some(file) = account.files.iter().find(|file| !plain_relative(file)) {
                    return Err(at(&format!(
                        "file {file:?} is not a path inside a home, with no `..`"
                    )));
                }
            } else if !account.files.is_empty() {
                return Err(at("only a harness-login account takes `files`"));
            } else {
                match (account.kind.reads_secret(), &account.secret, &account.dir) {
                    (true, _, Some(_)) => return Err(at("only a config-dir account takes `dir`")),
                    (false, Some(_), _) => return Err(at("a config-dir account reads no secret")),
                    (false, None, None) => return Err(at("a config-dir account needs `dir`")),
                    _ => {}
                }
            }
            if let Some(secret) = account.secret_name()
                && !valid_name(secret)
            {
                return Err(at(&format!("secret {secret:?} is not a secret name")));
            }
            match account.variable(&self.handler) {
                None if account.kind == AccountKind::HarnessLogin => {}
                Some(variable) if super::transport::kit::is_variable_name(&variable) => {}
                Some(variable) => {
                    return Err(at(&format!("{variable:?} is not a variable name")));
                }
                None => {
                    return Err(at(&format!(
                        "set `env`: handler {:?} has no variable for a {} account",
                        self.handler, account.kind
                    )));
                }
            }
            if account.max_concurrent == Some(0) {
                return Err(at("max_concurrent must be greater than zero"));
            }
            if account.files.is_empty()
                && (!account.renew_command.is_empty() || !account.renew_env.is_empty())
            {
                return Err(at("`renew_command` and `renew_env` need `files`"));
            }
            if account.renew_command.iter().any(|part| part.is_empty())
                || (account.renew_command.is_empty() && !account.renew_env.is_empty())
            {
                return Err(at("`renew_env` needs a `renew_command` with a program"));
            }
            if let Some(name) = account
                .renew_env
                .keys()
                .find(|name| !super::transport::kit::is_variable_name(name))
            {
                return Err(at(&format!("{name:?} is not a variable name")));
            }
        }
        Ok(())
    }
}

/// The pools an Agent's Sessions may draw an account from, or why none may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolChoice<'a> {
    /// No pool serves the Agent's handler: its loadout alone supplies credentials.
    Unpooled,
    /// The pools to draw from, in the order to try them.
    Pools(Vec<&'a AccountPool>),
    /// Pools serve the handler but none may be used here; the start fails with this reason.
    Refused(String),
}

/// Applies the three-way rule to `pools`: a pool is usable when it serves `handler`, its
/// globs admit `operation` or one of its `ancestors` (nearest first), and `allowed` (the
/// Operation's `pools`; `None` is every pool) names it. `pinned` (the Agent's or route's
/// `pool`) narrows the choice to one pool, the only way to a pool that serves by name alone.
pub fn choose_pools<'a>(
    pools: impl IntoIterator<Item = &'a AccountPool>,
    handler: Option<&str>,
    operation: Option<&str>,
    ancestors: &[String],
    allowed: Option<&[String]>,
    pinned: Option<&str>,
) -> PoolChoice<'a> {
    let pools: Vec<&AccountPool> = pools.into_iter().collect();
    let where_ = operation.map_or_else(
        || "outside any Operation".to_owned(),
        |operation| format!("in Operation {operation:?}"),
    );
    if let Some(pinned) = pinned {
        let Some(pool) = pools.iter().find(|pool| pool.id == pinned) else {
            return PoolChoice::Refused(format!("pinned pool {pinned:?} is not configured"));
        };
        if Some(pool.handler.as_str()) != handler {
            return PoolChoice::Refused(format!(
                "pinned pool {pinned:?} serves handler {:?}, not the Agent's {:?}",
                pool.handler,
                handler.unwrap_or_default()
            ));
        }
        return match refusal(pool, operation, ancestors, allowed) {
            Some(reason) => {
                PoolChoice::Refused(format!("pinned pool {pinned:?} {reason} {where_}"))
            }
            None => PoolChoice::Pools(vec![pool]),
        };
    }
    let Some(handler) = handler else {
        return PoolChoice::Unpooled;
    };
    let serving: Vec<&AccountPool> = pools
        .into_iter()
        .filter(|pool| pool.handler == handler && !pool.named_only)
        .collect();
    if serving.is_empty() {
        return PoolChoice::Unpooled;
    }
    let mut reasons = Vec::new();
    let mut usable = Vec::new();
    for pool in serving {
        match refusal(pool, operation, ancestors, allowed) {
            Some(reason) => reasons.push(format!("pool {:?} {reason}", pool.id)),
            None => usable.push(pool),
        }
    }
    if usable.is_empty() {
        PoolChoice::Refused(format!(
            "handler {handler:?} draws accounts from pools, but none may be used {where_}: {}",
            reasons.join("; ")
        ))
    } else {
        PoolChoice::Pools(usable)
    }
}

/// Why `pool` may not be used for `operation`, worded to follow the pool's name.
fn refusal(
    pool: &AccountPool,
    operation: Option<&str>,
    ancestors: &[String],
    allowed: Option<&[String]>,
) -> Option<String> {
    if !pool.admits(operation, ancestors) {
        return Some(format!(
            "admits only Operations matching {}",
            pool.operations.join(", ")
        ));
    }
    if let Some(allowed) = allowed
        && !allowed.iter().any(|name| name == &pool.id)
    {
        return Some("is not in the Operation's `pools`".to_owned());
    }
    None
}

/// What [`choose_route`] compares about one route with room, from the numbers pools keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteStanding {
    /// Thousandths of quota used on the route's best account; `None` when no provider reported
    /// any, which counts as the `unknown_use` given to [`choose_route`].
    pub used_permille: Option<u16>,
    /// Sessions on the route's best account now.
    pub active: usize,
    /// When its best account last took a Session, as Unix seconds.
    pub last_used: Option<f64>,
    /// The route's `priority`; higher first.
    pub priority: i64,
}

/// Which of `open` an Agent's Session takes, as `strategy` says, an earlier entry winning every
/// tie: [`PoolStrategy::Headroom`] compares use, then Sessions, then who waited longest, then
/// priority; [`PoolStrategy::RoundRobin`] who waited longest; [`PoolStrategy::Priority`] priority
/// alone, so routes of equal priority go in the order they are listed. `None` when `open` is empty.
pub fn choose_route(
    strategy: PoolStrategy,
    open: &[RouteStanding],
    unknown_use: u16,
) -> Option<usize> {
    let last_used = |route: &RouteStanding| route.last_used.unwrap_or(f64::NEG_INFINITY);
    let compare = |left: &RouteStanding, right: &RouteStanding| {
        let by_last_used = last_used(left).total_cmp(&last_used(right));
        let by_priority = right.priority.cmp(&left.priority);
        match strategy {
            PoolStrategy::RoundRobin => by_last_used,
            PoolStrategy::Priority => by_priority,
            PoolStrategy::Headroom => left
                .used_permille
                .unwrap_or(unknown_use)
                .cmp(&right.used_permille.unwrap_or(unknown_use))
                .then(left.active.cmp(&right.active))
                .then(by_last_used)
                .then(by_priority),
        }
    };
    (0..open.len()).min_by(|left, right| compare(&open[*left], &open[*right]).then(left.cmp(right)))
}

/// Logs a Codex account in to a `CODEX_HOME`, for the kinds Codex reads only from there.
pub trait AccountLogin: Send + Sync {
    /// Logs `account` of `pool` in to `home` with `credential`. The message of an error never
    /// carries the credential.
    ///
    /// # Errors
    ///
    /// Why the login did not happen.
    fn login(
        &self,
        pool: &AccountPool,
        account: &Account,
        home: &Path,
        credential: &str,
    ) -> Result<(), String>;
}

/// Runs the pool's login command (`codex` by default) as `<command> login <flag>` with
/// `CODEX_HOME` set to the home and the credential on standard input, as
/// `printenv OPENAI_API_KEY | codex login --with-api-key` does.
#[derive(Debug, Clone, Copy)]
pub struct CommandLogin {
    timeout: std::time::Duration,
}

impl Default for CommandLogin {
    /// Gives a login a Station's default `[accounts] login_timeout`.
    fn default() -> Self {
        Self {
            timeout: crate::station::limits::AccountLimits::default()
                .login_timeout
                .0,
        }
    }
}

impl CommandLogin {
    /// Kills a login still running after `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl AccountLogin for CommandLogin {
    fn login(
        &self,
        pool: &AccountPool,
        account: &Account,
        home: &Path,
        credential: &str,
    ) -> Result<(), String> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let flag = account
            .kind()
            .login_flag()
            .ok_or_else(|| format!("a {} account is not logged in", account.kind()))?;
        let (program, arguments) = pool
            .login()
            .split_first()
            .ok_or("the pool has no login command")?;
        let mut child = Command::new(program)
            .args(arguments)
            .args(["login", flag])
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("{program} could not be started: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let written = stdin
                .write_all(credential.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"));
            drop(stdin);
            if let Err(error) = written {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} login did not read its credential: {error}"
                ));
            }
        }
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("{program} login failed: {status}")),
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{program} login did not finish within {:?}",
                        self.timeout
                    ));
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                Err(error) => {
                    return Err(format!("{program} login could not be waited on: {error}"));
                }
            }
        }
    }
}

/// Whether `file` stays inside the directory it is joined to.
fn plain_relative(file: &str) -> bool {
    let path = Path::new(file);
    !file.is_empty()
        && !file.contains('\\')
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn valid_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= 64
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(id: &str, handler: &str) -> AccountPool {
        AccountPool::new(id, handler).account(Account::new("a", AccountKind::ClaudeOauthToken))
    }

    #[test]
    fn each_kind_names_its_variable_and_whether_a_session_can_move() {
        for (kind, handler, variable, moves) in [
            (
                AccountKind::ClaudeOauthToken,
                "claude",
                "CLAUDE_CODE_OAUTH_TOKEN",
                true,
            ),
            (
                AccountKind::AnthropicApiKey,
                "claude",
                "ANTHROPIC_API_KEY",
                true,
            ),
            (
                AccountKind::AnthropicAuthToken,
                "claude",
                "ANTHROPIC_AUTH_TOKEN",
                true,
            ),
            (AccountKind::OpenaiApiKey, "codex", "CODEX_HOME", false),
            (AccountKind::CodexAccessToken, "codex", "CODEX_HOME", false),
            (AccountKind::ConfigDir, "claude", "CLAUDE_CONFIG_DIR", false),
            (AccountKind::ConfigDir, "codex", "CODEX_HOME", false),
        ] {
            let account = Account::new("a", kind).dir("/logins/a");
            assert_eq!(
                account.variable(handler).as_deref(),
                Some(variable),
                "{kind}"
            );
            assert_eq!(kind.carries_session(), moves, "{kind}");
            assert_eq!(AccountKind::parse(kind.as_str()), Some(kind));
            assert!(CREDENTIAL_VARIABLES.contains(&variable));
        }
        assert_eq!(AccountKind::parse("password"), None);
        assert_eq!(
            Account::new("a", AccountKind::ConfigDir).variable("other"),
            None
        );
        assert_eq!(
            Account::new("a", AccountKind::ConfigDir)
                .env("TOOL_HOME")
                .variable("other")
                .as_deref(),
            Some("TOOL_HOME")
        );
        assert_eq!(
            Account::new("a", AccountKind::AnthropicApiKey).secret_name(),
            Some("a")
        );
        assert_eq!(
            Account::new("a", AccountKind::AnthropicApiKey)
                .secret("vault-key")
                .secret_name(),
            Some("vault-key")
        );
        assert_eq!(
            Account::new("a", AccountKind::ConfigDir).secret_name(),
            None
        );
        assert!(
            Account::new("a", AccountKind::ConfigDir)
                .weakening("p")
                .is_some()
        );
        assert!(
            Account::new("a", AccountKind::OpenaiApiKey)
                .weakening("p")
                .is_some()
        );
        assert!(
            Account::new("a", AccountKind::ClaudeOauthToken)
                .weakening("p")
                .is_none()
        );
    }

    #[test]
    fn validation_names_the_pool_the_account_and_the_setting() {
        assert!(pool("team", "claude").validate().is_ok());
        for (bad, expected) in [
            (AccountPool::new("team", "claude"), "has no accounts"),
            (pool("bad id", "claude"), "the ID"),
            (pool("team", "claude").operations(["["]), "is not a glob"),
            (
                AccountPool::new("team", "codex")
                    .account(Account::new("a", AccountKind::ClaudeOauthToken)),
                "is for handler \"claude\"",
            ),
            (
                AccountPool::new("team", "claude")
                    .account(Account::new("a", AccountKind::ConfigDir)),
                "needs `dir`",
            ),
            (
                AccountPool::new("team", "claude").account(
                    Account::new("a", AccountKind::ConfigDir)
                        .dir("/x")
                        .secret("s"),
                ),
                "reads no secret",
            ),
            (
                AccountPool::new("team", "mock")
                    .account(Account::new("a", AccountKind::ConfigDir).dir("/x")),
                "set `env`",
            ),
            (
                AccountPool::new("team", "claude")
                    .account(Account::new("a", AccountKind::AnthropicApiKey).max_concurrent(0)),
                "max_concurrent",
            ),
            (
                AccountPool::new("team", "claude")
                    .account(Account::new("a", AccountKind::AnthropicApiKey).env("not a name")),
                "is not a variable name",
            ),
            (
                pool("team", "claude").login_command(Vec::<String>::new()),
                "login_command",
            ),
        ] {
            let reason = bad.validate().unwrap_err();
            assert!(reason.contains(expected), "{reason}");
        }
        assert!(
            AccountPool::new("team", "mock")
                .account(Account::new("a", AccountKind::OpenaiApiKey))
                .validate()
                .is_ok(),
            "an unknown handler takes any kind"
        );
    }

    #[test]
    fn the_handler_the_pool_globs_and_the_operation_list_must_all_agree() {
        let pools = [
            pool("claude-team", "claude").operations(["ai6-*"]),
            pool("claude-all", "claude"),
            AccountPool::new("codex-team", "codex")
                .account(Account::new("c", AccountKind::OpenaiApiKey)),
        ];
        let ids = |choice: PoolChoice<'_>| -> Vec<String> {
            match choice {
                PoolChoice::Pools(pools) => pools.iter().map(|pool| pool.id().to_owned()).collect(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            ids(choose_pools(
                &pools,
                Some("claude"),
                Some("ai6-web"),
                &[],
                None,
                None
            )),
            ["claude-team", "claude-all"]
        );
        assert_eq!(
            ids(choose_pools(
                &pools,
                Some("claude"),
                Some("other"),
                &[],
                None,
                None
            )),
            ["claude-all"]
        );
        assert_eq!(
            ids(choose_pools(
                &pools,
                Some("codex"),
                Some("other"),
                &[],
                None,
                None
            )),
            ["codex-team"]
        );
        assert_eq!(
            choose_pools(&pools, Some("gemini"), Some("other"), &[], None, None),
            PoolChoice::Unpooled,
            "a handler no pool serves keeps its loadout path"
        );
        assert_eq!(
            choose_pools(&pools, None, Some("other"), &[], None, None),
            PoolChoice::Unpooled
        );

        let only_team = ["claude-team".to_owned()];
        let PoolChoice::Refused(reason) = choose_pools(
            &pools,
            Some("claude"),
            Some("other"),
            &[],
            Some(&only_team),
            None,
        ) else {
            panic!("refused");
        };
        assert!(reason.contains("pool \"claude-team\" admits only Operations matching ai6-*"));
        assert!(reason.contains("pool \"claude-all\" is not in the Operation's `pools`"));
        let PoolChoice::Refused(reason) =
            choose_pools(&pools, Some("claude"), None, &[], Some(&[]), None)
        else {
            panic!("refused");
        };
        assert!(reason.contains("outside any Operation"), "{reason}");
    }

    fn standing(
        used: Option<u16>,
        active: usize,
        last_used: Option<f64>,
        priority: i64,
    ) -> RouteStanding {
        RouteStanding {
            used_permille: used,
            active,
            last_used,
            priority,
        }
    }

    #[test]
    fn each_balance_chooses_a_route_by_its_own_measure() {
        let open = [
            standing(Some(800), 0, Some(10.0), 5),
            standing(Some(100), 3, Some(30.0), 1),
            standing(None, 1, Some(20.0), 9),
        ];
        let choose = |strategy, unknown| choose_route(strategy, &open, unknown);
        assert_eq!(choose(PoolStrategy::Headroom, 500), Some(1), "least used");
        assert_eq!(
            choose(PoolStrategy::Headroom, 50),
            Some(2),
            "a route no provider reported on counts as the unknown use"
        );
        assert_eq!(
            choose(PoolStrategy::RoundRobin, 500),
            Some(0),
            "who waited longest"
        );
        assert_eq!(choose(PoolStrategy::Priority, 500), Some(2), "highest");
        assert_eq!(choose_route(PoolStrategy::Priority, &[], 500), None);
    }

    #[test]
    fn equal_routes_spread_by_sessions_and_last_use_and_ties_keep_the_listed_order() {
        let tied = [standing(None, 0, None, 0), standing(None, 0, None, 0)];
        for strategy in [
            PoolStrategy::Headroom,
            PoolStrategy::RoundRobin,
            PoolStrategy::Priority,
        ] {
            assert_eq!(choose_route(strategy, &tied, 500), Some(0), "{strategy:?}");
        }
        let busy = [standing(None, 1, None, 0), standing(None, 0, Some(9.0), 0)];
        assert_eq!(choose_route(PoolStrategy::Headroom, &busy, 500), Some(1));
        let used = [
            standing(None, 0, Some(9.0), 0),
            standing(None, 0, Some(5.0), 0),
        ];
        assert_eq!(choose_route(PoolStrategy::Headroom, &used, 500), Some(1));
    }

    #[test]
    fn a_pool_that_serves_by_name_is_drawn_only_when_named() {
        let pools = [
            pool("gateway", "claude").named_only(true),
            pool("team", "claude"),
        ];
        let ids = |choice: PoolChoice<'_>| -> Vec<String> {
            match choice {
                PoolChoice::Pools(pools) => pools.iter().map(|pool| pool.id().to_owned()).collect(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            ids(choose_pools(
                &pools,
                Some("claude"),
                Some("G"),
                &[],
                None,
                None
            )),
            ["team"],
            "an Agent that names no pool does not draw it"
        );
        assert_eq!(
            ids(choose_pools(
                &pools,
                Some("claude"),
                Some("G"),
                &[],
                None,
                Some("gateway")
            )),
            ["gateway"]
        );
        assert_eq!(
            choose_pools(&pools[..1], Some("claude"), Some("G"), &[], None, None),
            PoolChoice::Unpooled,
            "it is as if the handler had no pool"
        );
    }

    #[test]
    fn a_harness_login_account_hands_over_nothing_and_takes_no_setting_for_it() {
        let login = Account::new("free", AccountKind::HarnessLogin);
        assert_eq!(login.variable("acp"), None);
        assert_eq!(login.secret_name(), None);
        assert!(!login.kind().carries_session());
        assert_eq!(
            AccountKind::parse("harness-login"),
            Some(AccountKind::HarnessLogin)
        );
        assert!(
            AccountPool::new("grok", "acp")
                .account(login.clone())
                .validate()
                .is_ok()
        );
        for bad in [
            login.clone().secret("s"),
            login.clone().dir("/x"),
            login.clone().env("X"),
        ] {
            let reason = AccountPool::new("grok", "acp")
                .account(bad)
                .validate()
                .unwrap_err();
            assert!(reason.contains("hands over nothing"), "{reason}");
        }
    }

    #[test]
    fn an_account_that_keeps_its_login_in_files_names_them_inside_a_home() {
        let login = Account::new("free", AccountKind::HarnessLogin)
            .files([".grok/auth.json"])
            .renew_command(["grok", "models"])
            .renew_env("GROK_AUTH_EARLY_INVALIDATION_SECS", "3600");
        assert!(login.keeps_login_files());
        assert_eq!(login.variable("acp"), None);
        assert_eq!(login.secret_name(), None);
        assert!(login.weakening("grok").is_some());
        assert!(
            AccountPool::new("grok", "acp")
                .account(login.clone())
                .validate()
                .is_ok()
        );
        for (bad, expected) in [
            (login.clone().files(["../auth.json"]), "inside a home"),
            (login.clone().files(["/etc/passwd"]), "inside a home"),
            (login.clone().files([""]), "inside a home"),
            (
                Account::new("a", AccountKind::OpenaiApiKey).files(["x"]),
                "only a harness-login",
            ),
            (
                Account::new("a", AccountKind::HarnessLogin).renew_command(["grok"]),
                "need `files`",
            ),
            (
                Account::new("a", AccountKind::HarnessLogin)
                    .files(["x"])
                    .renew_env("A", "1"),
                "needs a `renew_command`",
            ),
            (
                Account::new("a", AccountKind::HarnessLogin)
                    .files(["x"])
                    .renew_command(["grok"])
                    .renew_env("NOT A NAME", "1"),
                "is not a variable name",
            ),
        ] {
            let reason = AccountPool::new("grok", "acp")
                .account(bad)
                .validate()
                .unwrap_err();
            assert!(reason.contains(expected), "{reason}");
        }
    }

    #[test]
    fn a_pool_admits_a_subordinate_operation_through_its_parent() {
        let pools = [pool("lcs", "claude").operations(["BODYGUARD"])];
        let lineage = ["FORTITUDE".to_owned(), "BODYGUARD".to_owned()];
        assert!(pools[0].admits(Some("SOUTH"), &lineage));
        assert!(!pools[0].admits(Some("SOUTH"), &[]));
        assert!(!pools[0].admits(None, &lineage));
        let PoolChoice::Pools(chosen) =
            choose_pools(&pools, Some("claude"), Some("SOUTH"), &lineage, None, None)
        else {
            panic!("admitted through BODYGUARD");
        };
        assert_eq!(chosen[0].id(), "lcs");
    }

    #[test]
    fn an_agent_may_pin_one_pool_and_each_pin_refusal_is_named() {
        let pools = [
            pool("claude-team", "claude").operations(["ai6-*"]),
            pool("claude-all", "claude"),
        ];
        let PoolChoice::Pools(chosen) = choose_pools(
            &pools,
            Some("claude"),
            Some("ai6-web"),
            &[],
            None,
            Some("claude-all"),
        ) else {
            panic!("pinned");
        };
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].id(), "claude-all");
        for (pinned, handler, operation, expected) in [
            ("missing", "claude", "ai6-web", "is not configured"),
            (
                "claude-all",
                "codex",
                "ai6-web",
                "serves handler \"claude\"",
            ),
            (
                "claude-team",
                "claude",
                "other",
                "admits only Operations matching",
            ),
        ] {
            let PoolChoice::Refused(reason) = choose_pools(
                &pools,
                Some(handler),
                Some(operation),
                &[],
                None,
                Some(pinned),
            ) else {
                panic!("{pinned} refused");
            };
            assert!(reason.contains(expected), "{reason}");
        }
    }
}
