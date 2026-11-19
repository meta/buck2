/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsString;
use std::sync::Arc;

use buck2_error::Tier;
use buck2_error::buck2_error;
use buck2_util::process::async_background_command;
use dupe::Dupe;
use rustls_pki_types::CertificateDer;
use tokio::sync::Mutex;

use crate::certs;
use crate::certs::load_certs;

#[derive(Debug, buck2_error::Error)]
#[buck2(environment, tag = NoValidCerts)]
enum InvalidCertsError {
    #[error(
        "Could not find valid root certs. Please check your machine certificate settings. Instructions: https://fburl.com/devcert\nFailure Reason: {0}"
    )]
    SystemCerts(String),
    #[error(
        "Could not find valid client certs. Try refreshing your internal certs or re-login and try again. Instructions: https://fburl.com/devcert\nFailure Reason: {0}"
    )]
    ClientCerts(String),
    #[error(
        "Could not find valid certs for VPNless. Please refresh/renew certs with SKS agent and try again. Instructions: https://fburl.com/devcert"
    )]
    VPNlessCerts,
}

/// Use SKS Agent to check the status of the VPNless cert in the scenario that VPNless is supported.
/// SKS Agent is different in Windows so we need to use the appropriate command for the OS.
async fn is_vpnless_cert_valid() -> bool {
    let sks_agent = if cfg!(target_os = "windows") {
        "sks-agent"
    } else {
        "fb-sks-agent"
    };

    // Post suggests using the following for VPN-less scenario
    // https://fb.workplace.com/groups/382932749004606/permalink/1473311023300101/
    let cmd_result = async_background_command(sks_agent)
        .args(["renew", "--status", "--corp-x509"])
        .output()
        .await;

    match cmd_result {
        Ok(cmd_output) => String::from_utf8_lossy(&cmd_output.stdout).starts_with("true"),
        Err(_) => false,
    }
}

/// Which certificates of a PEM file must be valid for the file to pass.
#[derive(Clone, Copy)]
enum ValidCerts {
    /// A bundle, such as the root certificates: any valid certificate will do.
    Any,
    /// A client certificate file: the leaf comes first, followed by its chain.
    Leaf,
}

/// Check if the provided certs exists and if it is still valid at the current time.
async fn verify(path: &OsString, required: ValidCerts) -> buck2_error::Result<()> {
    let certs = load_certs(path).await?;
    if certs.is_empty() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Environment,
            "Could not find any certs to validate at '{0}'",
            path.to_string_lossy()
        ));
    }

    let is_valid = |bytes: &CertificateDer<'_>| {
        let x509_cert = match x509_parser::parse_x509_certificate(bytes) {
            Ok((_, x509_cert)) => x509_cert,
            Err(_) => return false,
        };

        x509_cert.validity().is_valid()
    };
    let valid = match required {
        ValidCerts::Any => certs.iter().any(is_valid),
        ValidCerts::Leaf => is_valid(&certs[0]),
    };

    if !valid {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Environment,
            "Certificate Expired: expired certs found at '{0}'",
            path.to_string_lossy()
        ));
    }

    Ok(())
}

pub async fn validate_certs() -> buck2_error::Result<()> {
    if cfg!(not(fbcode_build)) {
        return Ok(());
    }

    if certs::supports_vpnless() {
        if is_vpnless_cert_valid().await {
            return Ok(());
        }

        return Err(InvalidCertsError::VPNlessCerts.into());
    } else {
        let err_msg = "Could not find any files that may contain certificates";
        // System certs are unlikely to be invalid, but if it is, it's a bigger issue than invalid client certs so we check it first
        match certs::find_root_ca_certs() {
            Some(root_certs) => {
                if let Err(e) = verify(&root_certs, ValidCerts::Any).await {
                    return Err(InvalidCertsError::SystemCerts(e.to_string()).into());
                }
            }
            None => return Err(InvalidCertsError::SystemCerts(err_msg.to_owned()).into()),
        }

        match certs::find_internal_cert() {
            Some(client_certs) => {
                if let Err(e) = verify(&client_certs, ValidCerts::Leaf).await {
                    return Err(InvalidCertsError::ClientCerts(e.to_string()).into());
                }
            }
            None => return Err(InvalidCertsError::ClientCerts(err_msg.to_owned()).into()),
        }
    }

    Ok(())
}

#[derive(Clone, Dupe)]
pub struct CertState {
    pub state: Arc<Mutex<bool>>,
}

impl CertState {
    pub async fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(validate_certs().await.is_ok())),
        }
    }
}

async fn check_cert_state(cert_state: CertState) -> Option<buck2_error::Error> {
    let mut valid = cert_state.state.lock().await;

    // If previous state is error, then we need to check regardless of the current state
    // since we are expecting users to actively fix the issue and retry
    if !*valid {
        match validate_certs().await {
            Ok(_) => *valid = true,
            Err(e) => return Some(e),
        }
    }

    None
}

/// Invocation tag recorded when a command with an event stream failed while the daemon's certs
/// were invalid, whether or not the cert error became the command's classification. Failures
/// before the event stream exists (daemon state initialization) are not tagged.
pub const INVALID_CERTS_TAG: &str = "certs:invalid";

/// How the daemon's cert state relates to the errors a command failed with.
pub enum CertStateReport {
    /// The certs are valid.
    Valid,
    /// The certs are invalid and may well be why the command failed: RE, HTTP, Manifold and
    /// Sapling failures all look unrelated until the cert check runs. Report the cert error as the
    /// command's primary error.
    Explains(buck2_error::Error),
    /// The certs are invalid, but the command failed on a user error (`Tier::Input`) that it would
    /// have failed on with valid certs too. Keep the user's error as the classification and only
    /// show the cert error as a hint.
    Hint(buck2_error::Error),
}

impl CertStateReport {
    pub fn cert_error(&self) -> Option<&buck2_error::Error> {
        match self {
            Self::Valid => None,
            Self::Explains(e) | Self::Hint(e) => Some(e),
        }
    }
}

/// Check the cert state after a command failed with `errors`.
pub async fn check_cert_state_for<'a>(
    cert_state: CertState,
    errors: impl IntoIterator<Item = &'a buck2_error::Error>,
) -> CertStateReport {
    match check_cert_state(cert_state).await {
        Some(cert_error) => report_cert_error(cert_error, errors),
        None => CertStateReport::Valid,
    }
}

/// The invocation recorder classifies a command by its best-ranked error, and a cert error
/// outranks every user error, so it must not be added to a user error's report. Any error that
/// is not a user error, including a user error that also carries an infra tag, keeps the cert
/// error as the classification: the infra tag is the root cause, and certs may well be behind it.
fn report_cert_error<'a>(
    cert_error: buck2_error::Error,
    errors: impl IntoIterator<Item = &'a buck2_error::Error>,
) -> CertStateReport {
    for error in errors {
        if error.get_tier() != Some(Tier::Input) {
            return CertStateReport::Explains(cert_error);
        }
    }
    CertStateReport::Hint(cert_error)
}

#[cfg(test)]
mod report_tests {
    use buck2_error::ErrorTag;

    use super::*;

    fn cert_error() -> buck2_error::Error {
        InvalidCertsError::ClientCerts("test".to_owned()).into()
    }

    fn user_error() -> buck2_error::Error {
        buck2_error!(ErrorTag::MissingTarget, "Unknown target `foo`")
    }

    fn http_error() -> buck2_error::Error {
        buck2_error!(ErrorTag::Hyper, "client error (SendRequest)")
    }

    #[test]
    fn user_error_keeps_its_classification() {
        let user_error = user_error();
        let CertStateReport::Hint(hint) = report_cert_error(cert_error(), [&user_error]) else {
            panic!("a user error is not explained by invalid certs");
        };
        assert!(hint.has_tag(ErrorTag::NoValidCerts));
        assert_eq!(user_error.get_tier(), Some(Tier::Input));
    }

    #[test]
    fn http_error_is_classified_as_certs() {
        let http_error = http_error();
        let CertStateReport::Explains(e) = report_cert_error(cert_error(), [&http_error]) else {
            panic!("an HTTP failure with invalid certs is a cert error");
        };
        // The server wraps the original error as context of the cert error.
        let e = e.context(format!("{http_error:?}"));
        assert_eq!(e.best_tag(), Some(ErrorTag::NoValidCerts));
        assert_eq!(e.get_tier(), Some(Tier::Environment));
    }

    #[test]
    fn user_error_next_to_infra_error_is_classified_as_certs() {
        // The recorder classifies this command as infra, so the certs explain it.
        let errors = [user_error(), http_error()];
        assert!(matches!(
            report_cert_error(cert_error(), &errors),
            CertStateReport::Explains(_)
        ));
    }

    /// A user error that also carries an infra tag is an infra error: the infra tag is the root
    /// cause (an Eden read failing with EIO under a package listing error, for instance).
    #[test]
    fn user_error_with_infra_tag_is_classified_as_certs() {
        let error = user_error().tag([ErrorTag::IoInputOutputError, ErrorTag::IoSource]);
        assert_eq!(error.get_tier(), Some(Tier::Environment));
        assert!(matches!(
            report_cert_error(cert_error(), [&error]),
            CertStateReport::Explains(_)
        ));
    }

    #[test]
    fn starlark_error_keeps_its_classification() {
        let error = buck2_error!(ErrorTag::StarlarkParser, "parse error").tag([ErrorTag::Analysis]);
        assert!(matches!(
            report_cert_error(cert_error(), [&error]),
            CertStateReport::Hint(_)
        ));
    }

    /// A user error next to a cert-caused failure of another target: the recorder classifies
    /// the command by the cert-caused failure, so the cert error stays the classification.
    #[test]
    fn mixed_user_and_cert_caused_errors_are_classified_as_certs() {
        let errors = [
            user_error(),
            buck2_error!(ErrorTag::ReUnauthenticated, "RE: unauthenticated"),
        ];
        assert!(matches!(
            report_cert_error(cert_error(), &errors),
            CertStateReport::Explains(_)
        ));
    }

    /// Before the event stream exists the hint is attached as context, which keeps the user's
    /// classification and the cert message.
    #[test]
    fn hint_as_context_keeps_user_classification() {
        let error = user_error().context(cert_error().to_string());
        assert_eq!(error.get_tier(), Some(Tier::Input));
        assert!(format!("{error:?}").contains("Could not find valid client certs"));
        assert!(format!("{error:?}").contains("Unknown target `foo`"));
    }

    #[test]
    fn invalid_certs_tag_is_reported_for_hints_too() {
        let user_error = user_error();
        let report = report_cert_error(cert_error(), [&user_error]);
        assert!(report.cert_error().is_some());
        assert!(CertStateReport::Valid.cert_error().is_none());
    }
}

#[cfg(fbcode_build)]
#[cfg(test)]
mod tests {
    use std::env;
    use std::ffi::OsString;

    use crate::validate::ValidCerts;
    use crate::validate::verify;

    #[tokio::test]
    async fn invalid_certs_test() {
        let base_path = env::var("TEST_CERT_LOCATIONS").unwrap();

        let empty_path = format!("{base_path}/test_empty.pem");
        let empty_res = verify(&OsString::from(empty_path), ValidCerts::Leaf).await;
        assert!(empty_res.is_err());
        let err_msg = empty_res.unwrap_err().to_string();
        assert!(
            err_msg.starts_with("Could not find any certs to validate"),
            "{}",
            format!("Actual: {err_msg}")
        );

        let invalid_path = format!("{base_path}/test_invalid.pem");
        let invalid_res = verify(&OsString::from(invalid_path), ValidCerts::Leaf).await;
        assert!(invalid_res.is_err());
        let err_msg = invalid_res.unwrap_err().to_string();
        assert!(
            err_msg.starts_with("Could not find any certs to validate"),
            "{}",
            format!("Actual: {err_msg}")
        );

        // Self-signed cert for testing. Expired 05/31/2024
        let expired_path = format!("{base_path}/test_expired.pem");
        let expired_res = verify(&OsString::from(expired_path), ValidCerts::Leaf).await;
        assert!(expired_res.is_err());
        let err_msg = expired_res.unwrap_err().to_string();
        assert!(
            err_msg.starts_with("Certificate Expired"),
            "{}",
            format!("Actual: {err_msg}")
        );
    }

    #[tokio::test]
    async fn valid_cert_test() {
        // Self-signed cert for testing. Should expire in 100 years if this is around for that long!
        // Generated using:
        // 1. openssl genrsa -out mykey.pem 2048
        // 2. openssl req -new -key mykey.pem -out mycsr.csr
        // 3. openssl x509 -req -in mycsr.csr -signkey mykey.pem -out x509.crt -days 36500
        // Copy content in x509.crt
        let base_path = env::var("TEST_CERT_LOCATIONS").unwrap();
        let valid_path = format!("{base_path}/test_valid.pem");
        assert!(
            verify(&OsString::from(valid_path), ValidCerts::Leaf)
                .await
                .is_ok()
        );
    }

    /// A client certificate file holds the leaf first and its chain after it. An expired leaf
    /// is rejected even when a valid certificate follows it; a bundle only needs one valid
    /// certificate.
    #[tokio::test]
    async fn expired_leaf_followed_by_valid_cert_is_rejected() {
        let base_path = env::var("TEST_CERT_LOCATIONS").unwrap();
        let path = format!("{base_path}/test_expired_then_valid.pem");
        let res = verify(&OsString::from(path.clone()), ValidCerts::Leaf).await;
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.starts_with("Certificate Expired"), "{err_msg}");
        assert!(verify(&OsString::from(path), ValidCerts::Any).await.is_ok());
    }
}
