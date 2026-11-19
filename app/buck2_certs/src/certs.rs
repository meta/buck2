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
use std::path::Path;
#[cfg(fbcode_build)]
use std::sync::Arc;

use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_error::conversion::from_any_with_tag;
#[cfg(fbcode_build)]
use dupe::Dupe;
use rustls::ClientConfig;
use rustls::RootCertStore;
#[cfg(fbcode_build)]
use rustls::client::WebPkiServerVerifier;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::PrivateKeyDer;
use rustls_pki_types::pem::PemObject;

pub fn maybe_setup_cryptography() {
    setup_cryptography().ok();
}

pub fn setup_cryptography_or_fail() {
    setup_cryptography().unwrap();
}

fn setup_cryptography() -> std::result::Result<(), std::sync::Arc<rustls::crypto::CryptoProvider>> {
    // https://fb.workplace.com/groups/rust.language/permalink/29117966747825230/
    // Note that all but the first call will fail, so we callers should only use
    // this function as early as possible in their lifetime
    // Note that the use of 'ring' here is arbitrary and should not be
    // taken as an intentional choice of cryptographic provider
    rustls::crypto::ring::default_provider().install_default()
}

/// Load system root certs, trying a few different methods to get a valid root
/// certificate store.
async fn load_system_root_certs() -> buck2_error::Result<RootCertStore> {
    match find_root_ca_certs() {
        Some(path) => load_root_certs_from_path(Path::new(&path)).await,
        None => load_native_system_root_certs().await,
    }
}

async fn load_root_certs_from_path(path: &Path) -> buck2_error::Result<RootCertStore> {
    let root_certs = load_certs(path)
        .await
        .with_buck_error_context(|| format!("Loading root certs from: {}", path.display()))?;
    root_cert_store_from_certs(root_certs)
}

async fn load_native_system_root_certs() -> buck2_error::Result<RootCertStore> {
    let mut native_certs_results =
        tokio::task::spawn_blocking(rustls_native_certs::load_native_certs)
            .await
            .buck_error_context("Loading native system root certificates")?;

    let root_certs = if !native_certs_results.certs.is_empty() {
        Ok(native_certs_results.certs)
    } else {
        // Consider the last error to be indicative of the overall problem
        let native_certs_error = native_certs_results
            .errors
            .pop()
            .map(buck2_error::Error::from)
            .unwrap_or(buck2_error!(
                buck2_error::ErrorTag::NoValidCerts,
                "No certs or cert errors"
            ));

        if cfg!(fbcode_build) {
            let windows_message = if cfg!(target_os = "windows") {
                " on an admin PowerShell"
            } else {
                ""
            };
            let context = format!(
                "Error loading system root certificates native frameworks.
                    This is usually due to Chef not installed or working properly.
                    Please try `getchef -reason 'chef broken'`{windows_message}, `Fix My <OS>` via the f-menu, then `buck2 killall`.
                    If that doesn't resolve it, please visit HelpDesk to get Chef back to a healthy state."
            );
            Err(native_certs_error.context(context))
        } else {
            Err(native_certs_error
                .context("Error loading system root certificates native frameworks."))
        }
    }?;
    root_cert_store_from_certs(root_certs)
}

fn root_cert_store_from_certs(
    root_certs: Vec<CertificateDer<'static>>,
) -> buck2_error::Result<RootCertStore> {
    // According to [`rustls` documentation](https://docs.rs/rustls/latest/rustls/struct.RootCertStore.html#method.add_parsable_certificates),
    // it's better to only add parseable certs when loading system certs because
    // there are typically many system certs and not all of them can be valid. This
    // is pertinent for e.g. macOS which may have a lot of old certificates that may
    // not parse correctly.
    let mut roots = RootCertStore::empty();
    let (valid, invalid) = roots.add_parsable_certificates(root_certs);

    // But make sure we get at least _one_ valid cert, otherwise we legitimately won't be
    // able to make any connections via https.
    if valid == 0 {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Environment,
            "Error loading system certs: unable to find any valid system certs"
        ));
    }
    tracing::debug!("Loaded {} valid system root certs", valid);
    tracing::debug!("Loaded {} invalid system root certs", invalid);
    Ok(roots)
}

/// Replace server trust with the internal CA bundle and native system roots,
/// preserving client authentication and all other TLS settings.
#[cfg(fbcode_build)]
pub async fn set_internal_and_system_roots(config: &mut ClientConfig) -> buck2_error::Result<()> {
    let internal_roots = match find_root_ca_certs() {
        Some(path) => load_root_certs_from_path(Path::new(&path)).await?,
        None => RootCertStore::empty(),
    };
    let system_roots = load_native_system_root_certs().await?;
    configure_merged_roots(config, internal_roots, system_roots)
}

#[cfg(fbcode_build)]
fn configure_merged_roots(
    config: &mut ClientConfig,
    mut internal_roots: RootCertStore,
    system_roots: RootCertStore,
) -> buck2_error::Result<()> {
    internal_roots.roots.extend(system_roots.roots);
    let verifier = WebPkiServerVerifier::builder_with_provider(
        Arc::new(internal_roots),
        config.crypto_provider().dupe(),
    )
    .build()
    .map_err(|error| from_any_with_tag(error, buck2_error::ErrorTag::Certs))
    .buck_error_context("Creating TLS verifier with internal and system roots")?;
    config.dangerous().set_certificate_verifier(verifier);
    Ok(())
}

// Load private key from the given path
async fn load_key<P: AsRef<Path>>(key: P) -> buck2_error::Result<PrivateKeyDer<'static>> {
    let key = key.as_ref();

    let private_key = PrivateKeyDer::from_pem_file(key)
        .with_buck_error_context(|| format!("Error opening key file `{}`", key.display()))?;

    Ok(private_key)
}

/// Deserialize certificate pair at `cert` and `key` into structures that can
/// be inserted into rustls CertStore.
async fn load_cert_pair<P: AsRef<Path>>(
    cert: P,
    key: P,
) -> buck2_error::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let certs = load_certs(cert).await?;
    let key = load_key(key).await?;

    Ok((certs, key))
}

pub async fn tls_config_with_system_roots() -> buck2_error::Result<ClientConfig> {
    let system_roots = load_system_root_certs().await?;
    Ok(ClientConfig::builder()
        .with_root_certificates(system_roots)
        .with_no_client_auth())
}

pub async fn tls_config_with_single_cert<P: AsRef<Path>>(
    cert_path: P,
    key_path: P,
) -> buck2_error::Result<ClientConfig> {
    let system_roots = load_system_root_certs().await?;
    let (cert, key) = load_cert_pair(cert_path, key_path)
        .await
        .buck_error_context("Error loading certificate pair")?;
    ClientConfig::builder()
        .with_root_certificates(system_roots)
        .with_client_auth_cert(cert, key)
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Certs))
        .buck_error_context("Error creating TLS config with cert and key path")
}

// Load certs from the given path
pub(crate) async fn load_certs<P: AsRef<Path>>(
    cert_path: P,
) -> buck2_error::Result<Vec<CertificateDer<'static>>> {
    let cert_path = cert_path.as_ref();

    let cert_data = tokio::fs::read(cert_path)
        .await
        .with_buck_error_context(|| {
            format!("Error reading certificate file `{}`", cert_path.display())
        })?;

    let cert_results: Vec<Result<CertificateDer, rustls_pki_types::pem::Error>> =
        CertificateDer::pem_reader_iter(&mut cert_data.as_slice()).collect();

    let certs: Result<Vec<CertificateDer<'static>>, rustls_pki_types::pem::Error> =
        cert_results.into_iter().collect();

    certs.with_buck_error_context(|| {
        format!("Error reading certificate file `{}`", cert_path.display())
    })
}

/// Find root CA certs.
///
/// In OSS or non-fbcode builds, returns None; we do not support hardcoded root
/// certificates in non-fbcode builds and rely solely on rustls-native-certs.
pub(crate) fn find_root_ca_certs() -> Option<OsString> {
    #[cfg(fbcode_build)]
    return find_certs::find_root_ca_certs();

    #[cfg(not(fbcode_build))]
    match std::env::var_os("ROOT_CA_CERT_PATH") {
        Some(path) if Path::new(&path).exists() => Some(path),
        _ => None,
    }
}

/// Find TLS certs.
///
/// Return `None` in Cargo or open source builds; we do not support internal certs
/// in these builds.
pub fn find_internal_cert() -> Option<OsString> {
    #[cfg(fbcode_build)]
    return find_certs::find_tls_cert();

    #[cfg(not(fbcode_build))]
    return None;
}

/// Whether the machine buck is running on supports vpnless operation.
pub fn supports_vpnless() -> bool {
    #[cfg(fbcode_build)]
    return cpe::x2p::supports_vpnless();

    #[cfg(not(fbcode_build))]
    return false;
}

#[cfg(all(test, fbcode_build))]
mod tests {
    use rcgen::BasicConstraints;
    use rcgen::CertificateParams;
    use rcgen::DnType;
    use rcgen::ExtendedKeyUsagePurpose;
    use rcgen::IsCa;
    use rcgen::Issuer;
    use rcgen::KeyPair;
    use rcgen::KeyUsagePurpose;
    use rustls::CertificateError;
    use rustls::ClientConnection;
    use rustls::ServerConfig;
    use rustls::ServerConnection;
    use rustls::server::WebPkiClientVerifier;
    use rustls_pki_types::PrivatePkcs8KeyDer;

    use super::*;

    struct Identity {
        root: CertificateDer<'static>,
        cert: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    }

    impl Identity {
        fn new(name: &str) -> Self {
            let root_key = KeyPair::generate().unwrap();
            let mut root_params = CertificateParams::default();
            root_params
                .distinguished_name
                .push(DnType::CommonName, name);
            root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
            let root = root_params.self_signed(&root_key).unwrap();
            let issuer = Issuer::new(root_params, root_key);
            let key = KeyPair::generate().unwrap();
            let mut params = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
            params.extended_key_usages = vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ];
            let cert = params.signed_by(&key, &issuer).unwrap();
            Self {
                root: root.der().clone(),
                cert: cert.der().clone(),
                key: PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            }
        }

        fn roots(&self) -> RootCertStore {
            let mut roots = RootCertStore::empty();
            roots.add(self.root.clone()).unwrap();
            roots
        }

        fn server_config(&self, client_roots: RootCertStore) -> ServerConfig {
            let verifier = WebPkiClientVerifier::builder(Arc::new(client_roots))
                .build()
                .unwrap();
            let mut config = ServerConfig::builder()
                .with_client_cert_verifier(verifier)
                .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
                .unwrap();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            config
        }
    }

    fn handshake(
        client_config: &ClientConfig,
        server_config: &ServerConfig,
        server_name: &'static str,
    ) -> Result<(), rustls::Error> {
        let mut client = ClientConnection::new(
            Arc::new(client_config.clone()),
            server_name.try_into().unwrap(),
        )?;
        let mut server = ServerConnection::new(Arc::new(server_config.clone()))?;
        for _ in 0..10 {
            let mut client_bytes = Vec::new();
            client.write_tls(&mut client_bytes).unwrap();
            server.read_tls(&mut client_bytes.as_slice()).unwrap();
            server.process_new_packets()?;

            let mut server_bytes = Vec::new();
            server.write_tls(&mut server_bytes).unwrap();
            client.read_tls(&mut server_bytes.as_slice()).unwrap();
            client.process_new_packets()?;

            if !client.is_handshaking() && !server.is_handshaking() {
                assert!(
                    server
                        .peer_certificates()
                        .is_some_and(|certs| !certs.is_empty())
                );
                assert_eq!(client.alpn_protocol(), Some(b"http/1.1".as_slice()));
                return Ok(());
            }
        }
        panic!("TLS handshake did not complete")
    }

    #[test]
    fn test_root_cert_store_requires_valid_certificates() {
        assert!(root_cert_store_from_certs(Vec::new()).is_err());
        assert!(root_cert_store_from_certs(vec![CertificateDer::from(vec![0])]).is_err());
    }

    #[test]
    fn test_root_cert_store_skips_invalid_certificates() {
        let identity = Identity::new("System root");
        let roots =
            root_cert_store_from_certs(vec![CertificateDer::from(vec![0]), identity.root.clone()])
                .unwrap();
        assert_eq!(roots.roots, identity.roots().roots);
    }

    #[test]
    fn test_merged_roots_preserve_authentication_and_certificate_verification() {
        maybe_setup_cryptography();
        let internal = Identity::new("Internal root");
        let system = Identity::new("System root");
        let untrusted = Identity::new("Untrusted root");
        let internal_server = internal.server_config(internal.roots());
        let system_server = system.server_config(internal.roots());
        let untrusted_server = untrusted.server_config(internal.roots());
        let mut config = ClientConfig::builder()
            .with_root_certificates(internal.roots())
            .with_client_auth_cert(vec![internal.cert.clone()], internal.key.clone_key())
            .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.max_fragment_size = Some(1024);

        handshake(&config, &internal_server, "localhost").unwrap();
        assert_eq!(
            handshake(&config, &system_server, "localhost"),
            Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer
            ))
        );

        let client_auth = config.client_auth_cert_resolver.dupe();
        let crypto_provider = config.crypto_provider().dupe();
        configure_merged_roots(&mut config, internal.roots(), system.roots()).unwrap();
        assert!(Arc::ptr_eq(&client_auth, &config.client_auth_cert_resolver));
        assert!(Arc::ptr_eq(&crypto_provider, config.crypto_provider()));
        assert_eq!(config.max_fragment_size, Some(1024));
        handshake(&config, &internal_server, "localhost").unwrap();
        handshake(&config, &system_server, "localhost").unwrap();
        assert_eq!(
            handshake(&config, &untrusted_server, "localhost"),
            Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer
            ))
        );
        assert!(matches!(
            handshake(&config, &system_server, "wrong.example"),
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForNameContext { .. }
            ))
        ));

        configure_merged_roots(&mut config, RootCertStore::empty(), system.roots()).unwrap();
        handshake(&config, &system_server, "localhost").unwrap();
        assert_eq!(
            handshake(&config, &internal_server, "localhost"),
            Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer
            ))
        );
    }
}
