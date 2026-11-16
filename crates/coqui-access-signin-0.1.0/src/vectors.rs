//! The vectors every consumer of the sign-in policy runs, as JSON text.
//!
//! [`POLICY`] holds cases `{ name, config, context, expect }` for any implementation of the
//! policy; [`FLOWS`] holds scripted sign-ins for a consumer's own endpoint. Their shape is in
//! the narrative site's sign-in page.

/// Cases for the policy: a rule set, a context, and the methods and completeness expected.
pub const POLICY: &str = include_str!("../vectors/policy.json");

/// Scripted sign-ins with stand-in authenticators, for a consumer's real endpoint.
pub const FLOWS: &str = include_str!("../vectors/flows.json");
