//! OpenCode (`opencode acp`): its [`HarnessDef`](crate::stdio::accounts::HarnessDef) and sign-in, the table of its access
//! settings, the two providers a host registers for it, and its sessions: reading one out of its store
//! (`opencode export`) and writing one into it (`opencode import`).

pub mod accounts;
pub mod history;
pub mod import;
mod provider;

pub use accounts::{DEF, STDIO, TARGET};
pub use provider::{OpenCodeAccountSettings, OpenCodeAccounts, OpenCodeProvider, OpenCodeSettings};
