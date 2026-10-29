//! End-to-end test of the **native** HTTPS client (replaces the
//! `curl -k` child process).
//!
//! An ephemeral TLS server is started on `127.0.0.1` with a **self-signed**
//! certificate (the same kind the League client uses) and it is checked that:
//!   1. the client accepts that certificate and pins its fingerprint;
//!   2. later requests keep working with the pinned fingerprint;
//!   3. if the certificate changes (the LoL client regenerates it on restart),
//!      it is pinned again and the change is recorded.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use rofl_analyzer::live::{GoldModelParams, LivePoller};
use rofl_analyzer::tls::ClienteLocal;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;

/// Starts a minimal HTTPS server (one fixed JSON response) with a
/// fresh self-signed certificate. Returns the URL and the thread handle.
fn servidor_autofirmado(cuerpo: &str) -> (String, std::thread::JoinHandle<()>) {
    // The server needs a crypto provider in the process.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let tls = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("certificado autofirmado");
    let cert: CertificateDer<'static> = tls.cert.der().clone();
    let key = PrivatePkcs8KeyDer::from(tls.signing_key.serialize_der());

    let cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], PrivateKeyDer::Pkcs8(key))
        .expect("configuración TLS del servidor");
    let cfg = Arc::new(cfg);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind del servidor");
    let puerto = listener.local_addr().expect("dirección local").port();
    let cuerpo = cuerpo.to_string();

    let manejador = std::thread::spawn(move || {
        for conexion in listener.incoming() {
            let Ok(mut tcp) = conexion else { continue };
            let Ok(conn) = rustls::ServerConnection::new(cfg.clone()) else {
                continue;
            };
            let mut stream = rustls::StreamOwned::new(conn, &mut tcp);
            let mut buf = [0u8; 4096];
            // HTTP request (its content does not matter: only the GET matters).
            let _ = stream.read(&mut buf);
            let respuesta = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                cuerpo.len(),
                cuerpo
            );
            let _ = stream.write_all(respuesta.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("https://127.0.0.1:{puerto}/liveclientdata/allgamedata"), manejador)
}

#[test]
fn acepta_certificado_autofirmado_y_lo_fija() {
    let (url, _h) = servidor_autofirmado(r#"{"allgamedata":"ok"}"#);
    let cliente = ClienteLocal::new(10).expect("cliente HTTPS local");

    let cuerpo = cliente.get(&url).expect("primera petición");
    assert!(cuerpo.contains("allgamedata"), "cuerpo inesperado: {cuerpo}");
    assert_eq!(cliente.cambios_certificado(), 0, "no debería haber cambios aún");

    // Second request with the fingerprint already pinned: all the same.
    let cuerpo2 = cliente.get(&url).expect("segunda petición");
    assert!(cuerpo2.contains("allgamedata"));
    assert_eq!(cliente.cambios_certificado(), 0);
}

#[test]
fn cuenta_el_cambio_de_certificado() {
    let (url, _h) = servidor_autofirmado(r#"{"a":1}"#);
    let cliente = ClienteLocal::new(10).expect("cliente HTTPS local");
    cliente.get(&url).expect("primera petición");

    // Another server ⇒ another certificate ⇒ it is pinned again and the change is counted.
    let (url2, _h2) = servidor_autofirmado(r#"{"b":2}"#);
    let cuerpo = cliente.get(&url2).expect("petición con certificado distinto");
    assert!(cuerpo.contains("\"b\""));
    assert_eq!(cliente.cambios_certificado(), 1, "debería contar un cambio");
}

/// The full live path (native HTTPS → parsing → events) with a real
/// `allgamedata` capture.
///
/// The fixture is a real capture with the player names replaced by
/// `Jugador NN#TEST`, so the test also runs on CI (a local, uncommitted
/// `live_cap_a.json` used to be required, which made every run red).
#[test]
fn el_poller_ingiere_la_partida_por_https_nativo() {
    let ruta = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("live_cap_a.json");
    let cuerpo = std::fs::read_to_string(&ruta).expect("captura real de allgamedata");
    let (url, _h) = servidor_autofirmado(&cuerpo);

    let mut poller = LivePoller::with_config(
        &url,
        10,
        Duration::from_millis(200),
        GoldModelParams::default(),
    );
    assert!(poller.poll_once().expect("sondeo"), "el sondeo debe traer datos");

    let captura = poller.build_capture();
    let kills = captura
        .events
        .iter()
        .filter(|e| e.kind == "ChampionKill")
        .count();
    let torres = captura
        .events
        .iter()
        .filter(|e| e.kind == "TurretKilled")
        .count();
    assert_eq!(kills, 37, "kills esperados en la captura real");
    assert_eq!(torres, 4, "torres esperadas en la captura real");
    assert!(poller.duration_s() > 1000.0, "duración de partida leída");
}
