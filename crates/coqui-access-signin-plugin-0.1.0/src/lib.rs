//! `coqui-access-signin` as a library a host loads: the Guatiao entry point of the
//! `coqui_signin_rules` provider, whose configuration is a `coqui_access_signin::SignInConfig`.
//!
//! ```json
//! { "rule": [
//!     { "networks": ["10.0.0.0/8"], "allow": [["password"], ["certificate"]] },
//!     { "allow": [["certificate"]] } ] }
//! ```
//!
//! Every key is optional but `allow`; no rule means nothing is ever offered.

coqui_access::guatiao::providers!(coqui_access_signin::SignInRules);
