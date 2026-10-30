//! How a Station runs: [`StationLimits`], the tables of the `station` record that bound and
//! pace its work, one table per area, and the provider configurations it hands out. Each
//! default is the value a Station applies when the record says nothing; the record applies at
//! restart.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional. A queue is a plain count of at least one, since it is allocated up front.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// Every table of the `station` record, as a Station keeps them while it runs.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    /// `[accounts]`: pool accounts.
    pub accounts: AccountLimits,
    /// `[approvals]`: green ink steps.
    pub approvals: ApprovalLimits,
    /// `[harness]`: a working directory's harness configuration.
    pub harness: HarnessLimits,
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
    /// `[audit]`: audit record details.
    pub audit: AuditLimits,
    /// `[theaters]`: ephemeral Theaters.
    pub theaters: TheaterLimits,
    /// `[theater_tools]`: the `theater_*` tools of remote-hands Sessions.
    pub theater_tools: TheaterToolLimits,
    /// `[head_of_station]`: the Head of Station's rounds.
    pub head_of_station: HeadOfStationLimits,
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
            ("runner.page", self.runner.page as u64),
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulerLimits {
    /// Events of one Session queued for its event pump; past it the oldest is dropped.
    /// Default 256.
    pub event_queue: Limit,
    /// Seconds between scheduler rounds. Default 0.5.
    pub interval: Seconds,
    /// Seconds before a Mission whose start failed is tried again. Default 5.
    pub retry_delay: Seconds,
    /// Seconds between the Station's check-ins with the Registry. Default 15.
    pub check_in: Seconds,
    /// Times a brief is placed before it is filed as failed. Default 3.
    pub intake_attempts: u32,
    /// Bytes of the Agent role catalog the Station publishes. Default 512 KiB.
    pub role_catalog_bytes: Limit,
    /// Seconds the Station waits for each of its loops when it stops. Default 2.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PublisherLimits {
    /// Seconds one request to Universal Exports may take. Default 2.
    pub request_timeout: Seconds,
    /// Lifecycle events queued for publication; past it new ones are dropped. Default 64.
    pub queue: usize,
    /// Bytes of terminal output in one published frame. Default 1 MiB.
    pub terminal_bytes: Limit,
    /// Bytes of terminal output kept for a Session not yet bound to its Mission. Default
    /// 1 MiB.
    pub pre_bind_terminal_bytes: Limit,
    /// Seconds before a failed input or control poll is tried again. Default 0.1.
    pub input_retry_delay: Seconds,
    /// Seconds Universal Exports holds a control poll open; kept under `request_timeout`.
    /// Default 1.
    pub control_wait: Seconds,
    /// Mission controls queued for the scheduler. Default 16.
    pub control_queue: usize,
    /// Times the outcome of a control is reported. Default 3.
    pub control_report_attempts: u32,
}

impl Default for PublisherLimits {
    fn default() -> Self {
        Self {
            request_timeout: Seconds::new(2),
            queue: 64,
            terminal_bytes: Limit::at(MIB),
            pre_bind_terminal_bytes: Limit::at(MIB),
            input_retry_delay: Seconds(std::time::Duration::from_millis(100)),
            control_wait: Seconds::new(1),
            control_queue: 16,
            control_report_attempts: 3,
        }
    }
}

/// `[sources]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceLimits {
    /// Seconds between polls of a Source that names no interval. Default 60.
    pub poll: Seconds,
    /// The shortest interval a Source may ask for, in seconds; a smaller value is looser.
    /// Default 10.
    pub min_poll: Seconds,
    /// Seconds between rounds reporting finished briefs to their Sources. Default 15.
    pub report_every: Seconds,
    /// Seconds between rounds firing Sleepers. Default 20.
    pub sleeper_every: Seconds,
    /// Seconds between rounds sending the outbox of tracker writes. Default 5.
    pub outbox_every: Seconds,
    /// Seconds between rounds reading new comments on the issues Missions are linked to.
    /// Default 60.
    pub reply_every: Seconds,
    /// Outbox entries sent in one round. Default 20.
    pub outbox_batch: Limit,
    /// Sends tried before an outbox entry fails. Default 5.
    pub send_attempts: Limit,
    /// Seconds before a failed send is tried again; each attempt doubles it. Default 30.
    pub retry_backoff: Seconds,
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
        }
    }
}

/// `[transports]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransportLimits {
    /// Seconds before a Theater whose probe failed is probed again. Default 60.
    pub reprobe_after: Seconds,
    /// Seconds between looks over a Theater for its requirements and ambient credentials.
    /// Default 300.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryLimits {
    /// Seconds before the first try to re-attach to a Session; each try doubles it.
    /// Default 5.
    pub first_reattach_delay: Seconds,
    /// The longest wait between tries, in seconds. Default 60.
    pub max_reattach_delay: Seconds,
    /// Tries before the Station gives a Session up as lost. Default 10.
    pub give_up_tries: u32,
    /// Seconds between notes on a Session being recovered. Default 15.
    pub notes_every: Seconds,
}

impl Default for RecoveryLimits {
    fn default() -> Self {
        Self {
            first_reattach_delay: Seconds::new(5),
            max_reattach_delay: Seconds::new(60),
            give_up_tries: 10,
            notes_every: Seconds::new(15),
        }
    }
}

/// `[vetting]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VettingLimits {
    /// Findings kept per verdict. Default 16.
    pub findings: Limit,
    /// Other work a review brief lists for the reviewer. Default 50.
    pub review_work: Limit,
    /// Bytes of a finding's detail. Default 256.
    pub detail_bytes: Limit,
    /// Manipulation phrases an Operation may screen for; past it a brief is held.
    /// Default 256.
    pub phrases: Limit,
    /// Bytes of one such phrase. Default 256.
    pub phrase_bytes: Limit,
    /// Seconds of the rolling window rates are counted over. Default 3,600.
    pub rate_window: Seconds,
    /// Mission proposals Headquarters keeps track of. Default 500.
    pub proposals: Limit,
    /// Seconds a proposal's approval request waits for a decision. Default 3,600.
    pub proposal_request_ttl: Seconds,
    /// Seconds a proposal waits for its brief to settle before it is vetted. Default 30.
    pub proposal_settle: Seconds,
    /// Seconds between sweeps for expired proposal requests. Default 60.
    pub expiry_sweep: Seconds,
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
        }
    }
}

/// `[accounts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccountLimits {
    /// Seconds past a reported reset before a limited account is used again. Default 15.
    pub reset_grace: Seconds,
    /// The use, in thousandths, assumed of an account that reports none. Default 500.
    pub unknown_use_permille: u16,
    /// Seconds a claim on a pool account lasts between renewals. Default 60.
    pub claim_ttl: Seconds,
    /// The longest a limited account is kept out, in seconds, whatever reset time its
    /// provider reports. Default 691,200 (8 days).
    pub limit_horizon: Seconds,
    /// Seconds a pool account's login may take. Default 60.
    pub login_timeout: Seconds,
}

impl Default for AccountLimits {
    fn default() -> Self {
        Self {
            reset_grace: Seconds::new(15),
            unknown_use_permille: 500,
            claim_ttl: Seconds(crate::directorates::registry::store::DEFAULT_LEASE),
            limit_horizon: Seconds::new(8 * DAY),
            login_timeout: Seconds::new(60),
        }
    }
}

/// `[approvals]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalLimits {
    /// Bytes of a green ink step's kind. Default 32.
    pub kind_bytes: Limit,
    /// Bytes of a green ink step's target. Default 256.
    pub target_bytes: Limit,
    /// How deep `sh -c "sh -c ..."` and git aliases are followed when a command is
    /// classified. Default 4.
    pub classify_nesting: usize,
}

impl Default for ApprovalLimits {
    fn default() -> Self {
        Self {
            kind_bytes: Limit::at(32),
            target_bytes: Limit::at(256),
            classify_nesting: 4,
        }
    }
}

/// `[harness]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HarnessLimits {
    /// Bytes of one file of a project's harness configuration. Default 1 MiB.
    pub manifest_file_bytes: Limit,
    /// Bytes of the whole configuration. Default 16 MiB.
    pub manifest_bytes: Limit,
    /// Files in the configuration. Default 4,096.
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

/// `[provision]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvisionLimits {
    /// Seconds the requirements probe may take. Default 60.
    pub probe_timeout: Seconds,
    /// Seconds one provisioning command may take. Default 120.
    pub command_timeout: Seconds,
    /// Bytes of a required program's name. Default 64.
    pub name_bytes: Limit,
    /// Characters of a program's version kept from the probe. Default 200.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecretLimits {
    /// Seconds the leases of a Session's loadout and pool account secrets last. Default 3,600.
    pub lease_ttl: Seconds,
    /// The longest lease, in seconds; a green ink secret's lease lasts as long as its
    /// approval, up to it. Default 3,600.
    pub max_lease_ttl: TimeLimit,
}

impl Default for SecretLimits {
    fn default() -> Self {
        Self {
            lease_ttl: Seconds::new(3_600),
            max_lease_ttl: TimeLimit::secs(3_600),
        }
    }
}

/// `[terminal_output]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalOutputLimits {
    /// Bytes of one line of a JSON harness's output. Default 64 MiB.
    pub json_line_bytes: Limit,
    /// Bytes of one terminal control sequence before it is dropped. Default 64.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GreenInkLimits {
    /// Seconds the harness gate's exchange with the Station may take. Default 30.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetachedLimits {
    /// Seconds between reads of a detached harness's output. Default 0.2.
    pub poll: Seconds,
    /// Seconds one of the wrapper's scripts may take over the transport. Default 30.
    pub script_timeout: Seconds,
    /// Characters of the Session's name in the wrapper's file names. Default 64.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerLimits {
    /// Events one read of a runner returns. Default 256.
    pub page: usize,
    /// The longest a read of a runner waits for a new event, in seconds. Default 30.
    pub max_wait: Seconds,
    /// Seconds an ended Session is kept by its runner. Default 604,800 (a week).
    pub keep_ended: Seconds,
    /// Bytes of a refused request's body a runner reads before it answers. Default 64 KiB.
    pub refused_body_drain_bytes: Limit,
    /// Bytes of one request through a runner's Cable relay. Default 4 MiB.
    pub cable_request_bytes: Limit,
    /// Seconds a runner's Cable relay may take to reach the Station. Default 10.
    pub cable_connect_timeout: Seconds,
    /// Seconds the Station's read of a runner waits for an event. Default 1.
    pub read_wait: Seconds,
    /// Bytes of output an `ai6 runner call` over a transport may return. Default 32 MiB.
    pub call_output_bytes: Limit,
    /// Seconds connecting to a runner may take. Default 5.
    pub connect_timeout: Seconds,
    /// Seconds a request to a runner may take past the longest read. Default 30.
    pub call_margin: Seconds,
    /// Seconds a started runner has to answer. Default 30.
    pub start_wait: Seconds,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            page: 256,
            max_wait: Seconds::new(30),
            keep_ended: Seconds::new(7 * DAY),
            refused_body_drain_bytes: Limit::at(64 * KIB),
            cable_request_bytes: Limit::at(4 * MIB),
            cable_connect_timeout: Seconds::new(10),
            read_wait: Seconds::new(1),
            call_output_bytes: Limit::at(32 * MIB),
            connect_timeout: Seconds::new(5),
            call_margin: Seconds::new(30),
            start_wait: Seconds::new(30),
        }
    }
}

/// `[audit]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditLimits {
    /// Characters of the secret names an audit record lists. Default 240.
    pub names_detail_chars: Limit,
    /// Bytes of a configuration change's audit target. Default 512.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TheaterLimits {
    /// Bytes of an ephemeral Theater's name; a longer one is cut and given a digest.
    /// Default 100.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TheaterToolLimits {
    /// Bytes of a command's output `theater_run` returns. Default 256 KiB.
    pub output_bytes: Limit,
    /// Bytes `theater_read` returns. Default 256 KiB.
    pub read_bytes: Limit,
    /// Seconds a `theater_run` command may take unless it asks for another. Default 120.
    pub timeout: Seconds,
    /// The longest a command may ask for, in seconds. Default 3,600.
    pub max_timeout: TimeLimit,
    /// Seconds the git commands the tools run may take. Default 30.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HeadOfStationLimits {
    /// Changes of a safe house listed in one report. Default 20.
    pub listed_changes: Limit,
    /// Directory entries one disk survey walks. Default 500,000.
    pub survey_entries: Limit,
}

impl Default for HeadOfStationLimits {
    fn default() -> Self {
        Self {
            listed_changes: Limit::at(20),
            survey_entries: Limit::at(500_000),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_read_a_few_keys_over_their_defaults() {
        let limits: StationLimits = toml::from_str(
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
        assert!(toml::from_str::<StationLimits>("[scheduler]\nunknown = 1\n").is_err());
        let zero: StationLimits = toml::from_str("[publisher]\nqueue = 0\n").unwrap();
        assert!(zero.validate().unwrap_err().contains("publisher.queue"));
    }
}
