//! Antigravity (`agy acp`), measured on 1.5.3 from its help text and binary strings only.
//!
//! Its login is kept in `<gemini-dir>/<app-data-dir>/antigravity-oauth-token` and, on Windows, in the Credential Manager
//! entry `gemini:antigravity`. Which one wins, whether `--gemini-dir` and `--app-data-dir` move the token, whether two
//! logins can exist at once, and what the sign-in `agy acp --login` prints were not measured, so the provider lists no
//! kind but `harness-login` and no login method until they are.

use super::HarnessDef;

/// What Antigravity takes as an account.
pub static DEF: HarnessDef = HarnessDef {
    name: "Antigravity",
    program: "agy",
    args: &["acp"],
    kinds: &[],
    variables: &[],
    renew_args: None,
    login_unsupported: "agy signs in with `agy acp --login`, whose prompts were not run: it may write the Windows Credential Manager, which a throwaway profile does not isolate, and where it keeps the login and whether two logins can exist at once are unmeasured",
};
