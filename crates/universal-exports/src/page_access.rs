//! Who is asking for a page. Every request under the pages' base names its person by the
//! gateway's page authenticator (a request-taking coqui-access provider: `mtls` on the gateway's
//! own TLS, `proxy` behind a front door that verified a certificate or signed someone in), else by
//! the page cookie, which a sign-in in a browser sets for the pages' path and which names that
//! sign-in. None of these is read by any `/v1` route: there only a bearer token counts.
//!
//! The page cookie is `SameSite=None`: a page runs in a sandboxed frame, an opaque origin, and
//! Chrome and Edge send a `Lax` or `Strict` cookie, or a `Partitioned` one, on none of its own
//! requests (its scripts, styles, images and credentialed fetches), only on the frame and the
//! page's own document. A cookie that goes along on another site's requests too is why every
//! page file and feed also needs the view token its frame names.

use std::net::SocketAddr;

use ai6::directorates::m::access::{Factor, SignInContext, purposes};
use ai6::{Attempt, Lookup, Person, Verdict};
use axum::Extension;
use axum::extract::ConnectInfo;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};

use crate::tls::ClientCertificates;
use crate::{Gateway, constant_time_eq, named_cookie, person_by_id};

/// The cookie that names a sign-in to the page routes.
pub(crate) const PAGE_COOKIE: &str = "ai6_pages";

/// Why a request for a page names nobody.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoViewer {
    /// No credential came, or one that names nobody: the answer is an unknown page's.
    Nobody,
    /// The page cookie names a sign-in that has ended.
    SignedOut,
    /// The authenticator, the directory or the Registry could not be asked.
    Unavailable,
}

/// The connection facts a page request carries: the client's address and the certificates it
/// proved on the gateway's own TLS.
pub(crate) type Connection = (
    Option<Extension<ConnectInfo<SocketAddr>>>,
    Option<Extension<ClientCertificates>>,
);

/// The person asking for a page: the page authenticator's answer when it has one, else the page
/// cookie's sign-in.
pub(crate) async fn viewer(
    gateway: &Gateway,
    headers: &HeaderMap,
    (connection, certificates): Connection,
) -> Result<Person, NoViewer> {
    let providers = gateway.access.as_ref().ok_or(NoViewer::Unavailable)?;
    if let Some(authenticator) = providers.page_authenticator.clone() {
        let address = connection
            .as_ref()
            .map(|Extension(ConnectInfo(address))| *address);
        let certificates = certificates.map(|Extension(ClientCertificates(chain))| chain);
        // A certificate the gateway's own TLS proved is `certificate`; anything the request
        // carries for a front door to have vouched for is `proxy`.
        let method = if certificates.as_ref().is_some_and(|chain| !chain.is_empty()) {
            "certificate"
        } else {
            "proxy"
        };
        let attempt = Attempt {
            headers: headers
                .iter()
                .filter(|(name, _)| *name != COOKIE)
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                })
                .collect(),
            peer: connection.map(|Extension(ConnectInfo(address))| address.ip().to_string()),
            secure: Some(certificates.is_some()),
            certificates: certificates
                .filter(|chain| !chain.is_empty())
                .map(|chain| chain.into_iter().map(Into::into).collect()),
            ..Attempt::default()
        };
        let verdict = tokio::task::spawn_blocking(move || authenticator.authenticate(attempt))
            .await
            .map_err(|_| NoViewer::Unavailable)?
            .map_err(|_| NoViewer::Unavailable)?;
        match verdict {
            Verdict::Valid(lookup) => {
                let person = found(gateway, lookup).await?;
                let step = Factor {
                    method: method.to_owned(),
                    provider: providers.page_authenticator_id.clone(),
                    person: Some(person.id.clone()),
                };
                let context = SignInContext {
                    done: vec![step],
                    person: Some(person.clone()),
                    ..gateway.sign_in_context(
                        purposes::SIGN_IN,
                        headers,
                        address,
                        crate::sign_in::PAGES,
                    )
                };
                return match gateway.decide_sign_in(providers, context).await {
                    Some(decision) if decision.complete => Ok(person),
                    _ => Err(NoViewer::Nobody),
                };
            }
            Verdict::Unknown => {}
            _ => return Err(NoViewer::Nobody),
        }
    }
    let Some(value) = page_cookie(headers) else {
        return Err(NoViewer::Nobody);
    };
    let sign_in = signed_in(gateway, value).ok_or(NoViewer::Nobody)?;
    let registry = gateway.registry.clone().ok_or(NoViewer::Unavailable)?;
    let record = tokio::task::spawn_blocking(move || registry.login_by_id(&sign_in))
        .await
        .map_err(|_| NoViewer::Unavailable)?
        .map_err(|_| NoViewer::Unavailable)?
        .ok_or(NoViewer::SignedOut)?;
    person_by_id(gateway, record.person_id().to_owned())
        .await
        .map_err(|error| match error.status {
            axum::http::StatusCode::UNAUTHORIZED => NoViewer::SignedOut,
            _ => NoViewer::Unavailable,
        })
}

/// The person the directory finds for `lookup`.
async fn found(gateway: &Gateway, lookup: Lookup) -> Result<Person, NoViewer> {
    let directory = gateway
        .access
        .as_ref()
        .ok_or(NoViewer::Unavailable)?
        .directory
        .clone();
    tokio::task::spawn_blocking(move || directory.lookup(lookup))
        .await
        .map_err(|_| NoViewer::Unavailable)?
        .map_err(|_| NoViewer::Unavailable)?
        .ok_or(NoViewer::Nobody)
}

/// The value of the page cookie a request carries; a client speaking HTTP/2 may split `Cookie`
/// over several headers.
fn page_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| named_cookie(cookie, PAGE_COOKIE))
}

/// The keyed digest that binds a page cookie to the sign-in it names.
fn mark(gateway: &Gateway, sign_in: &str) -> Option<String> {
    let store = gateway.pages.as_ref()?;
    Some(store.keyed_text(&["page-cookie", sign_in]))
}

/// The sign-in a page cookie's value names, when its mark is this gateway's.
fn signed_in(gateway: &Gateway, value: &str) -> Option<String> {
    let (sign_in, presented) = value.rsplit_once('.')?;
    let expected = mark(gateway, sign_in)?;
    constant_time_eq(expected.as_bytes(), presented.as_bytes()).then(|| sign_in.to_owned())
}

/// The path the page cookie is sent to: the pages' base on the API's own origin; `None` when
/// pages have an origin of their own, where a sign-in at the API cannot set it.
fn cookie_path(gateway: &Gateway) -> Option<String> {
    let config = gateway.pages_config.as_ref()?;
    if config.origin.is_some() {
        return None;
    }
    Some(pages_path(gateway))
}

/// The path of the pages' base.
fn pages_path(gateway: &Gateway) -> String {
    let base = crate::moneypenny::pages::pages_base(gateway);
    let after_scheme = base.find("://").map_or(0, |at| at + 3);
    let path = base[after_scheme..]
        .find('/')
        .map_or("", |at| &base[after_scheme + at..]);
    format!("{path}/")
}

/// The `Set-Cookie` that names `sign_in` to the page routes for `seconds`; `None` where pages
/// get no cookie.
pub(crate) fn set_page_cookie(
    gateway: &Gateway,
    sign_in: &str,
    seconds: f64,
) -> Option<HeaderValue> {
    let path = cookie_path(gateway)?;
    let value = format!("{sign_in}.{}", mark(gateway, sign_in)?);
    let max_age = seconds.ceil().clamp(0.0, f64::from(u32::MAX)) as u64;
    HeaderValue::from_str(&format!(
        "{PAGE_COOKIE}={value}; Path={path}; HttpOnly; Secure; SameSite=None; Max-Age={max_age}"
    ))
    .ok()
}

/// The `Set-Cookie` a hand-off answers with, from the pages' own routes: it can set the cookie
/// where a sign-in at the API cannot, on an origin of the pages' own.
pub(crate) fn handed_off_cookie(
    gateway: &Gateway,
    sign_in: &str,
    seconds: f64,
) -> Option<HeaderValue> {
    let value = format!("{sign_in}.{}", mark(gateway, sign_in)?);
    let max_age = seconds.ceil().clamp(0.0, f64::from(u32::MAX)) as u64;
    HeaderValue::from_str(&format!(
        "{PAGE_COOKIE}={value}; Path={}; HttpOnly; Secure; SameSite=None; Max-Age={max_age}",
        pages_path(gateway)
    ))
    .ok()
}

/// The `Set-Cookie` that drops the page cookie; `None` where pages get no cookie.
pub(crate) fn ended_page_cookie(gateway: &Gateway) -> Option<HeaderValue> {
    let path = cookie_path(gateway)?;
    HeaderValue::from_str(&format!(
        "{PAGE_COOKIE}=; Path={path}; HttpOnly; Secure; SameSite=None; Max-Age=0"
    ))
    .ok()
}

/// Adds `cookie` to `headers` when there is one.
pub(crate) fn append(headers: &mut HeaderMap, cookie: Option<HeaderValue>) {
    if let Some(cookie) = cookie {
        headers.append(SET_COOKIE, cookie);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use ai6::directorates::m::access::{Authenticator, Directory};
    use ai6::guatiao::library::ProviderError;
    use ai6::{Attempt, FileRegistry, Lookup, Person, Verdict};
    use coqui_access_signin::{RuleDef, SignInRules};

    use super::*;
    use crate::{AccessProviders, GatewayConfig, GatewayLimits};

    /// Names Alice for any request that carries `x-viewer`, as a front door that verified her would.
    struct Front;

    impl Authenticator for Front {
        fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
            let vouched = attempt
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("x-viewer"));
            Ok(if vouched {
                Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: "alice".to_owned(),
                    ..Default::default()
                })
            } else {
                Verdict::Unknown
            })
        }
    }

    impl Directory for Front {
        fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
            Ok((lookup.value == "alice").then(|| Person {
                id: "alice-id".to_owned(),
                name: "alice".to_owned(),
                ..Person::default()
            }))
        }
    }

    fn gateway(rule: RuleDef) -> Gateway {
        let directory = tempfile::tempdir().unwrap();
        let access = AccessProviders::new(
            Arc::new(Front),
            Arc::new(Front),
            Arc::new(ai6::directorates::m::access::RoleAuthorizer::default()),
        )
        .with_page_authenticator(Arc::new(Front))
        .with_page_authenticator_id("front")
        .with_sign_in_policy(Arc::new(SignInRules::builder().rule(rule).build().unwrap()));
        Gateway::with_access(
            GatewayConfig::new(
                "https://localhost",
                "m-token-that-is-long-enough-for-testing",
                GatewayLimits::default(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(FileRegistry::open(directory.path())),
            access,
        )
    }

    async fn viewer_of(gateway: &Gateway) -> Result<Person, NoViewer> {
        let mut headers = HeaderMap::new();
        headers.insert("x-viewer", HeaderValue::from_static("alice"));
        viewer(gateway, &headers, (None, None)).await
    }

    #[tokio::test]
    async fn a_viewer_is_decided_as_the_method_the_front_door_gives_it_for_the_pages() {
        let rule = |chain: &[&str]| RuleDef::new([chain.to_vec()]);
        let allowed = gateway(rule(&["proxy"]).services(["pages"]));
        assert_eq!(viewer_of(&allowed).await.unwrap().id, "alice-id");
        for refused in [
            rule(&["certificate"]),
            rule(&["proxy"]).services(["unex"]),
            rule(&["proxy"]).providers(["another"]),
            rule(&["proxy", "totp"]),
        ] {
            assert_eq!(viewer_of(&gateway(refused)).await, Err(NoViewer::Nobody));
        }
        let named = gateway(rule(&["proxy"]).providers(["front"]));
        assert!(
            viewer_of(&named).await.is_ok(),
            "the authenticator is listed by its ID"
        );
    }
}
