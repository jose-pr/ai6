//! Pool accounts: one row per account, claimed in the write transaction that serializes
//! every Registry writer.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::accounts::{
    AccountClaimRequest, AccountProfile, AccountRef, AccountState, AccountStore, QuotaWindow,
    add_tokens, claim_in, parse_state, release_in, renew_in, report_in, state_bytes, validate_ref,
};
use crate::directorates::registry::store::invalid_data;

fn account_in(tx: &mut Tx<'_>, account: &AccountRef) -> io::Result<AccountState> {
    match tx.query_opt(
        "SELECT record FROM ai6_accounts WHERE pool = $1 AND account = $2",
        &[
            account.pool.as_str().into(),
            account.account.as_str().into(),
        ],
    )? {
        Some(row) => parse_state(row.text(0)?.as_bytes()),
        None => Ok(AccountState::new(account.clone())),
    }
}

fn put_account(tx: &mut Tx<'_>, state: &AccountState, revision: i64) -> io::Result<()> {
    let text = String::from_utf8(state_bytes(state)?)
        .map_err(|_| invalid_data("account record is not UTF-8"))?;
    tx.execute(
        "INSERT INTO ai6_accounts (pool, account, record, revision) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (pool, account) DO UPDATE SET record = excluded.record, \
         revision = excluded.revision",
        &[
            state.account.pool.as_str().into(),
            state.account.account.as_str().into(),
            text.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

fn all_accounts(tx: &mut Tx<'_>) -> io::Result<Vec<AccountState>> {
    tx.query(
        "SELECT record FROM ai6_accounts ORDER BY pool, account",
        &[],
    )?
    .into_iter()
    .map(|row| parse_state(row.text(0)?.as_bytes()))
    .collect()
}

impl SqlRegistry {
    fn update_account(
        &self,
        account: &AccountRef,
        change: impl FnOnce(&mut AccountState),
    ) -> io::Result<()> {
        validate_ref(account)?;
        self.write(|tx, revision| {
            let mut state = account_in(tx, account)?;
            change(&mut state);
            put_account(tx, &state, revision)
        })
    }
}

impl AccountStore for SqlRegistry {
    fn declare_account(&self, account: &AccountRef, profile: &AccountProfile) -> io::Result<()> {
        self.update_account(account, |state| state.profile = Some(profile.clone()))
    }

    fn claim_account(
        &self,
        request: &AccountClaimRequest,
        now: f64,
    ) -> io::Result<Option<AccountState>> {
        let accounts: Vec<AccountRef> = request_accounts(request);
        for account in &accounts {
            validate_ref(account)?;
        }
        self.write(|tx, revision| {
            let mut states = BTreeMap::new();
            for account in &accounts {
                states.insert(account.account.clone(), account_in(tx, account)?);
            }
            let (chosen, changed) = claim_in(&mut states, request, now);
            for name in changed {
                if let Some(state) = states.get(&name) {
                    put_account(tx, state, revision)?;
                }
            }
            Ok(chosen.and_then(|name| states.remove(&name)))
        })
    }

    fn release_account(&self, station: &str, mission: &str) -> io::Result<bool> {
        self.write(|tx, revision| {
            let mut released = false;
            for mut state in all_accounts(tx)? {
                if release_in(&mut state, station, mission) {
                    put_account(tx, &state, revision)?;
                    released = true;
                }
            }
            Ok(released)
        })
    }

    fn renew_account_claims(
        &self,
        station: &str,
        missions: &[String],
        ttl: Duration,
        now: f64,
    ) -> io::Result<()> {
        self.write(|tx, revision| {
            for mut state in all_accounts(tx)? {
                if renew_in(&mut state, station, missions, ttl, now) {
                    put_account(tx, &state, revision)?;
                }
            }
            Ok(())
        })
    }

    fn report_quota(&self, account: &AccountRef, windows: &[QuotaWindow]) -> io::Result<()> {
        self.update_account(account, |state| report_in(state, windows))
    }

    fn limit_account(&self, account: &AccountRef, until: f64) -> io::Result<()> {
        self.update_account(account, |state| {
            state.limited_until = Some(state.limited_until.map_or(until, |own| own.max(until)));
        })
    }

    fn record_account_usage(&self, account: &AccountRef, tokens: u64, now: i64) -> io::Result<()> {
        self.update_account(account, |state| add_tokens(state, tokens, now))
    }

    fn accounts(&self) -> io::Result<Vec<AccountState>> {
        self.read(all_accounts)
    }
}

fn request_accounts(request: &AccountClaimRequest) -> Vec<AccountRef> {
    request
        .candidates()
        .iter()
        .map(|candidate| AccountRef::new(request.pool(), &candidate.account))
        .collect()
}
