//! How a Station runs: [`StationLimits`], the tables of the `station` record that bound and
//! pace its work, one table per area, and the provider configurations it hands out. Each
//! default is the value a Station applies when the record says nothing; the record applies at
//! restart.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional. A queue is a plain count of at least one, since it is allocated up front.
//!
//! Each table's fields are declared once here, with their title and default: the schema, the
//! reference tables in `docs/configuration.md` and `ai6 config schema` read them.

use std::collections::BTreeMap;

use guatiao::{FromValue, Schema, ToValue};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// Every table of the `station` record, as a Station keeps them while it runs.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationLimits {
    /// `[scheduler]`: the scheduler and its check-ins.
    pub scheduler: SchedulerLimits,
    /// `[publisher]`: publication to Universal Exports.
    pub publisher: PublisherLimits,
    /// `[sources]`: Source polling and Sleepers.
    pub sources: SourceLimits,
    /// `[transports]`: how often Theaters are looked over.
    pub transports: TransportLimits,
    /// `[recovery]`: re-attaching to Sessions after a restart.
    pub recovery: RecoveryLimits,
    /// `[vetting]`: vetting briefs and Mission proposals.
    pub vetting: VettingLimits,
    /// `[review]`: reviews of Missions' plans and output.
    pub review: ReviewLimits,
    /// `[accounts]`: pool accounts.
    pub accounts: AccountLimits,
    /// `[approvals]`: green ink steps.
    pub approvals: ApprovalLimits,
    /// `[harness]`: a working directory's harness configuration.
    pub harness: HarnessLimits,
    /// `[overlays]`: reading the overlays loadouts and Operations issue.
    pub overlays: OverlayLimits,
    /// `[provision]`: probing and provisioning Theaters.
    pub provision: ProvisionLimits,
    /// `[secrets]`: loadout secret leases.
    pub secrets: SecretLimits,
    /// `[terminal_output]`: decoding harness output.
    pub terminal_output: TerminalOutputLimits,
    /// `[green_ink]`: the harness gate.
    pub green_ink: GreenInkLimits,
    /// `[detached]`: harnesses run detached over a transport.
    pub detached: DetachedLimits,
    /// `[runner]`: field runners, the Station's client of them and `ai6 runner serve`.
    pub runner: RunnerLimits,
    /// `[api]`: the Station's own HTTP API.
    pub api: ApiLimits,
    /// `[audit]`: audit record details.
    pub audit: AuditLimits,
    /// `[theaters]`: ephemeral Theaters.
    pub theaters: TheaterLimits,
    /// `[theater_tools]`: the `theater_*` tools of remote-hands Sessions.
    pub theater_tools: TheaterToolLimits,
    /// `[head_of_station]`: the Head of Station's rounds.
    pub head_of_station: HeadOfStationLimits,
    /// `[probes]`: reading a Mission's deliverables back.
    pub probes: ProbeLimits,
    /// `[providers.<id>]`: the Guatiao configuration a Station hands provider `<id>` when it
    /// builds it with none of its own (handlers, terminals and safe houses).
    pub providers: BTreeMap<String, serde_json::Value>,
}

impl StationLimits {
    /// Checks what cannot work: a queue, page or attempt count of zero, or a provider
    /// configuration that is not a table.
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        for (key, value) in [
            (
                "scheduler.intake_attempts",
                u64::from(self.scheduler.intake_attempts),
            ),
            ("publisher.queue", self.publisher.queue as u64),
            (
                "publisher.control_queue",
                self.publisher.control_queue as u64,
            ),
            (
                "publisher.control_report_attempts",
                u64::from(self.publisher.control_report_attempts),
            ),
            (
                "recovery.give_up_tries",
                u64::from(self.recovery.give_up_tries),
            ),
            ("runner.replay_batch", self.runner.replay_batch as u64),
        ] {
            if value == 0 {
                return Err(format!("station.{key}: at least 1"));
            }
        }
        for (key, limit) in [
            ("scheduler.event_queue", self.scheduler.event_queue),
            ("publisher.terminal_bytes", self.publisher.terminal_bytes),
        ] {
            if limit.get() == Some(0) {
                return Err(format!("station.{key}: at least 1"));
            }
        }
        if self.accounts.unknown_use_permille > 1000 {
            return Err("station.accounts.unknown_use_permille: 0 to 1000".to_owned());
        }
        if let Some((id, _)) = self
            .providers
            .iter()
            .find(|(_, config)| !config.is_object())
        {
            return Err(format!("station.providers.{id}: a table"));
        }
        Ok(())
    }
}

/// `[scheduler]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SchedulerLimits {
    /// Events of one Session queued for its event pump; past it the oldest is dropped.
    #[schema(title = "Events queued per Session")]
    #[map(default = SchedulerLimits::default().event_queue)]
    pub event_queue: Limit,
    /// Seconds between scheduler rounds.
    #[schema(title = "Between scheduler rounds")]
    #[map(default = SchedulerLimits::default().interval)]
    pub interval: Seconds,
    /// Seconds before a Mission whose start failed is tried again.
    #[schema(title = "Retry a failed Mission start after")]
    #[map(default = SchedulerLimits::default().retry_delay)]
    pub retry_delay: Seconds,
    /// Seconds between the Station's check-ins with the Registry.
    #[schema(title = "Between check-ins with the Registry")]
    #[map(default = SchedulerLimits::default().check_in)]
    pub check_in: Seconds,
    /// Times a brief is placed before it is filed as failed.
    #[schema(title = "Tries to place a brief")]
    #[map(default = SchedulerLimits::default().intake_attempts)]
    pub intake_attempts: u32,
    /// Bytes of the Agent role catalog and of the placement policies a check-in carries; past it,
    /// each is left out.
    #[schema(
        title = "Largest role catalog and placement policies",
        format = "bytes"
    )]
    #[map(default = SchedulerLimits::default().role_catalog_bytes)]
    pub role_catalog_bytes: Limit,
    /// Seconds the Station waits for each of its loops when it stops.
    #[schema(title = "Wait for each loop when stopping")]
    #[map(default = SchedulerLimits::default().shutdown_wait)]
    pub shutdown_wait: Seconds,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            event_queue: Limit::at(256),
            interval: Seconds(std::time::Duration::from_millis(500)),
            retry_delay: Seconds::new(5),
            check_in: Seconds::new(15),
            intake_attempts: 3,
            role_catalog_bytes: Limit::at(512 * KIB),
            shutdown_wait: Seconds::new(2),
        }
    }
}

/// `[publisher]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct PublisherLimits {
    /// Seconds one request to Universal Exports may take.
    #[schema(title = "Longest request to Universal Exports")]
    #[map(default = PublisherLimits::default().request_timeout)]
    pub request_timeout: Seconds,
    /// Events waiting for Universal Exports; past it output gives way, oldest first, and a Session's
    /// latest state is kept.
    #[schema(title = "Events waiting to publish")]
    #[map(default = PublisherLimits::default().queue)]
    pub queue: usize,
    /// Bytes of terminal output in one published frame. Longer output goes as several, and what
    /// Universal Exports states at registration lowers it. With room for its header, the default is
    /// the gateway's default `frame_bytes`.
    #[schema(title = "Terminal output in one frame", format = "bytes")]
    #[map(default = PublisherLimits::default().terminal_bytes)]
    pub terminal_bytes: Limit,
    /// Bytes of terminal output kept for a Session not yet bound to its Mission; past it the oldest
    /// goes.
    #[schema(
        title = "Terminal output kept before a Mission binds",
        format = "bytes"
    )]
    #[map(default = PublisherLimits::default().pre_bind_terminal_bytes)]
    pub pre_bind_terminal_bytes: Limit,
    /// Seconds before a failed publication is tried again; each try doubles it, up to
    /// `retry_max_delay`.
    #[schema(title = "Retry a failed publication after")]
    #[map(default = PublisherLimits::default().publish_retry_delay)]
    pub publish_retry_delay: Seconds,
    /// The longest pause between tries, of a publication or of an input or control poll.
    #[schema(title = "Longest pause between tries")]
    #[map(default = PublisherLimits::default().retry_max_delay)]
    pub retry_max_delay: Seconds,
    /// Seconds between registering every live Session again, which is how a gateway restarted during
    /// a quiet spell learns of them.
    #[schema(title = "Register live Sessions again every")]
    #[map(default = PublisherLimits::default().refresh_every)]
    pub refresh_every: Seconds,
    /// Seconds before a failed input or control poll is tried again; each try doubles it, up to
    /// `retry_max_delay`.
    #[schema(title = "Retry a failed input or control poll after")]
    #[map(default = PublisherLimits::default().input_retry_delay)]
    pub input_retry_delay: Seconds,
    /// Seconds Universal Exports holds a control poll open; kept under `request_timeout`.
    #[schema(title = "Control poll held open")]
    #[map(default = PublisherLimits::default().control_wait)]
    pub control_wait: Seconds,
    /// Seconds Universal Exports holds an input poll open; a keystroke ends it at once, so a long
    /// wait costs no delay. The gateway's `max_poll_wait` bounds it.
    #[schema(title = "Input poll held open")]
    #[map(default = PublisherLimits::default().input_wait)]
    pub input_wait: Seconds,
    /// Mission controls queued for the scheduler.
    #[schema(title = "Mission controls queued")]
    #[map(default = PublisherLimits::default().control_queue)]
    pub control_queue: usize,
    /// Times the outcome of a control is reported.
    #[schema(title = "Tries to report a control's outcome")]
    #[map(default = PublisherLimits::default().control_report_attempts)]
    pub control_report_attempts: u32,
}

impl Default for PublisherLimits {
    fn default() -> Self {
        Self {
            request_timeout: Seconds::new(2),
            queue: 64,
            terminal_bytes: Limit::at(192 * 1024),
            pre_bind_terminal_bytes: Limit::at(MIB),
            publish_retry_delay: Seconds(std::time::Duration::from_millis(500)),
            retry_max_delay: Seconds::new(30),
            refresh_every: Seconds::new(15),
            input_retry_delay: Seconds(std::time::Duration::from_millis(100)),
            control_wait: Seconds::new(1),
            input_wait: Seconds::new(20),
            control_queue: 16,
            control_report_attempts: 3,
        }
    }
}

/// `[sources]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SourceLimits {
    /// Seconds between polls of a Source whose `interval_secs` is not set.
    #[schema(title = "Between polls of a Source")]
    #[map(default = SourceLimits::default().poll)]
    pub poll: Seconds,
    /// The shortest interval a Source is polled at, in seconds; a smaller `interval_secs` is raised
    /// to it, and 0 takes the floor away.
    #[schema(title = "Shortest poll of a Source")]
    #[map(default = SourceLimits::default().min_poll)]
    pub min_poll: Seconds,
    /// Seconds between rounds reporting finished briefs to their Sources.
    #[schema(title = "Report finished briefs every")]
    #[map(default = SourceLimits::default().report_every)]
    pub report_every: Seconds,
    /// Seconds between rounds firing Sleepers.
    #[schema(title = "Fire Sleepers every")]
    #[map(default = SourceLimits::default().sleeper_every)]
    pub sleeper_every: Seconds,
    /// Seconds between rounds sending the outbox of tracker writes.
    #[schema(title = "Send the tracker outbox every")]
    #[map(default = SourceLimits::default().outbox_every)]
    pub outbox_every: Seconds,
    /// Seconds between rounds reading new comments on the issues Missions are linked to.
    #[schema(title = "Read new issue comments every")]
    #[map(default = SourceLimits::default().reply_every)]
    pub reply_every: Seconds,
    /// Outbox entries sent in one round.
    #[schema(title = "Outbox entries sent per round")]
    #[map(default = SourceLimits::default().outbox_batch)]
    pub outbox_batch: Limit,
    /// Sends tried before an outbox entry fails.
    #[schema(title = "Tries to send an outbox entry")]
    #[map(default = SourceLimits::default().send_attempts)]
    pub send_attempts: Limit,
    /// Seconds before a failed send is tried again; each attempt doubles it.
    #[schema(title = "Retry a failed send after")]
    #[map(default = SourceLimits::default().retry_backoff)]
    pub retry_backoff: Seconds,
    /// Stored replies `issue_read` shows of a linked issue, newest first.
    #[schema(title = "Replies shown of a linked issue")]
    #[map(default = SourceLimits::default().replies_read)]
    pub replies_read: Limit,
    /// Seconds between rounds reading the newest settled items again, so a label taken off one
    /// is seen and its return reopens it.
    #[schema(title = "Read settled items every")]
    #[map(default = SourceLimits::default().reopen_watch_every)]
    pub reopen_watch_every: Seconds,
    /// Settled items read in each of those rounds, per tracker instance, newest first; each is
    /// one request to the tracker.
    #[schema(title = "Settled items read per round")]
    #[map(default = SourceLimits::default().reopen_watch)]
    pub reopen_watch: Limit,
    /// Seconds one ask for a live tracker read waits at Universal Exports before asking again.
    #[schema(title = "Wait for a live tracker read")]
    #[map(default = SourceLimits::default().read_wait)]
    pub read_wait: Seconds,
    /// The most bytes of one live tracker read Universal Exports asks for.
    #[schema(title = "Longest live tracker read asked for", format = "bytes")]
    #[map(default = SourceLimits::default().read_bytes)]
    pub read_bytes: Limit,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            poll: Seconds::new(60),
            min_poll: Seconds::new(10),
            report_every: Seconds::new(15),
            sleeper_every: Seconds::new(20),
            outbox_every: Seconds::new(5),
            reply_every: Seconds::new(60),
            outbox_batch: Limit::at(20),
            send_attempts: Limit::at(5),
            retry_backoff: Seconds::new(30),
            replies_read: Limit::at(10),
            reopen_watch_every: Seconds::new(300),
            reopen_watch: Limit::at(20),
            read_wait: Seconds::new(20),
            read_bytes: Limit::at(16 * 1024),
        }
    }
}

/// `[transports]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TransportLimits {
    /// Seconds before a Theater the Station could not reach is probed again.
    #[schema(title = "Probe a failed Theater again after")]
    #[map(default = TransportLimits::default().reprobe_after)]
    pub reprobe_after: Seconds,
    /// Seconds between looks over a Theater for its requirements and ambient credentials.
    #[schema(title = "Look a Theater over every")]
    #[map(default = TransportLimits::default().report_every)]
    pub report_every: Seconds,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            reprobe_after: Seconds::new(60),
            report_every: Seconds::new(300),
        }
    }
}

/// `[recovery]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct RecoveryLimits {
    /// Seconds before the first try to re-attach to a Session; each failed try doubles it.
    #[schema(title = "First re-attach try after")]
    #[map(default = RecoveryLimits::default().first_reattach_delay)]
    pub first_reattach_delay: Seconds,
    /// The longest wait between tries, in seconds.
    #[schema(title = "Longest wait between re-attach tries")]
    #[map(default = RecoveryLimits::default().max_reattach_delay)]
    pub max_reattach_delay: Seconds,
    /// Tries at a Registry step a concurrent renewal can refuse, when the Station gives a lease up.
    #[schema(title = "Tries to give a lease up")]
    #[map(default = RecoveryLimits::default().give_up_tries)]
    pub give_up_tries: u32,
    /// Seconds between writes of a Session's recent events to its record.
    #[schema(title = "Write a Session's recent events every")]
    #[map(default = RecoveryLimits::default().notes_every)]
    pub notes_every: Seconds,
    /// Seconds the look at whether a Session's Theater still answers may take.
    #[schema(title = "Longest look at a Theater")]
    #[map(default = RecoveryLimits::default().probe_timeout)]
    pub probe_timeout: Seconds,
}

impl Default for RecoveryLimits {
    fn default() -> Self {
        Self {
            first_reattach_delay: Seconds::new(5),
            max_reattach_delay: Seconds::new(60),
            give_up_tries: 10,
            notes_every: Seconds::new(15),
            probe_timeout: Seconds::new(30),
        }
    }
}

/// `[vetting]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct VettingLimits {
    /// Findings kept per verdict.
    #[schema(title = "Findings kept per verdict")]
    #[map(default = VettingLimits::default().findings)]
    pub findings: Limit,
    /// Other work a review brief lists for the reviewer.
    #[schema(title = "Other work a review brief lists")]
    #[map(default = VettingLimits::default().review_work)]
    pub review_work: Limit,
    /// Bytes of a finding's detail, of the lines a review brief quotes, and of a vetting timeline
    /// detail.
    #[schema(title = "Longest finding detail", format = "bytes")]
    #[map(default = VettingLimits::default().detail_bytes)]
    pub detail_bytes: Limit,
    /// Manipulation phrases an Operation may screen for; past it, or past `phrase_bytes`, screened
    /// briefs are held.
    #[schema(title = "Manipulation phrases per Operation")]
    #[map(default = VettingLimits::default().phrases)]
    pub phrases: Limit,
    /// Bytes of one such phrase.
    #[schema(title = "Longest manipulation phrase", format = "bytes")]
    #[map(default = VettingLimits::default().phrase_bytes)]
    pub phrase_bytes: Limit,
    /// Seconds of the rolling window hourly rates are counted over.
    #[schema(title = "Rates are counted over")]
    #[map(default = VettingLimits::default().rate_window)]
    pub rate_window: Seconds,
    /// Pending Mission proposals looked at per Operation each round, within
    /// `[registry.limits.proposals] results`.
    #[schema(title = "Proposals looked at per round")]
    #[map(default = VettingLimits::default().proposals)]
    pub proposals: Limit,
    /// Seconds the green ink request a class authorisation approves for a proposal lasts.
    #[schema(title = "A proposal's green ink request lasts")]
    #[map(default = VettingLimits::default().proposal_request_ttl)]
    pub proposal_request_ttl: Seconds,
    /// Seconds a proposal waits before M promotes it on its own.
    #[schema(title = "A proposal waits before it is promoted")]
    #[map(default = VettingLimits::default().proposal_settle)]
    pub proposal_settle: Seconds,
    /// Seconds between looks at rate holds for expiry.
    #[schema(title = "Look at rate holds every")]
    #[map(default = VettingLimits::default().expiry_sweep)]
    pub expiry_sweep: Seconds,
    /// Bytes at the end of a review's answer searched for its verdict; the answer ends the message.
    #[schema(title = "Review answer searched", format = "bytes")]
    #[map(default = VettingLimits::default().answer_tail_bytes)]
    pub answer_tail_bytes: Limit,
}

impl Default for VettingLimits {
    fn default() -> Self {
        Self {
            findings: Limit::at(16),
            review_work: Limit::at(50),
            detail_bytes: Limit::at(256),
            phrases: Limit::at(256),
            phrase_bytes: Limit::at(256),
            rate_window: Seconds::new(3_600),
            proposals: Limit::at(500),
            proposal_request_ttl: Seconds::new(3_600),
            proposal_settle: Seconds::new(30),
            expiry_sweep: Seconds::new(60),
            answer_tail_bytes: Limit::at(16 * KIB),
        }
    }
}

/// `[review]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ReviewLimits {
    /// Bytes of a Mission's brief a reviewer is given.
    #[schema(title = "Goals a reviewer reads", format = "bytes")]
    #[map(default = ReviewLimits::default().goals_bytes)]
    pub goals_bytes: Limit,
    /// Bytes of an Agent's plan or final message a reviewer is given.
    #[schema(title = "Report a reviewer reads", format = "bytes")]
    #[map(default = ReviewLimits::default().submitted_bytes)]
    pub submitted_bytes: Limit,
    /// Bytes of a Mission's change a reviewer is given; a change cut here says so.
    #[schema(title = "Change a reviewer reads", format = "bytes")]
    #[map(default = ReviewLimits::default().diff_bytes)]
    pub diff_bytes: Limit,
    /// Commits listed for a reviewer.
    #[schema(title = "Commits a reviewer is shown")]
    #[map(default = ReviewLimits::default().commits)]
    pub commits: Limit,
    /// Reasons kept from one vote, and told to an author sent back.
    #[schema(title = "Reasons kept per vote")]
    #[map(default = ReviewLimits::default().reasons)]
    pub reasons: Limit,
    /// Bytes of one reason, and of a line a review records.
    #[schema(title = "Longest reason", format = "bytes")]
    #[map(default = ReviewLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes at the end of a reviewer's answer searched for its vote; the vote ends the message.
    #[schema(title = "Reviewer's answer searched", format = "bytes")]
    #[map(default = ReviewLimits::default().answer_tail_bytes)]
    pub answer_tail_bytes: Limit,
    /// Times a Station tries a write to a review another Station keeps writing first, before it
    /// logs that and leaves it for the next round.
    #[schema(title = "Tries of a contested write")]
    #[map(default = ReviewLimits::default().write_tries)]
    pub write_tries: Limit,
    /// How recent a Station's check-in must be for the handlers it reports to count when a panel
    /// is filled.
    #[schema(title = "Handlers reported within")]
    #[map(default = ReviewLimits::default().station_window)]
    pub station_window: Seconds,
}

impl Default for ReviewLimits {
    fn default() -> Self {
        Self {
            goals_bytes: Limit::at(32 * KIB),
            submitted_bytes: Limit::at(32 * KIB),
            diff_bytes: Limit::at(64 * KIB),
            commits: Limit::at(100),
            reasons: Limit::at(16),
            reason_bytes: Limit::at(KIB),
            answer_tail_bytes: Limit::at(16 * KIB),
            write_tries: Limit::at(16),
            station_window: Seconds::new(300),
        }
    }
}

/// `[accounts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AccountLimits {
    /// Seconds past a reported reset before a Session parked on a limit resumes.
    #[schema(title = "Wait past a reported reset")]
    #[map(default = AccountLimits::default().reset_grace)]
    pub reset_grace: Seconds,
    /// Thousandths used an account whose provider reported nothing counts as, 0 to 1,000.
    #[schema(title = "Use assumed when none is reported")]
    #[map(default = AccountLimits::default().unknown_use_permille)]
    pub unknown_use_permille: u16,
    /// Seconds a claim on an account lasts unless the Station renews it at check-in.
    #[schema(title = "A claim on an account lasts")]
    #[map(default = AccountLimits::default().claim_ttl)]
    pub claim_ttl: Seconds,
    /// The longest a limited account is kept out, in seconds, whatever reset time its provider
    /// reports.
    #[schema(title = "Longest a limited account stays out")]
    #[map(default = AccountLimits::default().limit_horizon)]
    pub limit_horizon: Seconds,
    /// Seconds a pool account's login command may take.
    #[schema(title = "Longest account login")]
    #[map(default = AccountLimits::default().login_timeout)]
    pub login_timeout: Seconds,
    /// Seconds between this Station's looks at the models each account and route may use; 0 never
    /// looks on its own.
    #[schema(title = "Look at the models every")]
    #[map(default = AccountLimits::default().models_every)]
    pub models_every: Seconds,
    /// Accounts and routes a look at the models asks at once; `null` asks every one at once, and
    /// 0 asks none, so a look changes nothing.
    #[schema(title = "Model listings at once")]
    #[map(default = AccountLimits::default().model_probes)]
    pub model_probes: Limit,
    /// Seconds a look waits for one account's or route's model listing; one that takes longer
    /// keeps the list it had before, or is unknown.
    #[schema(title = "Longest model listing")]
    #[map(default = AccountLimits::default().model_probe_timeout)]
    pub model_probe_timeout: TimeLimit,
    /// Seconds between the Headquarters Station's rounds over the logins kept in the Registry; 0
    /// runs none.
    #[schema(title = "Between login renewal rounds")]
    #[map(default = AccountLimits::default().renew_every)]
    pub renew_every: Seconds,
    /// Seconds before a login's known end at which a round renews it: well before its harness's
    /// own threshold, so a Session's harness has no reason to.
    #[schema(title = "Renew a login this long before it ends")]
    #[map(default = AccountLimits::default().renew_within)]
    pub renew_within: Seconds,
    /// Seconds since a login was last renewed at which a round renews it, where its end cannot
    /// be read.
    #[schema(title = "Renew a login of unknown end after")]
    #[map(default = AccountLimits::default().renew_after)]
    pub renew_after: Seconds,
    /// Seconds a login's renewal command may take; never past its lease.
    #[schema(title = "Longest login renewal")]
    #[map(default = AccountLimits::default().renew_timeout)]
    pub renew_timeout: Seconds,
    /// Seconds a Station holds a login's renewal lease; another may take it after that.
    #[schema(title = "A login's renewal lease lasts")]
    #[map(default = AccountLimits::default().renew_lease)]
    pub renew_lease: Seconds,
    /// Seconds before the first retry of a failed renewal; each failure doubles it.
    #[schema(title = "First wait after a failed renewal")]
    #[map(default = AccountLimits::default().renew_backoff)]
    pub renew_backoff: Seconds,
    /// The longest wait between retries of a failed renewal, in seconds.
    #[schema(title = "Longest wait after a failed renewal")]
    #[map(default = AccountLimits::default().renew_backoff_max)]
    pub renew_backoff_max: Seconds,
    /// Failed renewals in a row before an account is marked as needing a person; `null` never
    /// marks one.
    #[schema(title = "Failed renewals before a person is needed")]
    #[map(default = AccountLimits::default().renew_attempts)]
    pub renew_attempts: Limit,
}

impl Default for AccountLimits {
    fn default() -> Self {
        Self {
            reset_grace: Seconds::new(15),
            unknown_use_permille: 500,
            claim_ttl: Seconds(crate::directorates::registry::store::DEFAULT_LEASE),
            limit_horizon: Seconds::new(8 * DAY),
            login_timeout: Seconds::new(60),
            models_every: Seconds::new(3_600),
            model_probes: Limit::at(4),
            model_probe_timeout: TimeLimit::secs(20),
            renew_every: Seconds::new(600),
            renew_within: Seconds::new(3_600),
            renew_after: Seconds::new(6 * DAY),
            renew_timeout: Seconds::new(120),
            renew_lease: Seconds::new(300),
            renew_backoff: Seconds::new(300),
            renew_backoff_max: Seconds::new(6 * 3_600),
            renew_attempts: Limit::at(5),
        }
    }
}

/// `[approvals]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ApprovalLimits {
    /// Bytes of the kind of a step an Agent asks about.
    #[schema(title = "Longest green ink step kind", format = "bytes")]
    #[map(default = ApprovalLimits::default().kind_bytes)]
    pub kind_bytes: Limit,
    /// Bytes of the target of a step an Agent asks about.
    #[schema(title = "Longest green ink step target", format = "bytes")]
    #[map(default = ApprovalLimits::default().target_bytes)]
    pub target_bytes: Limit,
    /// How deep nested shells (`sh -c "sh -c ..."`) and git aliases are followed when a command is
    /// read; the harness gate gets it in `AI6_GREEN_INK_NESTING`.
    #[schema(title = "Nesting followed when classifying a command")]
    #[map(default = ApprovalLimits::default().classify_nesting)]
    pub classify_nesting: usize,
    /// Steps one `check_approval` call may ask about.
    #[schema(title = "Steps checked at once")]
    #[map(default = ApprovalLimits::default().checked_steps)]
    pub checked_steps: Limit,
}

impl Default for ApprovalLimits {
    fn default() -> Self {
        Self {
            kind_bytes: Limit::at(32),
            target_bytes: Limit::at(256),
            classify_nesting: 4,
            checked_steps: Limit::at(64),
        }
    }
}

/// `[harness]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct HarnessLimits {
    /// Bytes of one file of a project's harness configuration.
    #[schema(title = "Largest harness configuration file", format = "bytes")]
    #[map(default = HarnessLimits::default().manifest_file_bytes)]
    pub manifest_file_bytes: Limit,
    /// Bytes of the whole configuration.
    #[schema(title = "Largest harness configuration", format = "bytes")]
    #[map(default = HarnessLimits::default().manifest_bytes)]
    pub manifest_bytes: Limit,
    /// Files in the configuration.
    #[schema(title = "Files in a harness configuration")]
    #[map(default = HarnessLimits::default().manifest_files)]
    pub manifest_files: Limit,
}

impl Default for HarnessLimits {
    fn default() -> Self {
        Self {
            manifest_file_bytes: Limit::at(MIB),
            manifest_bytes: Limit::at(16 * MIB),
            manifest_files: Limit::at(4_096),
        }
    }
}

/// `[overlays]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct OverlayLimits {
    /// Bytes of an overlay's `overlay.cfg`, or of one of its `aos` env files.
    #[schema(title = "Largest overlay manifest", format = "bytes")]
    #[map(default = OverlayLimits::default().manifest_bytes)]
    pub manifest_bytes: Limit,
    /// Nesting of a manifest's arrays and tables, its top table being the first level.
    #[schema(title = "Deepest overlay manifest")]
    #[map(default = OverlayLimits::default().manifest_depth)]
    pub manifest_depth: Limit,
    /// Files in one overlay, the excluded names (`.git`, `node_modules`, ...) not counted.
    #[schema(title = "Files in an overlay")]
    #[map(default = OverlayLimits::default().files)]
    pub files: Limit,
    /// Bytes of one overlay's files together.
    #[schema(title = "Largest overlay", format = "bytes")]
    #[map(default = OverlayLimits::default().bytes)]
    pub bytes: Limit,
    /// Bytes of one file of an overlay.
    #[schema(title = "Largest overlay file", format = "bytes")]
    #[map(default = OverlayLimits::default().file_bytes)]
    pub file_bytes: Limit,
    /// Issue an overlay whose `[overlay.<name>]` has no pin, reporting it as `unpinned` each
    /// time; without this such an overlay is refused. A weakening.
    #[schema(title = "Issue overlays without a pin")]
    #[map(default)]
    pub unpinned: bool,
    /// Bytes of the rules, routing and context a Session's overlays add to its initial context
    /// together. Past it the Session is refused, naming this setting, rather than its text cut.
    #[schema(title = "Largest overlay context", format = "bytes")]
    #[map(default = OverlayLimits::default().context_bytes)]
    pub context_bytes: Limit,
    /// Seconds a fetch of one overlay's Git source may take.
    #[schema(title = "Longest overlay fetch")]
    #[map(default = OverlayLimits::default().fetch_timeout)]
    pub fetch_timeout: Seconds,
    /// Seconds installing one overlay's dependency file may take.
    #[schema(title = "Longest dependency install")]
    #[map(default = OverlayLimits::default().deps_timeout)]
    pub deps_timeout: Seconds,
    /// Bytes of each output stream kept from an overlay's setup, env script or package manager.
    #[schema(title = "Largest output of an overlay's step", format = "bytes")]
    #[map(default = OverlayLimits::default().output_bytes)]
    pub output_bytes: Limit,
    /// The `dotagents.pyz` on the Station that runs the `cmds/` of an overlay holding them. An
    /// overlay with `cmds/` is refused without it. Default: none.
    #[schema(title = "dotagents for overlay commands")]
    pub dotagents_pyz: Option<String>,
    /// The SHA-256 `dotagents_pyz` must have, `sha256:<hex>`; a mismatch refuses. Without one it is
    /// refused unless `unpinned` allows it. Default: none.
    #[schema(title = "Pin of dotagents")]
    pub dotagents_pin: Option<String>,
}

impl Default for OverlayLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: Limit::at(MIB),
            manifest_depth: Limit::at(64),
            files: Limit::at(4_096),
            bytes: Limit::at(64 * MIB),
            file_bytes: Limit::at(16 * MIB),
            unpinned: false,
            // A harness on Windows takes its context on a command line of at most 32 KiB.
            context_bytes: Limit::at(24 * 1024),
            fetch_timeout: Seconds::new(300),
            deps_timeout: Seconds::new(600),
            output_bytes: Limit::at(MIB),
            dotagents_pyz: None,
            dotagents_pin: None,
        }
    }
}

impl OverlayLimits {
    /// How much of an overlay's tree is read.
    pub fn tree(&self) -> crate::directorates::q::overlays::TreeLimits {
        crate::directorates::q::overlays::TreeLimits {
            files: self.files,
            bytes: self.bytes,
            file_bytes: self.file_bytes,
        }
    }

    /// How much of a manifest is read.
    pub fn manifest(&self) -> crate::directorates::q::overlays::ReadLimits {
        crate::directorates::q::overlays::ReadLimits {
            bytes: self.manifest_bytes,
            depth: self.manifest_depth,
        }
    }

    /// One line per setting looser than its default: a larger limit, and `unpinned`.
    pub fn weakenings(&self) -> Vec<String> {
        let mut lines = crate::bounds::looser(
            "station.overlays",
            &serde_json::json!({
                "manifest_bytes": self.manifest_bytes.get(),
                "manifest_depth": self.manifest_depth.get(),
                "files": self.files.get(),
                "bytes": self.bytes.get(),
                "file_bytes": self.file_bytes.get(),
                "context_bytes": self.context_bytes.get(),
                "output_bytes": self.output_bytes.get(),
            }),
            &serde_json::json!({
                "manifest_bytes": Self::default().manifest_bytes.get(),
                "manifest_depth": Self::default().manifest_depth.get(),
                "files": Self::default().files.get(),
                "bytes": Self::default().bytes.get(),
                "file_bytes": Self::default().file_bytes.get(),
                "context_bytes": Self::default().context_bytes.get(),
                "output_bytes": Self::default().output_bytes.get(),
            }),
            &[],
        );
        if self.unpinned {
            lines.push(
                "station.overlays.unpinned = true: an overlay with no pin is issued, its files                  unchecked"
                    .to_owned(),
            );
        }
        lines
    }
}

/// `[provision]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ProvisionLimits {
    /// Seconds the requirements probe, and a resources sample, may take.
    #[schema(title = "Longest requirements probe")]
    #[map(default = ProvisionLimits::default().probe_timeout)]
    pub probe_timeout: Seconds,
    /// Seconds one provisioning command may take.
    #[schema(title = "Longest provisioning command")]
    #[map(default = ProvisionLimits::default().command_timeout)]
    pub command_timeout: Seconds,
    /// Bytes of a required program's name; a longer one fails the probe.
    #[schema(title = "Longest required program name", format = "bytes")]
    #[map(default = ProvisionLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Characters of a program's version kept from the probe.
    #[schema(title = "Longest program version kept")]
    #[map(default = ProvisionLimits::default().version_chars)]
    pub version_chars: Limit,
}

impl Default for ProvisionLimits {
    fn default() -> Self {
        Self {
            probe_timeout: Seconds::new(60),
            command_timeout: Seconds::new(120),
            name_bytes: Limit::at(64),
            version_chars: Limit::at(200),
        }
    }
}

/// `[secrets]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SecretLimits {
    /// Seconds the leases of a Session's loadout and pool account secrets last.
    #[schema(title = "A secret's lease lasts")]
    #[map(default = SecretLimits::default().lease_ttl)]
    pub lease_ttl: Seconds,
    /// The longest lease, in seconds; a green ink secret's lease lasts as long as its approval, up to
    /// it.
    #[schema(title = "Longest secret lease")]
    #[map(default = SecretLimits::default().max_lease_ttl)]
    pub max_lease_ttl: TimeLimit,
    /// Secrets shorter than this are not looked for in what an Agent writes outward: a short
    /// one would refuse ordinary words. Smaller is looser.
    #[schema(title = "Shortest secret looked for", format = "bytes")]
    #[map(default = SecretLimits::default().min_matched_bytes)]
    pub min_matched_bytes: u64,
}

impl Default for SecretLimits {
    fn default() -> Self {
        Self {
            lease_ttl: Seconds::new(3_600),
            max_lease_ttl: TimeLimit::secs(3_600),
            min_matched_bytes: 6,
        }
    }
}

/// `[terminal_output]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TerminalOutputLimits {
    /// Bytes of one line of a JSON harness's output held while the rest arrives.
    #[schema(title = "Longest line of JSON output", format = "bytes")]
    #[map(default = TerminalOutputLimits::default().json_line_bytes)]
    pub json_line_bytes: Limit,
    /// Bytes of one terminal control sequence before it is read as text.
    #[schema(title = "Longest terminal control sequence", format = "bytes")]
    #[map(default = TerminalOutputLimits::default().csi_bytes)]
    pub csi_bytes: Limit,
}

impl Default for TerminalOutputLimits {
    fn default() -> Self {
        Self {
            json_line_bytes: Limit::at(64 * MIB),
            csi_bytes: Limit::at(64),
        }
    }
}

/// `[green_ink]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct GreenInkLimits {
    /// Seconds the harness gate's exchange with the Station may take; the gate gets it in
    /// `AI6_GREEN_INK_TIMEOUT`.
    #[schema(title = "Longest harness gate exchange")]
    #[map(default = GreenInkLimits::default().gate_timeout)]
    pub gate_timeout: Seconds,
}

impl Default for GreenInkLimits {
    fn default() -> Self {
        Self {
            gate_timeout: Seconds::new(30),
        }
    }
}

/// `[detached]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct DetachedLimits {
    /// Seconds between reads of a detached harness's output.
    #[schema(title = "Read a detached harness's output every")]
    #[map(default = DetachedLimits::default().poll)]
    pub poll: Seconds,
    /// Seconds one of the wrapper's scripts may take over the transport.
    #[schema(title = "Longest wrapper script")]
    #[map(default = DetachedLimits::default().script_timeout)]
    pub script_timeout: Seconds,
    /// Characters of the Session's name in the wrapper's file names.
    #[schema(title = "Session name in the wrapper's files")]
    #[map(default = DetachedLimits::default().name_chars)]
    pub name_chars: Limit,
}

impl Default for DetachedLimits {
    fn default() -> Self {
        Self {
            poll: Seconds(std::time::Duration::from_millis(200)),
            script_timeout: Seconds::new(30),
            name_chars: Limit::at(64),
        }
    }
}

/// `[runner]`: the Station hands it to every field runner it starts, which applies the keys
/// a runner serves by.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct RunnerLimits {
    /// Seconds a runner waits for a harness's next event before it checks whether to stop.
    #[schema(title = "Check whether to stop after waiting")]
    #[map(default = RunnerLimits::default().pump_poll)]
    pub pump_poll: Seconds,
    /// Seconds between a runner connection's looks for spooled events to send.
    #[schema(title = "Look for spooled events every")]
    #[map(default = RunnerLimits::default().feed_poll)]
    pub feed_poll: Seconds,
    /// Spooled events a runner connection takes from the spool at once.
    #[schema(title = "Spooled events taken at once")]
    #[map(default = RunnerLimits::default().replay_batch)]
    pub replay_batch: usize,
    /// Seconds an ended Session is kept by its runner.
    #[schema(title = "Keep an ended Session for")]
    #[map(default = RunnerLimits::default().keep_ended)]
    pub keep_ended: Seconds,
    /// Bytes of one request through a runner's Cable relay.
    #[schema(title = "Largest request through the Cable relay", format = "bytes")]
    #[map(default = RunnerLimits::default().cable_request_bytes)]
    pub cable_request_bytes: Limit,
    /// Seconds the Station may take to reach its own Cable endpoint for a request a runner
    /// relays.
    #[schema(title = "The Station reaches its Cable endpoint within")]
    #[map(default = RunnerLimits::default().cable_connect_timeout)]
    pub cable_connect_timeout: Seconds,
    /// Requests through a runner's Cable relay the Station answers at once for one Session;
    /// past it a request is refused until one ends.
    #[schema(title = "Cable relay requests open at once")]
    #[map(default = RunnerLimits::default().cable_streams)]
    pub cable_streams: Limit,
    /// Seconds one read by the Station of a runner's Session waits for an event.
    #[schema(title = "One read of a Session waits")]
    #[map(default = RunnerLimits::default().read_wait)]
    pub read_wait: Seconds,
    /// Seconds `ai6 runner connect` may take to reach the runner's loopback port.
    #[schema(title = "`ai6 runner connect` reaches the runner within")]
    #[map(default = RunnerLimits::default().connect_timeout)]
    pub connect_timeout: Seconds,
    /// Seconds a runner that accepted `ai6 runner connect` has to answer its authentication.
    #[schema(title = "A runner answers its authentication within")]
    #[map(default = RunnerLimits::default().answer_wait)]
    pub answer_wait: Seconds,
    /// Seconds a started runner has to answer.
    #[schema(title = "A started runner answers within")]
    #[map(default = RunnerLimits::default().start_wait)]
    pub start_wait: Seconds,
    /// Seconds starting a runner over a transport may take past `start_wait`.
    #[schema(title = "Extra time to start over a transport")]
    #[map(default = RunnerLimits::default().start_margin)]
    pub start_margin: Seconds,
    /// Seconds between looks at whether a runner started on this host serves yet.
    #[schema(title = "Look at whether a started runner serves every")]
    #[map(default = RunnerLimits::default().start_poll)]
    pub start_poll: Seconds,
    /// Seconds one read of `ai6 runner connect`'s terminal waits before it checks whether to stop or
    /// give up.
    #[schema(title = "One read of the connect terminal waits")]
    #[map(default = RunnerLimits::default().terminal_poll)]
    pub terminal_poll: Seconds,
    /// Seconds a connection another one took a Session from stays open, so the notice saying so is
    /// written before the socket closes under it.
    #[schema(title = "Keep a superseded connection open for")]
    #[map(default = RunnerLimits::default().superseded_close_wait)]
    pub superseded_close_wait: Seconds,
    /// Seconds at least between the Station's tries at a runner it cannot reach.
    #[schema(title = "Shortest pause between tries at a runner")]
    #[map(default = RunnerLimits::default().retry_pause)]
    pub retry_pause: Seconds,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            pump_poll: Seconds(std::time::Duration::from_millis(250)),
            feed_poll: Seconds(std::time::Duration::from_millis(50)),
            replay_batch: 256,
            keep_ended: Seconds::new(7 * DAY),
            cable_request_bytes: Limit::at(4 * MIB),
            cable_connect_timeout: Seconds::new(10),
            cable_streams: Limit::at(64),
            read_wait: Seconds::new(1),
            connect_timeout: Seconds::new(5),
            answer_wait: Seconds::new(10),
            start_wait: Seconds::new(30),
            start_margin: Seconds::new(30),
            start_poll: Seconds(std::time::Duration::from_millis(50)),
            terminal_poll: Seconds(std::time::Duration::from_millis(200)),
            superseded_close_wait: Seconds(std::time::Duration::from_millis(200)),
            retry_pause: Seconds(std::time::Duration::from_millis(100)),
        }
    }
}

/// `[api]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ApiLimits {
    /// Bytes of a refused request's body the Station's own API reads before it answers.
    #[schema(title = "Refused request body read", format = "bytes")]
    #[map(default = ApiLimits::default().refused_body_drain_bytes)]
    pub refused_body_drain_bytes: Limit,
}

impl Default for ApiLimits {
    fn default() -> Self {
        Self {
            refused_body_drain_bytes: Limit::at(64 * KIB),
        }
    }
}

/// `[audit]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AuditLimits {
    /// Characters of the secret names a credential timeline event lists.
    #[schema(title = "Secret names a credential event lists")]
    #[map(default = AuditLimits::default().names_detail_chars)]
    pub names_detail_chars: Limit,
    /// Bytes of a refused configuration's audit target.
    #[schema(title = "Longest refused configuration target", format = "bytes")]
    #[map(default = AuditLimits::default().target_bytes)]
    pub target_bytes: Limit,
}

impl Default for AuditLimits {
    fn default() -> Self {
        Self {
            names_detail_chars: Limit::at(240),
            target_bytes: Limit::at(512),
        }
    }
}

/// `[theaters]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TheaterLimits {
    /// Bytes of an ephemeral Theater's name; a longer one is cut and given a digest.
    #[schema(title = "Longest ephemeral Theater name", format = "bytes")]
    #[map(default = TheaterLimits::default().ephemeral_name_bytes)]
    pub ephemeral_name_bytes: Limit,
}

impl Default for TheaterLimits {
    fn default() -> Self {
        Self {
            ephemeral_name_bytes: Limit::at(100),
        }
    }
}

/// `[theater_tools]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TheaterToolLimits {
    /// Bytes of each stream of a command's output `theater_run` returns.
    #[schema(title = "Output `theater_run` returns", format = "bytes")]
    #[map(default = TheaterToolLimits::default().output_bytes)]
    pub output_bytes: Limit,
    /// Bytes `theater_read` returns.
    #[schema(title = "What `theater_read` returns", format = "bytes")]
    #[map(default = TheaterToolLimits::default().read_bytes)]
    pub read_bytes: Limit,
    /// Seconds a `theater_run` command may take unless it asks for another.
    #[schema(title = "A command may take")]
    #[map(default = TheaterToolLimits::default().timeout)]
    pub timeout: Seconds,
    /// The longest a command may ask for, in seconds.
    #[schema(title = "Longest a command may ask for")]
    #[map(default = TheaterToolLimits::default().max_timeout)]
    pub max_timeout: TimeLimit,
    /// Seconds each git command the tools run for green ink may take.
    #[schema(title = "A git command may take")]
    #[map(default = TheaterToolLimits::default().git_timeout)]
    pub git_timeout: Seconds,
}

impl Default for TheaterToolLimits {
    fn default() -> Self {
        Self {
            output_bytes: Limit::at(256 * KIB),
            read_bytes: Limit::at(256 * KIB),
            timeout: Seconds::new(120),
            max_timeout: TimeLimit::secs(3_600),
            git_timeout: Seconds::new(30),
        }
    }
}

/// `[head_of_station]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct HeadOfStationLimits {
    /// Changes of a kept safe house listed in one report.
    #[schema(title = "Safe house changes in one report")]
    #[map(default = HeadOfStationLimits::default().listed_changes)]
    pub listed_changes: Limit,
    /// Directory entries one survey of a safe house walks before it reports a partial size.
    #[schema(title = "Directory entries one survey walks")]
    #[map(default = HeadOfStationLimits::default().survey_entries)]
    pub survey_entries: Limit,
    /// Bytes of the reason a round gives when it asks to delete a safe house; longer is cut.
    #[schema(title = "Longest reason for a safe house deletion", format = "bytes")]
    #[map(default = HeadOfStationLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes of what a provisioning round's audit record names; longer is cut.
    #[schema(title = "Longest provisioning audit target", format = "bytes")]
    #[map(default = HeadOfStationLimits::default().audit_target_bytes)]
    pub audit_target_bytes: Limit,
}

impl Default for HeadOfStationLimits {
    fn default() -> Self {
        Self {
            listed_changes: Limit::at(20),
            survey_entries: Limit::at(500_000),
            reason_bytes: Limit::at(2_000),
            audit_target_bytes: Limit::at(500),
        }
    }
}

/// `[probes]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ProbeLimits {
    /// Seconds a `git` read of a remote may take before the branch or commit is reported unread.
    #[schema(title = "Longest git probe")]
    #[map(default = ProbeLimits::default().git_timeout)]
    pub git_timeout: Seconds,
    /// Seconds an HTTP request for an artifact may take.
    #[schema(title = "Longest HTTP probe")]
    #[map(default = ProbeLimits::default().http_timeout)]
    pub http_timeout: Seconds,
    /// Seconds a read of a tracker item and its comments may take.
    #[schema(title = "Longest tracker probe")]
    #[map(default = ProbeLimits::default().tracker_timeout)]
    pub tracker_timeout: Seconds,
    /// Seconds a look at a file in a Mission's working directory may take.
    #[schema(title = "Longest file probe")]
    #[map(default = ProbeLimits::default().file_timeout)]
    pub file_timeout: Seconds,
    /// Comments of a tracker item a probe reads, the latest.
    #[schema(title = "Comments a probe reads")]
    #[map(default = ProbeLimits::default().comments)]
    pub comments: Limit,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        Self {
            git_timeout: Seconds::new(30),
            http_timeout: Seconds::new(20),
            tracker_timeout: Seconds::new(30),
            file_timeout: Seconds::new(5),
            comments: Limit::at(100),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::config::StationRuntime;
    use crate::directorates::m::records::from_toml;

    fn read(text: &str) -> Result<StationLimits, String> {
        from_toml::<StationRuntime>(text).map(|record| record.limits())
    }

    #[test]
    fn what_was_written_in_the_code_defaults_to_the_same_value() {
        assert_eq!(ApprovalLimits::default().checked_steps, Limit::at(64));
        assert_eq!(SourceLimits::default().replies_read, Limit::at(10));
        assert_eq!(
            VettingLimits::default().answer_tail_bytes,
            Limit::at(16_384)
        );
        assert_eq!(RecoveryLimits::default().probe_timeout, Seconds::new(30));
        let rounds = HeadOfStationLimits::default();
        assert_eq!(rounds.reason_bytes, Limit::at(2_000));
        assert_eq!(rounds.audit_target_bytes, Limit::at(500));
        assert_eq!(SecretLimits::default().min_matched_bytes, 6);
    }

    #[test]
    fn tables_read_a_few_keys_over_their_defaults() {
        let limits = read(
            "[scheduler]\ncheck_in = 0.2\n\
             [publisher]\nqueue = 1\n[secrets]\nmax_lease_ttl = \"none\"\n\
             [providers.pty]\nscrollback = 10\n",
        )
        .unwrap();
        assert_eq!(
            limits.scheduler.check_in,
            Seconds(std::time::Duration::from_millis(200))
        );
        assert_eq!(
            limits.scheduler.interval,
            SchedulerLimits::default().interval
        );
        assert_eq!(limits.publisher.queue, 1);
        assert_eq!(limits.secrets.max_lease_ttl, TimeLimit::UNBOUNDED);
        assert_eq!(limits.providers["pty"]["scrollback"], 10);
        limits.validate().unwrap();
        assert!(read("[scheduler]\nunknown = 1\n").is_err());
        let zero = read("[publisher]\nqueue = 0\n").unwrap();
        assert!(zero.validate().unwrap_err().contains("publisher.queue"));
        let zero = read("[runner]\nreplay_batch = 0\n").unwrap();
        assert!(zero.validate().unwrap_err().contains("runner.replay_batch"));
    }
}
