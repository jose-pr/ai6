//! The pure parts of account pools: a pool of accounts that provide one credential standard, the pools a host makes of every standard
//! some account provides, and what an entry of an agent offers of them. Which accounts and kinds exist, and which standards a kind
//! provides, is the host's, behind [`Provides`].

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_api::standard::is_standard;
use coqui_acp_api::{HARNESS_LOGIN, LOCAL_ACCOUNT};

/// What the name of the host's own pool for a standard starts with.
pub const AUTOMATIC: &str = "any-";

/// The standard `name` is the host's own pool of, when it is one: `any-` and a standard's id.
#[must_use]
pub fn automatic_standard(name: &str) -> Option<&str> {
    name.strip_prefix(AUTOMATIC)
        .filter(|rest| is_standard(rest))
}

/// The name of the host's own pool for `standard`.
#[must_use]
pub fn automatic_name(standard: &str) -> String {
    format!("{AUTOMATIC}{standard}")
}

/// Whether a pool is shown in a listing: a person's own always, a host's `any-<standard>` only with two or more accounts, since
/// one cannot rotate. Resolving a pool by name does not ask this.
#[must_use]
pub const fn worth_listing(automatic: bool, accounts: usize) -> bool {
    !automatic || accounts >= 2
}

/// The host's knowledge of accounts and the standards they provide.
pub trait Provides {
    /// The names of the configured accounts, in order, without the automatic `local`.
    fn accounts(&self) -> Vec<String>;

    /// The kind of the configured account `name` and the standards it provides; `None` when it is no account or its kind is not
    /// known.
    fn kind_of(&self, account: &str) -> Option<(String, Vec<String>)>;

    /// The standards the logins of the harnesses provide, one for each harness that offers its login as `local`.
    fn logins(&self) -> Vec<String>;
}

/// The kind of the account `name` and the standards it provides; the automatic `local` is of the harness-login kind and provides
/// what the logins of the harnesses do.
fn provided(provides: &dyn Provides, name: &str) -> Option<(String, Vec<String>)> {
    if name == LOCAL_ACCOUNT {
        return Some((HARNESS_LOGIN.to_string(), provides.logins()));
    }
    provides.kind_of(name)
}

/// Why `member` cannot be in a pool of `standard`, naming the account, its kind's standards and the pool's.
///
/// # Errors
///
/// The reason, as a sentence.
pub fn misfit(provides: &dyn Provides, standard: &str, member: &str) -> Result<(), String> {
    let Some((kind, standards)) = provided(provides, member) else {
        return Err(format!(
            "{member} is not an account: the configuration does not hold it and its provider lists no such kind"
        ));
    };
    if standards.iter().any(|each| each == standard) {
        return Ok(());
    }
    let list = if standards.is_empty() {
        "none".to_string()
    } else {
        standards.join(", ")
    };
    Err(format!(
        "account {member} (kind {kind}) provides {list}, and the pool's standard is {standard}"
    ))
}

/// A pool a person declared: accounts that provide one standard.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Declared {
    /// The pool's name.
    pub name: String,
    /// The standard every member provides.
    pub standard: String,
    /// The members, by account name, in the order the pool takes them.
    pub accounts: Vec<String>,
}

impl Declared {
    /// A declared pool.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        standard: impl Into<String>,
        accounts: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            standard: standard.into(),
            accounts,
        }
    }
}

/// Checks that each member of `pool` provides its standard.
///
/// # Errors
///
/// The member that does not, with the standards on each side, led by the pool's name.
pub fn check_one(provides: &dyn Provides, pool: &Declared) -> Result<(), String> {
    for member in &pool.accounts {
        misfit(provides, &pool.standard, member)
            .map_err(|why| format!("pool {}: {why}", pool.name))?;
    }
    Ok(())
}

/// A pool and the accounts in it, by name, in the order it takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccountPool {
    /// The pool's name.
    pub name: String,
    /// The standard its members provide.
    pub standard: String,
    /// The host's own `any-<standard>`: in no file.
    pub automatic: bool,
    /// The members.
    pub accounts: Vec<String>,
}

/// Every pool of the host: those a person declared, in their order, then `any-<standard>` for each standard some account provides
/// (the logins of the harnesses included), each holding all the accounts that provide it, `local` first.
#[must_use]
pub fn listing(declared: &[Declared], provides: &dyn Provides) -> Vec<AccountPool> {
    let mut pools: Vec<AccountPool> = declared
        .iter()
        .map(|pool| AccountPool {
            name: pool.name.clone(),
            standard: pool.standard.clone(),
            automatic: false,
            accounts: pool.accounts.clone(),
        })
        .collect();
    let by_login = provides.logins();
    let mut standards: BTreeSet<String> = by_login.iter().cloned().collect();
    let kinds: Vec<(String, Vec<String>)> = provides
        .accounts()
        .into_iter()
        .map(|name| {
            let held = provides
                .kind_of(&name)
                .map(|(_, standards)| standards)
                .unwrap_or_default();
            (name, held)
        })
        .collect();
    for (_, held) in &kinds {
        standards.extend(held.iter().cloned());
    }
    for standard in standards {
        let mut accounts = Vec::new();
        if by_login.contains(&standard) {
            accounts.push(LOCAL_ACCOUNT.to_string());
        }
        accounts.extend(
            kinds
                .iter()
                .filter(|(_, held)| held.contains(&standard))
                .map(|(name, _)| name.clone()),
        );
        pools.push(AccountPool {
            name: automatic_name(&standard),
            automatic: true,
            standard,
            accounts,
        });
    }
    pools
}

/// [`listing`] as a person is shown it: without the host's pools that hold fewer than two accounts.
#[must_use]
pub fn listed(declared: &[Declared], provides: &dyn Provides) -> Vec<AccountPool> {
    let mut pools = listing(declared, provides);
    pools.retain(|pool| worth_listing(pool.automatic, pool.accounts.len()));
    pools
}

/// An account a pool holds, as one entry sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryMember {
    /// The account's name.
    pub name: String,
    /// Whether the entry offers it, by its own name or through a pool it names: a connection that holds no grant for accounts may
    /// run only on those.
    pub listed: bool,
}

impl EntryMember {
    /// A member of an entry's pool.
    #[must_use]
    pub fn new(name: impl Into<String>, listed: bool) -> Self {
        Self {
            name: name.into(),
            listed,
        }
    }
}

/// A pool as one entry of a harness offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryPool {
    /// The pool's name.
    pub name: String,
    /// The standard its members provide.
    pub standard: String,
    /// The host's own `any-<standard>`.
    pub automatic: bool,
    /// Whether the entry names it, which gives its members to every connection; any other pool the harness accepts is open to a
    /// connection that holds a grant for accounts.
    pub listed: bool,
    /// The members the entry can run.
    pub members: Vec<EntryMember>,
}

impl EntryPool {
    /// A pool of an entry.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        standard: impl Into<String>,
        (automatic, listed): (bool, bool),
        members: Vec<EntryMember>,
    ) -> Self {
        Self {
            name: name.into(),
            standard: standard.into(),
            automatic,
            listed,
            members,
        }
    }

    /// The members a connection may run on: all of them with a grant for accounts (`any`), else those the entry offers.
    pub fn usable(&self, any: bool) -> impl Iterator<Item = &EntryMember> {
        self.members
            .iter()
            .filter(move |member| any || member.listed || self.listed)
    }

    /// Whether a connection may choose this pool: the entry's own, or the host's `any-<standard>`, or any with a grant for accounts.
    #[must_use]
    pub const fn open_to(&self, any: bool) -> bool {
        any || self.listed || self.automatic
    }
}
