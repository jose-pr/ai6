//! An Agent's quota tool: ask what its own account has left.
//!
//! `check_quota` exists for every Run whose Station opened its [`QuotaDesk`]. It reads the pool
//! account the Run holds from its provider now, unless a reading stands from a moment ago, and
//! answers its windows. It names no account and carries no credential, and it starts no work.

use rmcp::tool_router;

use super::CableTools;
use crate::station::server::{DaemonState, usage_reads};

/// What a Run's Agent may ask of its own account's quota.
#[derive(Clone)]
pub(in crate::station::server) struct QuotaDesk {
    state: DaemonState,
}

impl QuotaDesk {
    /// The desk of the Runs of `state`'s Station.
    pub(in crate::station::server) fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// The Run `mission_id`'s account, read for its Agent ([`usage_reads::for_run`]). Blocks.
    fn check(&self, mission_id: &str) -> Result<serde_json::Value, String> {
        usage_reads::for_run(&self.state, mission_id)
    }
}

#[tool_router(router = quota_router, vis = "pub(super)")]
impl CableTools {
    /// Ask what your own account has left.
    #[rmcp::tool(
        description = "Check how much of your own provider quota is left: each window of the account this Run draws on, with how much is used, when it resets and the models it covers, and whether the Station read it just now or a moment ago. Use it before a long step, or when a limit looks near; it is cheap and changes nothing. A Run that draws on no pool account is told so."
    )]
    async fn check_quota(&self) -> String {
        let Some(desk) = self.quota_desk.get().cloned() else {
            return "The quota tool is unavailable to this Run".to_owned();
        };
        let Ok(principal) = self.principal() else {
            return "Quota identity is unavailable".to_owned();
        };
        let Some(mission) = principal.mission_id().map(str::to_owned) else {
            return "The quota tool requires a bound Run".to_owned();
        };
        match tokio::task::spawn_blocking(move || desk.check(&mission)).await {
            Ok(Ok(answer)) => answer.to_string(),
            Ok(Err(reason)) => {
                serde_json::json!({ "status": "unavailable", "reason": reason }).to_string()
            }
            Err(_) => "the quota check failed".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use coqui_acp_api::{Bucket, Refill, UsageReport};
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ContentBlock};
    use rmcp::service::{RoleClient, RunningService};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    use super::*;
    use crate::directorates::m::identity::RunId;
    use crate::directorates::medical::exposure::ExposureGate;
    use crate::directorates::registry::accounts::{AccountProfile, AccountRef};
    use crate::directorates::registry::cables::CablePrincipal;
    use crate::station::server::accounts::testing::hold;
    use crate::station::server::cables_mcp::Endpoint;
    use crate::station::server::tests::limited_state;
    use crate::station::server::usage_reads::Read;

    async fn connect(endpoint: &Endpoint) -> RunningService<RoleClient, ()> {
        ().serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint.url().to_owned())
                .auth_header(endpoint.token().to_owned()),
        ))
        .await
        .unwrap()
    }

    async fn names(client: &RunningService<RoleClient, ()>) -> Vec<String> {
        client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    async fn check(client: &RunningService<RoleClient, ()>) -> String {
        let response = client
            .call_tool(CallToolRequestParams::new("check_quota"))
            .await
            .unwrap();
        let [ContentBlock::Text(text)] = response.content.as_slice() else {
            panic!("the tool answers one text block");
        };
        text.text.clone()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_quota_tool_exists_once_opened_and_reads_the_runs_own_account_once() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now) = limited_state(
            root.path(),
            "",
            "[agent.007]\nhandler = \"mock-limit\"\n",
            1_000_000,
        );
        let account = AccountRef::new("pool", "secret-name");
        let profile = AccountProfile {
            handler: "mock-limit".to_owned(),
            kind: "claude-oauth-token".to_owned(),
            operations: vec!["G".to_owned()],
            priority: 0,
            max_concurrent: None,
            labels: BTreeMap::new(),
            sealed_login: false,
        };
        state
            .intake
            .declare_account(&account, &state.station_id, &profile)
            .unwrap();
        hold(&state, "mission-1", account);
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        state.usage_reads.read_with(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            let mut bucket = Bucket::new("five_hour");
            bucket.limit = Some(1000);
            bucket.used = Some(250);
            bucket.refill = Refill::FixedWindow {
                length: None,
                resets_at: Some(1_005_000),
            };
            let mut report = UsageReport::new();
            report.buckets = vec![bucket];
            Read::Report(Box::new(report))
        });

        let identity = CablePrincipal::try_new(
            "007".to_owned(),
            "G".to_owned(),
            None,
            state.station_id.clone(),
            None,
            Some("mission-1".to_owned()),
        )
        .unwrap();
        let mut endpoint = Endpoint::start_with_exposure(
            state.cables.clone(),
            identity,
            state.intake.clone(),
            false,
            ExposureGate::default(),
        )
        .await
        .unwrap();
        endpoint
            .bind_run(Some(&RunId::try_new("run-1").unwrap()))
            .unwrap();

        let before = connect(&endpoint).await;
        assert!(
            !names(&before)
                .await
                .iter()
                .any(|name| name == "check_quota")
        );
        before.cancel().await.unwrap();

        endpoint.open_quota_tools(QuotaDesk::new(state.clone()));
        let client = connect(&endpoint).await;
        assert!(
            names(&client)
                .await
                .iter()
                .any(|name| name == "check_quota")
        );
        let first: serde_json::Value = serde_json::from_str(&check(&client).await).unwrap();
        let second: serde_json::Value = serde_json::from_str(&check(&client).await).unwrap();
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "the second ask is the same reading"
        );
        assert_eq!(first["pool"], "pool");
        assert_eq!(first["windows"][0]["name"], "five_hour");
        assert_eq!(first["windows"][0]["used_permille"], 250);
        assert_eq!(first["read"], "read");
        assert_eq!(second["read"], "reading from 0s ago");
        assert!(
            !first.to_string().contains("secret-name"),
            "the account is not named to the Agent"
        );
        client.cancel().await.unwrap();
    }
}
