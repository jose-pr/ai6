// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The HTTPS trust of an AI6 client: the public roots and a deployment's own authorities.

use std::path::Path;

use ureq::tls::{Certificate, TlsConfig};

/// The public roots and `certificates` (DER). ureq takes either its built-in roots or a list, so
/// the list repeats the same roots as certificates.
#[must_use]
pub fn trusting(certificates: &[Vec<u8>]) -> TlsConfig {
    let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|root| Certificate::from_der(root.as_ref()))
        .chain(
            certificates
                .iter()
                .map(|der| Certificate::from_der(der).to_owned()),
        );
    TlsConfig::builder().root_certs(roots.into()).build()
}

/// The public roots and the authorities in the PEM file `ca_file`.
///
/// # Errors
///
/// A message naming `ca_file` when the path is relative (Stations share the records that name
/// it) or the file cannot be read as certificates.
pub fn trusting_file(ca_file: &Path) -> Result<TlsConfig, String> {
    if !ca_file.is_absolute() {
        return Err(format!(
            "ca_file must be an absolute path, since Stations share this record: {}",
            ca_file.display()
        ));
    }
    let added = crate::bootstrap::read_ca_file(ca_file).map_err(|e| format!("ca_file: {e}"))?;
    Ok(trusting(&added))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ca_file_must_be_absolute_and_readable() {
        let relative = trusting_file(Path::new("ca.pem")).unwrap_err();
        assert!(relative.contains("absolute"), "{relative}");
        let missing = std::env::temp_dir().join("ai6-tls-no-such-ca.pem");
        let error = trusting_file(&missing).unwrap_err();
        assert!(error.starts_with("ca_file: "), "{error}");
    }

    #[test]
    fn no_added_authority_still_trusts_the_public_roots() {
        let config = trusting(&[]);
        assert!(matches!(
            config.root_certs(),
            ureq::tls::RootCerts::Specific(roots) if !roots.is_empty()
        ));
    }
}
