//! Renews a sealed Claude Code or Codex login through that harness's account provider.
//!
//! The providers know how their harness renews a login with no model turn (Claude Code's `auth
//! status` inside its window, Codex's token refresh over its app server), which variables the
//! harness reads for a credential and so are removed while it does, and when a login is lost; AI6
//! keeps no command for them. A login of another harness is still renewed by its account's
//! `renew_command`, and every sign-out is its `logout_command`.
//!
//! The renewal round hands the renewer a throwaway home that already holds the login's files.
//! The provider looks at the login there as it is and lets the harness rewrite it in place; the
//! round reads the files back and decides from them, as it does after a command.

use std::path::Path;
use std::time::Duration;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::{AccountGrant, AccountProvider, Credentials, ProcessSpawner, RenewRequest};
use coqui_acp_claude::{AccountSettings as ClaudeSettings, ClaudeAccounts};
use coqui_acp_codex::{AccountSettings as CodexSettings, CodexAccounts};
use coqui_acp_harness::LocalSpawner;

use crate::directorates::q::logins::{CommandRenewer, LoginRenewer, RenewEnd};
use crate::directorates::q::{Account, CLAUDE_LOGIN_FILE, CODEX_LOGIN_FILE};

/// The harness a login belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Harness {
    Claude,
    Codex,
}

impl Harness {
    /// The harness whose own login file `account` keeps, and that file, when it keeps one.
    fn of(account: &Account) -> Option<(Self, &'static str)> {
        let files = account.login_files();
        if files.iter().any(|file| file == CLAUDE_LOGIN_FILE) {
            Some((Self::Claude, CLAUDE_LOGIN_FILE))
        } else if files.iter().any(|file| file == CODEX_LOGIN_FILE) {
            Some((Self::Codex, CODEX_LOGIN_FILE))
        } else {
            None
        }
    }

    /// The kind of account the provider keeps this login as, and the credential key holding its file.
    fn kind(self) -> (&'static str, &'static str) {
        match self {
            Self::Claude => (
                coqui_acp_claude::accounts::LOGIN,
                coqui_acp_claude::accounts::FILE_KEY,
            ),
            Self::Codex => (
                coqui_acp_codex::accounts::CHATGPT,
                coqui_acp_codex::accounts::FILE_KEY,
            ),
        }
    }
}

/// Renews the logins of Claude Code and Codex through their account providers, and every other
/// login, and every sign-out, by the account's own commands.
#[derive(Debug, Clone, Default)]
pub struct ProviderRenewer {
    claude: Option<String>,
    codex: Option<String>,
}

impl ProviderRenewer {
    /// The providers' own programs: `claude` and `codex`, found on the `PATH`.
    pub const DEFAULT: Self = Self {
        claude: None,
        codex: None,
    };

    /// Renews a Claude Code login with the program `executable` instead of `claude`.
    #[must_use]
    pub fn claude_program(mut self, executable: impl Into<String>) -> Self {
        self.claude = Some(executable.into());
        self
    }

    /// Renews a Codex login with the program `executable` instead of `codex`.
    #[must_use]
    pub fn codex_program(mut self, executable: impl Into<String>) -> Self {
        self.codex = Some(executable.into());
        self
    }

    /// Asks `harness`'s account provider to renew the login `file` of `home`. The grant carries
    /// the file's text, which the provider compares with what the directory holds.
    fn renew_through(
        &self,
        harness: Harness,
        file: &str,
        account: &str,
        home: &Path,
        timeout: Duration,
    ) -> Result<(), ProviderError> {
        let bad = |why: &str| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why);
        let path = home.join(file);
        let dir = path
            .parent()
            .ok_or_else(|| bad("a login file is in a directory"))?;
        let text = std::fs::read_to_string(&path).map_err(|error| {
            ProviderError::new(
                Status::GUATIAO_ERR_NOT_FOUND,
                &format!("the login file could not be read: {}", error.kind()),
            )
        })?;
        let (kind, key) = harness.kind();
        let credentials: Credentials = [(key, text.as_str())].into_iter().collect();
        let grant = AccountGrant::new(account, kind)
            .credentials(credentials)
            .state_dir(dir.to_string_lossy());
        let spawner = LocalSpawner::new().files(dir).into_object();
        let limit = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let request = RenewRequest::new(grant);
        // Renewed or not, the files decide: a login the harness left as it was is `Unchanged`
        // to the round, and one it lost is a refusal here.
        match harness {
            Harness::Claude => {
                let settings = ClaudeSettings {
                    executable: self.claude.clone().unwrap_or_else(|| "claude".to_owned()),
                    command_timeout_ms: limit,
                    ..ClaudeSettings::default()
                };
                ClaudeAccounts::new(settings).renew(request, spawner)?;
            }
            Harness::Codex => {
                let settings = CodexSettings {
                    executable: self.codex.clone().unwrap_or_else(|| "codex".to_owned()),
                    command_timeout_ms: limit,
                    ..CodexSettings::default()
                };
                CodexAccounts::new(settings).renew(request, spawner)?;
            }
        }
        Ok(())
    }
}

impl LoginRenewer for ProviderRenewer {
    fn can_renew(&self, account: &Account) -> bool {
        Harness::of(account).is_some() || CommandRenewer.can_renew(account)
    }

    fn renew(&self, account: &Account, home: &Path, timeout: Duration) -> RenewEnd {
        let Some((harness, file)) = Harness::of(account) else {
            return CommandRenewer.renew(account, home, timeout);
        };
        match self.renew_through(harness, file, account.name(), home, timeout) {
            Ok(()) => RenewEnd::Success,
            Err(error) => {
                RenewEnd::NotRun(format!("the account provider refused: {}", reason(&error)))
            }
        }
    }

    fn sign_out(&self, account: &Account, home: &Path, timeout: Duration) -> RenewEnd {
        CommandRenewer.sign_out(account, home, timeout)
    }
}

/// The provider's own words for why it refused, which hold no credential.
fn reason(error: &ProviderError) -> String {
    let message = error.message();
    if message.is_empty() {
        format!("status {:?}", error.status)
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::q::AccountKind;

    fn login(files: &[&str]) -> Account {
        Account::new("shared", AccountKind::HarnessLogin).files(files.iter().copied())
    }

    fn fake() -> String {
        let tests = std::env::current_exe().expect("the test knows where it is");
        let profile = tests
            .parent()
            .and_then(Path::parent)
            .expect("a test binary is in target/<profile>/deps");
        let program = profile.join("examples").join(format!(
            "fake_claude_status{}",
            std::env::consts::EXE_SUFFIX
        ));
        assert!(
            program.exists(),
            "build the stand-in first: cargo build -p ai6 --example fake_claude_status"
        );
        program.to_string_lossy().into_owned()
    }

    fn now_ms() -> u64 {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970");
        u64::try_from(since.as_millis()).expect("fits")
    }

    fn claude_login(expires_ms: u64, refresh: &str) -> String {
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"access-old","refreshToken":"{refresh}","expiresAt":{expires_ms},"scopes":["user:inference","user:profile"]}}}}"#
        )
    }

    fn home_with(text: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("a home");
        let dir = home.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join(".credentials.json"), text).expect("a login");
        home
    }

    fn stored(home: &Path) -> String {
        std::fs::read_to_string(home.join(CLAUDE_LOGIN_FILE)).expect("the login stays")
    }

    #[test]
    fn the_login_files_the_pools_name_are_the_providers_own() {
        assert_eq!(
            CLAUDE_LOGIN_FILE,
            format!(".claude/{}", coqui_acp_claude::accounts::CREDENTIALS_FILE)
        );
        assert_eq!(
            CODEX_LOGIN_FILE,
            format!(".codex/{}", coqui_acp_codex::accounts::AUTH_FILE)
        );
    }

    #[test]
    fn a_claude_or_codex_login_renews_with_no_command_and_any_other_needs_one() {
        let renewer = ProviderRenewer::DEFAULT;
        assert!(renewer.can_renew(&login(&[CLAUDE_LOGIN_FILE])));
        assert!(renewer.can_renew(&login(&[CODEX_LOGIN_FILE])));
        assert!(!renewer.can_renew(&login(&[".grok/auth.json"])));
        assert!(renewer.can_renew(&login(&[".grok/auth.json"]).renew_command(["grok", "models"])));
    }

    #[test]
    fn a_login_near_its_end_is_renewed_by_the_provider_and_the_new_copy_is_in_the_home() {
        let home = home_with(&claude_login(now_ms() + 60_000, "refresh-old"));
        let renewer = ProviderRenewer::DEFAULT.claude_program(fake());
        let end = renewer.renew(
            &login(&[CLAUDE_LOGIN_FILE]),
            home.path(),
            Duration::from_secs(30),
        );
        assert_eq!(end, RenewEnd::Success);
        let after = stored(home.path());
        assert!(after.contains("refresh-rotated"), "{}", after.len());
        assert!(!after.contains("refresh-old"));
    }

    #[test]
    fn a_login_far_from_its_end_is_left_as_it_is() {
        let text = claude_login(now_ms() + 6 * 3_600_000, "refresh-old");
        let home = home_with(&text);
        let renewer = ProviderRenewer::DEFAULT.claude_program(fake());
        let end = renewer.renew(
            &login(&[CLAUDE_LOGIN_FILE]),
            home.path(),
            Duration::from_secs(30),
        );
        assert_eq!(end, RenewEnd::Success);
        assert_eq!(stored(home.path()), text);
    }

    #[test]
    fn a_login_the_provider_finds_blank_is_not_renewed_and_the_refusal_names_no_credential() {
        let home = home_with(r#"{"claudeAiOauth":{"accessToken":"","refreshToken":""}}"#);
        let renewer = ProviderRenewer::DEFAULT.claude_program(fake());
        let end = renewer.renew(
            &login(&[CLAUDE_LOGIN_FILE]),
            home.path(),
            Duration::from_secs(30),
        );
        let RenewEnd::NotRun(why) = end else {
            panic!("a blank login is not renewed");
        };
        assert!(why.starts_with("the account provider refused"), "{why}");
        assert!(!why.contains("refresh-old"));
    }

    #[test]
    fn a_login_with_no_file_in_the_home_is_not_run() {
        let home = tempfile::tempdir().expect("a home");
        let end = ProviderRenewer::DEFAULT.renew(
            &login(&[CLAUDE_LOGIN_FILE]),
            home.path(),
            Duration::from_secs(5),
        );
        assert!(matches!(end, RenewEnd::NotRun(_)), "{end:?}");
    }
}
