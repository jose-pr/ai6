//! Pool accounts: one row per account, claimed in the write transaction that serializes
//! every Registry writer.

use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::accounts::{
    AccountClaimRequest, AccountProfile, AccountRef, AccountState, AccountStore, QuotaWindow,
    add_tokens, claim_among, parse_state, release_in, renew_in, report_in, state_bytes,
    validate_ref,
};
use crate::directorates::registry::limits::AccountLimits;
use crate::directorates::registry::store::invalid_data;

fn account_in(
    tx: &mut Tx<'_>,
    account: &AccountRef,
    limits: &AccountLimits,
) -> io::Result<AccountState> {
    match tx.query_opt(
        "SELECT record FROM ai6_accounts WHERE pool = $1 AND account = $2",
        &[
            account.pool.as_str().into(),
            account.account.as_str().into(),
        ],
    )? {
        Some(row) => parse_state(row.text(0)?.as_bytes(), limits),
        None => Ok(AccountState::new(account.clone())),
    }
}

fn put_account(
    tx: &mut Tx<'_>,
    state: &AccountState,
    revision: i64,
    limits: &AccountLimits,
) -> io::Result<()> {
    let text = String::from_utf8(state_bytes(state, limits)?)
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

fn all_accounts(tx: &mut Tx<'_>, limits: &AccountLimits) -> io::Result<Vec<AccountState>> {
    tx.query(
        "SELECT record FROM ai6_accounts ORDER BY pool, account",
        &[],
    )?
    .into_iter()
    .map(|row| parse_state(row.text(0)?.as_bytes(), limits))
    .collect()
}

impl SqlRegistry {
    fn update_account(
        &self,
        account: &AccountRef,
        change: impl FnOnce(&mut AccountState),
    ) -> io::Result<()> {
        validate_ref(account)?;
        let limits = &self.limits.accounts;
        self.write(|tx, revision| {
            let mut state = account_in(tx, account, limits)?;
            change(&mut state);
            put_account(tx, &state, revision, limits)
        })
    }
}

impl AccountStore for SqlRegistry {
    fn declare_account(
        &self,
        account: &AccountRef,
        station: &str,
        profile: &AccountProfile,
    ) -> io::Result<()> {
        self.update_account(account, |state| {
            state.profiles.insert(station.to_owned(), profile.clone());
        })
    }

    fn withdraw_account(&self, account: &AccountRef, station: &str) -> io::Result<()> {
        self.update_account(account, |state| {
            state.profiles.remove(station);
        })
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
        let limits = &self.limits.accounts;
        self.write(|tx, revision| {
            let (chosen, changed) = claim_among(all_accounts(tx, limits)?, request, now, limits);
            for state in &changed {
                put_account(tx, state, revision, limits)?;
            }
            Ok(chosen)
        })
    }

    fn release_account(&self, station: &str, mission: &str) -> io::Result<bool> {
        let limits = &self.limits.accounts;
        self.write(|tx, revision| {
            let mut released = false;
            for mut state in all_accounts(tx, limits)? {
                if release_in(&mut state, station, mission) {
                    put_account(tx, &state, revision, limits)?;
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
        let limits = &self.limits.accounts;
        self.write(|tx, revision| {
            for mut state in all_accounts(tx, limits)? {
                if renew_in(&mut state, station, missions, ttl, now) {
                    put_account(tx, &state, revision, limits)?;
                }
            }
            Ok(())
        })
    }

    fn report_quota(&self, account: &AccountRef, windows: &[QuotaWindow]) -> io::Result<()> {
        let limits = &self.limits.accounts;
        self.update_account(account, |state| report_in(state, windows, limits))
    }

    fn clear_account_limit(&self, account: &AccountRef) -> io::Result<bool> {
        let mut cleared = false;
        self.update_account(account, |state| {
            cleared = state.limited_until.take().is_some();
        })?;
        Ok(cleared)
    }

    fn limit_account(&self, account: &AccountRef, until: f64) -> io::Result<()> {
        self.update_account(account, |state| {
            state.limited_until = Some(state.limited_until.map_or(until, |own| own.max(until)));
        })
    }

    fn record_account_usage(&self, account: &AccountRef, tokens: u64, now: i64) -> io::Result<()> {
        let limits = &self.limits.accounts;
        self.update_account(account, |state| add_tokens(state, tokens, now, limits))
    }

    fn accounts(&self) -> io::Result<Vec<AccountState>> {
        self.read(|tx| all_accounts(tx, &self.limits.accounts))
    }
}

fn request_accounts(request: &AccountClaimRequest) -> Vec<AccountRef> {
    request
        .candidates()
        .iter()
        .map(|candidate| AccountRef::new(request.pool(), &candidate.account))
        .collect()
}
