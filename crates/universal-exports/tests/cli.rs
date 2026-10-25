use std::process::Command;

#[test]
fn help_documents_the_credential_environment_variable() {
    let output = Command::new(env!("CARGO_BIN_EXE_universal-exports"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("AI6_EXPORTS_STATION_TOKEN"));
    assert!(help.contains("AI6_EXPORTS_PUBLIC_ORIGIN"));
    assert!(help.contains("AI6_EXPORTS_PORT"));
    assert!(help.contains("AI6_EXPORTS_AUTHENTICATOR"));
    assert!(help.contains("AI6_EXPORTS_AUTHENTICATOR_CONFIG"));
    assert!(help.contains("AI6_EXPORTS_DIRECTORY"));
    assert!(help.contains("AI6_EXPORTS_DIRECTORY_CONFIG"));
    assert!(help.contains("AI6_EXPORTS_ACCESS_POLICY"));
    assert!(help.contains("AI6_EXPORTS_AUTHORIZER_CONFIG"));
    assert!(help.contains("AI6_EXPORTS_LOGIN_TRIES_PER_ADDRESS"));
    assert!(help.contains("AI6_EXPORTS_LOGIN_TRIES_PER_NAME"));
    assert!(help.contains("AI6_EXPORTS_CLIENT_ADDRESS_HEADER"));
}

#[test]
fn missing_credential_fails_without_prompting_or_echoing_a_value() {
    let output = Command::new(env!("CARGO_BIN_EXE_universal-exports"))
        .args(["--public-origin", "https://localhost"])
        .env_remove("AI6_EXPORTS_STATION_TOKEN")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("AI6_EXPORTS_STATION_TOKEN must be set"));
}
