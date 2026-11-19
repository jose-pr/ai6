//! What a session may touch and when it asks, said once for every translator.
//!
//! The three settings of [`Access`] mean the same for every harness: how much of the machine a command may reach
//! ([`Sandbox`]), whether it may use the network, and what is asked first ([`Approvals`]). Each translator turns them into
//! its command line's own words and says what that command line cannot express. A client gives a session its own through
//! the request's `_meta` under [`ACCESS_META`], which a translator takes only when its host allows it.

use alloc::format;
use alloc::string::{String, ToString};
use serde_json::{Map, Value, json};

use coqui_acp_api::guatiao::{FromValue, Schema, ToValue};

mod words;

pub use words::{Part, Target, Translation};

/// The `_meta` key of a session request that holds an [`Access`], as JSON: `{"sandbox": "none", "network": true,
/// "approvals": "ask"}`; a part left out is [`Access::STANDARD`]'s.
pub const ACCESS_META: &str = "dev.boriken.coqui.acp/access";

/// How much of the machine the commands a session runs may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ToValue, FromValue, Schema)]
#[map(crate = ::coqui_acp_api::guatiao)]
#[non_exhaustive]
pub enum Sandbox {
    /// Nothing holds them back but the person's own operating system account.
    #[map(rename = "none")]
    Unrestricted,
    /// They read widely and write only in the session's directories.
    #[map(rename = "workspace")]
    Workspace,
    /// They change nothing.
    #[map(rename = "read-only")]
    ReadOnly,
}

impl Sandbox {
    /// Its id, as a profile and `_meta` write it.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Unrestricted => "none",
            Self::Workspace => "workspace",
            Self::ReadOnly => "read-only",
        }
    }

    /// The sandbox an `id` names.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Unrestricted, Self::Workspace, Self::ReadOnly]
            .into_iter()
            .find(|sandbox| sandbox.id() == id)
    }
}

/// What is asked before a call runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ToValue, FromValue, Schema)]
#[map(crate = ::coqui_acp_api::guatiao, rename_all = "snake_case")]
#[non_exhaustive]
pub enum Approvals {
    /// Everything that is not read-only.
    Ask,
    /// Commands and the rest; edits run on their own.
    Edits,
    /// Nothing.
    Never,
}

impl Approvals {
    /// Its id, as a profile and `_meta` write it.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edits => "edits",
            Self::Never => "never",
        }
    }

    /// The approvals an `id` names.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Ask, Self::Edits, Self::Never]
            .into_iter()
            .find(|approvals| approvals.id() == id)
    }
}

/// A session's access: the sandbox, the network and the approvals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ToValue, FromValue, Schema)]
#[map(crate = ::coqui_acp_api::guatiao)]
#[non_exhaustive]
pub struct Access {
    /// How much the commands may reach.
    #[map(default = Sandbox::Unrestricted)]
    pub sandbox: Sandbox,
    /// Whether the commands may use the network.
    #[map(default = true)]
    pub network: bool,
    /// What is asked first.
    #[map(default = Approvals::Ask)]
    pub approvals: Approvals,
}

impl Default for Access {
    fn default() -> Self {
        Self::STANDARD
    }
}

impl Access {
    /// The person's own reach: no sandbox, the network on, and everything that is not read-only asked first.
    pub const STANDARD: Self = Self {
        sandbox: Sandbox::Unrestricted,
        network: true,
        approvals: Approvals::Ask,
    };

    /// The access `value` writes, a part left out being [`Access::STANDARD`]'s.
    ///
    /// # Errors
    ///
    /// The reason, for a value that is no object, names a part this type does not have or gives one a value it does not take.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let Value::Object(map) = value else {
            return Err("access is an object".to_string());
        };
        let mut access = Self::STANDARD;
        for (key, value) in map {
            match (key.as_str(), value) {
                ("sandbox", Value::String(id)) => {
                    access.sandbox = Sandbox::from_id(id).ok_or_else(|| {
                        format!("access.sandbox is none, workspace or read-only, not {id}")
                    })?;
                }
                ("network", Value::Bool(network)) => access.network = *network,
                ("approvals", Value::String(id)) => {
                    access.approvals = Approvals::from_id(id).ok_or_else(|| {
                        format!("access.approvals is ask, edits or never, not {id}")
                    })?;
                }
                (other, _) => {
                    return Err(format!(
                        "access.{other} is not a setting or has the wrong type"
                    ));
                }
            }
        }
        Ok(access)
    }

    /// The access of a request's `_meta`, if it holds one.
    ///
    /// # Errors
    ///
    /// As [`Access::from_json`].
    pub fn from_meta(meta: &Map<String, Value>) -> Result<Option<Self>, String> {
        meta.get(ACCESS_META).map(Self::from_json).transpose()
    }

    /// The access as JSON, the form [`Access::from_json`] reads.
    #[must_use]
    pub fn to_json(self) -> Value {
        json!({"sandbox": self.sandbox.id(), "network": self.network, "approvals": self.approvals.id()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_access_is_no_sandbox_the_network_on_and_everything_asked() {
        assert_eq!(Access::default(), Access::STANDARD);
        assert_eq!(Access::STANDARD.sandbox, Sandbox::Unrestricted);
        assert_eq!(Access::STANDARD.network, Access::default().network);
        assert!(Access::default().network);
        assert_eq!(Access::STANDARD.approvals, Approvals::Ask);
    }

    #[test]
    fn json_round_trips_and_a_part_left_out_is_standard() {
        let access = Access {
            sandbox: Sandbox::ReadOnly,
            network: false,
            approvals: Approvals::Never,
        };
        assert_eq!(Access::from_json(&access.to_json()), Ok(access));
        assert_eq!(
            Access::from_json(&json!({"sandbox": "workspace"})).map(|each| each.network),
            Ok(true)
        );
        assert_eq!(Access::from_json(&json!({})), Ok(Access::STANDARD));
    }

    #[test]
    fn a_wrong_part_is_refused_with_its_name() {
        for bad in [
            json!("none"),
            json!({"sandbox": "danger"}),
            json!({"network": "yes"}),
            json!({"approvals": "auto"}),
            json!({"mode": "plan"}),
        ] {
            assert!(Access::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_request_without_the_key_holds_none() {
        assert_eq!(Access::from_meta(&Map::new()), Ok(None));
        let mut meta = Map::new();
        meta.insert(ACCESS_META.to_string(), json!({"approvals": "edits"}));
        assert_eq!(
            Access::from_meta(&meta).map(|found| found.map(|each| each.approvals)),
            Ok(Some(Approvals::Edits))
        );
    }
}
