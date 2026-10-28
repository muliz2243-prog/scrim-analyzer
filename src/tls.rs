//! **Native** HTTPS client for the League client's local API.
//!
//! # Why this exists
//!
//! Previously the local API was read by launching `curl.exe -k` as a **child
//! process** every second. That pattern (unknown app → runs curl → URL, with
//! `-k` to skip certificate validation and with the console window hidden) is
//! exactly what antivirus software flags as a *downloader*, and it also
//! generated a process-creation event every second in any EDR.
//!
//! # What it does now
//!
//! The request is made **inside the process itself** with `reqwest` + `rustls`
//! (pure TLS, no external dependencies: the `.exe` no longer needs `curl.exe`
//! on the user's PC).
//!
//! And it is **stricter** than the old `-k`: the League client uses a
//! **self-signed** certificate (it does not fit any system trust chain), so
//! here the first certificate seen is **pinned** in memory and from then on
//! only that same certificate is accepted. If the client regenerates it (on
//! restart), it is pinned again and the change is counted so it is recorded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};

/// Verifier that **pins** the local client's certificate (TOFU + pin).
#[derive(Debug)]
pub struct VerificadorFijado {
    /// DER of the pinned certificate (the first one seen).
    pin: Mutex<Option<Vec<u8>>>,
    /// Accepted signature algorithms (those of the crypto provider).
    algoritmos: WebPkiSupportedAlgorithms,
    /// Number of times the client certificate has changed (been re-pinned).
    cambios: Mutex<u32>,
}

impl VerificadorFijado {
    fn new(algoritmos: WebPkiSupportedAlgorithms) -> Self {
        Self {
            pin: Mutex::new(None),
            algoritmos,
            cambios: Mutex::new(0),
        }
    }

    /// Number of times the client certificate has changed (was re-pinned).
    pub fn cambios(&self) -> u32 {
        *self.cambios.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ServerCertVerifier for VerificadorFijado {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let mut pin = self.pin.lock().unwrap_or_else(|e| e.into_inner());
        match pin.as_ref() {
            // Same certificate as the pinned one ⇒ valid.
            Some(fijado) if fijado.as_slice() == end_entity.as_ref() => {
                Ok(ServerCertVerified::assertion())
            }
            // The client regenerated it on restart: it is re-pinned and counted.
            Some(_) => {
                *pin = Some(end_entity.as_ref().to_vec());
                *self.cambios.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                Ok(ServerCertVerified::assertion())
            }
            // First time: the certificate presented by the local client is pinned.
            None => {
                *pin = Some(end_entity.as_ref().to_vec());
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algoritmos)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algoritmos)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algoritmos.supported_schemes()
    }
}

/// HTTPS client for the local API, with the client certificate pinned.
#[derive(Debug)]
pub struct ClienteLocal {
    http: reqwest::blocking::Client,
    verificador: Arc<VerificadorFijado>,
}

impl ClienteLocal {
    /// Creates the client. `timeout_s` is the maximum time for each request.
    pub fn new(timeout_s: u64) -> Result<Self, String> {
        // reqwest/rustls need a default crypto provider in the process; the
        // *ring* one is installed (the same one reqwest uses).
        if CryptoProvider::get_default().is_none() {
            let _ = ring::default_provider().install_default();
        }
        let provider = Arc::new(ring::default_provider());
        let verificador = Arc::new(VerificadorFijado::new(
            provider.signature_verification_algorithms.clone(),
        ));
        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS: {e}"))?
            .dangerous()
            .with_custom_certificate_verifier(verificador.clone())
            .with_no_client_auth();
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(cfg)
            // The local API never goes through a proxy: if the user has a
            // system proxy (VPN, Clash, corporate proxy), without this
            // reqwest would send the GET to `127.0.0.1` through the proxy and
            // the request would fail.
            .no_proxy()
            .timeout(Duration::from_secs(timeout_s))
            .build()
            .map_err(|e| format!("cliente HTTP: {e}"))?;
        Ok(Self { http, verificador })
    }

    /// GET to the local API and returns the body.
    pub fn get(&self, url: &str) -> Result<String, String> {
        let r = self.http.get(url).send().map_err(|e| e.to_string())?;
        r.text().map_err(|e| e.to_string())
    }

    /// Number of times the client certificate changed (was re-pinned).
    pub fn cambios_certificado(&self) -> u32 {
        self.verificador.cambios()
    }
}
