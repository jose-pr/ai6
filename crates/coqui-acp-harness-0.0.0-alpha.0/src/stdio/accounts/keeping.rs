//! What an account provider of a harness described by a [`HarnessDef`] does: it lists the kinds the table makes, watches
//! a login's file for a change nobody told the host of, renews a login with the harness's own command, and says what the
//! harness reports of a plan's usage. It runs no agent; the sign-in of a harness that has one is its own.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{
    AccountGrant, AccountKeeper, AccountOffer, AccountUsage, AccountUsageQuery, AccountWatch,
    LoginMethodInfo, ProcessSpawner, RenewReport, RenewRequest, WatchRequest,
};

use super::{Applied, HarnessDef, KindDef, apply, bad, identity_of, login_usable};
use crate::ThroughSpawner;
use crate::account::{Accounts, Files, Hold, Watching};
use crate::cli;
use crate::process::Launch;

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

/// The accounts of one harness, as an account provider keeps them. One lives as long as the provider.
#[derive(Debug)]
pub struct Keeping {
    def: &'static HarnessDef,
    /// What it knows of its accounts' files, and who hears of them.
    accounts: Arc<Accounts>,
}

impl Keeping {
    /// The accounts `def` takes.
    #[must_use]
    pub fn new(def: &'static HarnessDef) -> Self {
        Self {
            def,
            accounts: Arc::new(Accounts::new()),
        }
    }

    /// What the provider offers: the kinds of `def` that provide a standard (a kind whose standard the account chooses
    /// is made by its owner, not by a provider), and `logins`, the ways to sign in. No kind reports usage.
    #[must_use]
    pub fn offer(&self, logins: Vec<LoginMethodInfo>) -> AccountOffer {
        let mut offer = AccountOffer::new();
        offer.kinds = self
            .def
            .kinds
            .iter()
            .filter(|kind| !kind.provides.is_empty())
            .map(KindDef::info)
            .collect();
        offer.logins = logins;
        offer
    }

    /// The kind `grant` is of, when the provider keeps accounts of it. Nothing of another standard is taken: a harness
    /// applies those, the provider makes its own.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` for an id that is not one or a kind the provider does not list.
    pub fn own(&self, grant: &AccountGrant) -> Result<&'static KindDef, ProviderError> {
        grant.check_id()?;
        self.def
            .kind(&grant.kind)
            .filter(|kind| !kind.provides.is_empty())
            .ok_or_else(|| bad("this provider keeps no account of that kind"))
    }

    /// The look at `grant`'s login file, which writes nothing: `None` for a kind with no file to look at.
    fn look(
        &self,
        grant: &AccountGrant,
        applied: &Applied,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        keeper: Option<Object<dyn AccountKeeper>>,
    ) -> Result<Option<Hold>, ProviderError> {
        let files = Files::new(Arc::clone(spawner));
        if applied.needs_files && !files.available() {
            return Err(unsupported(&alloc::format!(
                "this spawner has no private files, which a {} account needs",
                grant.kind
            )));
        }
        let (Some(watched), Some(dir)) = (applied.watched, grant.state_dir.as_deref()) else {
            return Ok(None);
        };
        self.accounts
            .observe(
                files,
                keeper,
                &grant.id,
                dir,
                grant.revision,
                grant.credentials.expose(),
                vec![crate::account::Watched::new(watched.path, watched.key).usable(login_usable)],
            )
            .map(Some)
    }

    /// Starts watching `request.account`'s login file, which `poll` looks at again and again.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` as [`own`](Self::own) and for a grant the kind refuses; `UNSUPPORTED` for a kind that keeps no file a
    /// program renews, an account that holds no login yet, or a spawner with no files.
    pub fn watch(
        &self,
        request: WatchRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
        poll: Duration,
    ) -> Result<Object<dyn AccountWatch>, ProviderError> {
        let grant = &request.account;
        let kind = self.own(grant)?;
        if !(kind.rotates && kind.state_dir) {
            return Err(unsupported(
                "this kind of account keeps its credential in no file that a program renews, so there is nothing to watch",
            ));
        }
        let Some(applied) = apply(self.def, grant)? else {
            return Err(unsupported("this account has no file to watch"));
        };
        let spawner = Arc::new(spawner);
        match self.look(grant, &applied, &spawner, Some(keeper))? {
            Some(hold) => Ok(Watching::begin(hold, poll)),
            None => Err(unsupported(
                "this account holds no login yet, so there is nothing to watch",
            )),
        }
    }

    /// What `query.account` has of plan usage, which is `reason`: the harness reports none.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` as [`own`](Self::own) and for a grant the kind refuses.
    pub fn usage(
        &self,
        query: &AccountUsageQuery,
        reason: &str,
    ) -> Result<AccountUsage, ProviderError> {
        self.own(&query.account)?;
        apply(self.def, &query.account)?;
        let mut usage = AccountUsage::unsupported(reason);
        usage.account = Some(query.account.id.clone());
        Ok(usage)
    }

    /// Renews the login of `request.account` with the harness's own command (`program` and the table's `renew_args`),
    /// which `limit` bounds. The login file is looked at as it is: a login that is missing, blank or another account's is
    /// `GONE`, and the grant's copy is not put back in its place.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` as [`own`](Self::own) and for a grant the kind refuses; `UNSUPPORTED` for a kind with no renewal;
    /// `GONE` for a login that is lost; `INTERNAL` when the command could not be run or failed.
    pub fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
        program: &str,
        limit: Duration,
    ) -> Result<RenewReport, ProviderError> {
        let grant = &request.account;
        let kind = self.own(grant)?;
        let Some(applied) = apply(self.def, grant)? else {
            return Err(unsupported(
                "the harness's own login is the harness's to renew",
            ));
        };
        let args = match (kind.renews, self.def.renew_args) {
            (true, Some(args)) => args,
            _ => return Err(unsupported(kind.renew_unsupported)),
        };
        let spawner = Arc::new(spawner);
        let Some(hold) = self.look(grant, &applied, &spawner, None)? else {
            return Err(unsupported(kind.renew_unsupported));
        };
        let renewed = self.renew_with(&hold, &spawner, grant, &applied, (program, args, limit));
        hold.finish();
        renewed
    }

    fn renew_with(
        &self,
        hold: &Hold,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        grant: &AccountGrant,
        applied: &Applied,
        (program, args, limit): (&str, &[&str], Duration),
    ) -> Result<RenewReport, ProviderError> {
        let gone = |why: alloc::string::String| ProviderError::new(Status::GUATIAO_ERR_GONE, &why);
        let lost = |hold: &Hold| -> Option<alloc::string::String> {
            let found = hold.look().ok()?;
            found.lost.into_iter().next().or_else(|| hold.lost())
        };
        if let Some(why) = lost(hold) {
            return Err(gone(why));
        }
        let mut launch = Launch::new(program).args(args.iter().copied());
        for (name, value) in applied.environment.expose().iter() {
            launch = launch.env(name.as_str(), value.as_str());
        }
        launch.remove_env = self.def.variables.iter().map(OsString::from).collect();
        let done = cli::run(
            &ThroughSpawner::shared(Arc::clone(spawner)),
            &launch,
            None,
            limit,
            4096,
        )
        .map_err(|_| {
            ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                "the renewal's program could not be started",
            )
        })?;
        if !done.succeeded() {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                if done.timed_out {
                    "the renewal's program did not finish in time"
                } else {
                    "the renewal's program did not succeed"
                },
            ));
        }
        Self::refuse_another_account(spawner, grant, applied)?;
        let found = hold.look()?;
        if let Some(why) = found.lost.into_iter().next().or_else(|| hold.lost()) {
            return Err(gone(why));
        }
        match found.renewed.into_iter().next_back() {
            Some((credentials, expires)) => {
                let mut report = RenewReport::default();
                report.credentials = Some(credentials);
                report.expires_at = expires;
                Ok(report)
            }
            None => Ok(RenewReport::unchanged(
                "the login is not near its end, so the harness left it as it was",
            )),
        }
    }

    /// A renewal that left the login of another account in the file is undone: the grant's copy is put back, and the
    /// renewal is refused. Only a file that says whose login it is can be told apart.
    fn refuse_another_account(
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        grant: &AccountGrant,
        applied: &Applied,
    ) -> Result<(), ProviderError> {
        let Some(watched) = applied.watched else {
            return Ok(());
        };
        let Some(mine) = grant
            .credentials
            .expose()
            .get(watched.key)
            .and_then(identity_of)
        else {
            return Ok(());
        };
        let files = Files::new(Arc::clone(spawner));
        let Ok(Some(now)) = files.read(watched.path) else {
            return Ok(());
        };
        if identity_of(&now).is_some_and(|theirs| theirs != mine) {
            if let Some(old) = grant.credentials.expose().get(watched.key) {
                let _ = files.write(watched.path, old);
            }
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the renewed login belongs to another account",
            ));
        }
        Ok(())
    }
}
