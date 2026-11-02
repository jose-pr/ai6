//! `ca_file` against a real TLS server on loopback, whose certificate chains to an authority no
//! public root list holds. Nothing here reaches a Jira site.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use ai6::{Source, SourcePoll};
use ai6_source_jira::Jira;

/// An authority of its own, as PEM, and a server it vouches for: the server's origin.
fn enterprise_server(body: &'static str) -> (String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                match tls.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ => break,
                }
            }
            if request.ends_with(b"\r\n\r\n") {
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(answer.as_bytes());
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    (origin, ca_pem)
}

fn settings(origin: &str, extra: &str) -> String {
    // SAFETY: only this test binary reads these names, and every test sets the same values.
    unsafe {
        std::env::set_var("AI6_TEST_CA_TOKEN", "token");
        std::env::set_var("AI6_TEST_CA_EMAIL", "bot@example.invalid");
    }
    format!(
        "site = \"{origin}\"\nproject = \"AI6\"\nemail_env = \"AI6_TEST_CA_EMAIL\"\ntoken_env = \"AI6_TEST_CA_TOKEN\"\n{extra}\n"
    )
}

fn toml_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

#[test]
fn a_server_under_an_enterprise_authority_needs_its_ca_file() {
    let (origin, ca) = enterprise_server(include_str!("fixtures/empty_search.json"));
    let dir = tempfile::tempdir().unwrap();
    let trusted = dir.path().join("enterprise.pem");
    std::fs::write(&trusted, ca).unwrap();

    let refused = Jira::from_settings(&settings(&origin, ""))
        .unwrap()
        .poll(SourcePoll { cursor: None })
        .unwrap_err()
        .to_string();
    assert!(refused.contains("request failed"), "{refused}");

    let with = format!("ca_file = \"{}\"", toml_path(&trusted));
    Jira::from_settings(&settings(&origin, &with))
        .unwrap()
        .poll(SourcePoll { cursor: None })
        .unwrap();

    let (_, other) = enterprise_server("");
    let wrong = dir.path().join("other.pem");
    std::fs::write(&wrong, other).unwrap();
    let with = format!("ca_file = \"{}\"", toml_path(&wrong));
    assert!(
        Jira::from_settings(&settings(&origin, &with))
            .unwrap()
            .poll(SourcePoll { cursor: None })
            .is_err(),
        "another authority vouches for nothing here"
    );
}

#[test]
fn a_ca_file_that_trusts_nothing_is_refused_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "not a certificate\n").unwrap();
    let missing = dir.path().join("missing.pem");
    for path in [&empty, &missing] {
        let with = format!("ca_file = \"{}\"", toml_path(path));
        let refused = Jira::from_settings(&settings("https://example.invalid", &with)).unwrap_err();
        assert!(refused.contains("ca_file"), "{refused}");
        assert!(refused.contains(path.to_str().unwrap()), "{refused}");
    }
    let refused = Jira::from_settings(&settings(
        "https://example.invalid",
        "ca_file = \"rel.pem\"",
    ))
    .unwrap_err();
    assert!(refused.contains("absolute"), "{refused}");
}
