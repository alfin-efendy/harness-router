//! Thin client to the engine daemon's control API. Cockpit's Tauri commands
//! proxy through this; the CoreEvent bridge consumes `events()`.

use crate::error::CmdError;
use futures::Stream;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

pub struct EngineClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl EngineClient {
    pub fn new(base_url: String, token: String) -> Self {
        EngineClient {
            base_url,
            token,
            // No global timeout: OAuth flows legitimately block for minutes.
            http: reqwest::Client::new(),
        }
    }

    /// Like [`EngineClient::new`], but for a remote runner reached over TLS
    /// where the leaf certificate is pinned by fingerprint (TOFU — paired
    /// once via `/pair`, no CA chain) rather than validated against a root
    /// store. `fingerprint` is the SHA-256 (base64) of the runner's cert, as
    /// returned by the daemon's `tls::load_or_generate` and captured during
    /// pairing. Same no-global-timeout property as `new`.
    pub fn new_pinned(base_url: String, device_token: String, fingerprint: String) -> Self {
        EngineClient {
            base_url,
            token: device_token,
            http: pinned_client(&fingerprint),
        }
    }

    pub async fn rpc<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, CmdError> {
        let resp = self
            .http
            .post(format!("{}/rpc/{}", self.base_url, method))
            .bearer_auth(&self.token)
            .json(&params)
            .send()
            .await
            .map_err(|e| CmdError {
                message: format!("engine unreachable: {e}"),
            })?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| CmdError {
            message: format!("engine returned invalid JSON: {e}"),
        })?;
        if status.is_success() {
            serde_json::from_value(body).map_err(|e| CmdError {
                message: format!("engine result decode failed for {method}: {e}"),
            })
        } else {
            Err(CmdError {
                message: body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("engine error")
                    .to_string(),
            })
        }
    }

    /// Authed `GET {base_url}/attachments/{rel}` — the raw bytes of one
    /// attachment file (plus its `Content-Type` header, if the server sent
    /// one) reused for both the local engine and a pinned-TLS remote runner
    /// exactly like [`EngineClient::rpc`]/[`EngineClient::events`]. `rel` is
    /// interpolated directly into the URL path (same convention as `rpc`'s
    /// `method` and `resolve_approval`'s `request_id` above) — the `reqwest`/
    /// `url` stack percent-encodes anything that needs it (spaces, non-ASCII
    /// filename bytes) while parsing, so callers never need to encode `rel`
    /// themselves. The route itself (`serve.rs::get_attachment`) is jailed
    /// and size-capped on the engine side; this is just the thin proxy.
    pub async fn get_attachment_bytes(
        &self,
        rel: &str,
    ) -> Result<(Vec<u8>, Option<String>), CmdError> {
        let resp = self
            .http
            .get(format!("{}/attachments/{}", self.base_url, rel))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| CmdError {
                message: format!("engine unreachable: {e}"),
            })?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if !status.is_success() {
            // Best-effort error message: the route's error envelope is JSON
            // `{"error": "..."}`, but a non-JSON body (e.g. an intermediary's
            // own error page) must not fail the whole call.
            let body = resp.bytes().await.unwrap_or_default();
            let message = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .unwrap_or_else(|| format!("engine returned {status}"));
            return Err(CmdError { message });
        }
        let bytes = resp.bytes().await.map_err(|e| CmdError {
            message: format!("engine attachment read failed: {e}"),
        })?;
        Ok((bytes.to_vec(), content_type))
    }

    pub async fn resolve_approval(
        &self,
        run_id: &str,
        request_id: &str,
        response: ryuzi_core::domain::ApprovalResponse,
    ) -> bool {
        let r = self
            .http
            .post(format!(
                "{}/approvals/{}/{}",
                self.base_url, run_id, request_id
            ))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "response": response }))
            .send()
            .await;
        match r {
            Ok(resp) => resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("resolved").and_then(|b| b.as_bool()))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Line-parse the SSE stream: accumulate raw bytes, split on '\n', strip
    /// "data: " prefixes, parse JSON. Ignores comments/keep-alives.
    ///
    /// Bytes are buffered as `Vec<u8>` (not `String`) because reqwest chunk
    /// boundaries do not respect UTF-8 character boundaries: a multi-byte
    /// sequence (emoji, CJK, accented text in a Notice/Message payload) can
    /// legitimately split across two chunks. Decoding each chunk
    /// independently would replace the truncated tail with U+FFFD before the
    /// completing bytes ever arrive. Decoding only complete, newline-terminated
    /// lines guarantees every decode sees a full UTF-8 boundary.
    pub async fn events(&self) -> Result<impl Stream<Item = Value>, CmdError> {
        use futures::StreamExt;
        let resp = self
            .http
            .get(format!("{}/events", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| CmdError {
                message: format!("engine events unreachable: {e}"),
            })?;
        let mut buf: Vec<u8> = Vec::new();
        let stream = resp
            .bytes_stream()
            .filter_map(move |chunk| {
                let mut out: Vec<Value> = Vec::new();
                if let Ok(bytes) = chunk {
                    buf.extend_from_slice(&bytes);
                    out = drain_lines(&mut buf);
                }
                async move { Some(futures::stream::iter(out)) }
            })
            .flatten();
        Ok(stream)
    }
}

/// Drain complete (`\n`-terminated) lines from `buf`, decode each as UTF-8,
/// strip the SSE `"data: "` prefix, and parse the payload as JSON. Bytes
/// after the last newline are left in `buf` for the next call so a
/// multi-byte UTF-8 sequence split across chunk boundaries is only decoded
/// once all of its bytes have arrived. Non-data and unparseable lines are
/// skipped.
fn drain_lines(buf: &mut Vec<u8>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buf.drain(..=nl).collect();
        let line = String::from_utf8_lossy(&line_bytes);
        let line = line.trim_end();
        if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                out.push(v);
            }
        }
    }
    out
}

const SPAWN_TIMEOUT_MS: u64 = 30_000;

/// Attach to a live daemon, or spawn the `ryuzi` control-plane binary
/// (resolved by [`resolve_control_binary`]) and wait for it to become
/// reachable.
///
/// The returned `EngineClient` is captured ONCE by the caller (typically into
/// managed Tauri state) and never refreshed mid-session, so recovering across
/// a later daemon restart (crash, canary self-update) relies on the token and
/// port staying stable — `control_token::write_token` now reuses an existing
/// valid token across same-port restarts specifically so this held client
/// keeps working without a manual Cockpit restart. A `control_port` change,
/// however, still requires restarting Cockpit.
pub async fn connect_or_spawn() -> anyhow::Result<EngineClient> {
    let dir = ryuzi_core::paths::db_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    if let Some(client) = try_attach(&dir).await {
        return Ok(client);
    }

    // A live-but-unreachable daemon (pre-API version, stale token) blocks
    // attach forever — take over: SIGTERM, wait dead, respawn. Reconcile
    // resumes any interrupted turns after the new daemon boots.
    if let Some(status) = ryuzi_core::daemon_status::read_status(&dir) {
        if status.pid > 0 && ryuzi_core::daemon_status::is_alive(status.pid) {
            eprintln!(
                "[ryuzi] engine daemon (pid {}) unreachable — restarting it",
                status.pid
            );
            ryuzi_core::daemon_status::send_sigterm(status.pid);
            for _ in 0..50 {
                if !ryuzi_core::daemon_status::is_alive(status.pid) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    spawn_control_plane(&dir)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(SPAWN_TIMEOUT_MS);
    while std::time::Instant::now() < deadline {
        if let Some(client) = try_attach(&dir).await {
            return Ok(client);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    anyhow::bail!("engine daemon did not become reachable within 30s (see daemon.log)")
}

async fn try_attach(dir: &std::path::Path) -> Option<EngineClient> {
    let status = ryuzi_core::daemon_status::read_status(dir)?;
    let state = ryuzi_core::daemon_status::derive_state(
        Some(&status),
        &ryuzi_core::daemon_status::is_alive,
    );
    if !state.running {
        return None;
    }
    let port = status.port?;
    let token = ryuzi_core::control_token::read_token(dir)?;
    let base_url = local_base_url(status.scheme.as_deref(), status.host.as_deref(), port);
    // A non-loopback bind serves TLS and nothing else (`serve` binds exactly
    // one listener), and its cert is self-signed, so it can only be reached
    // through the same fingerprint pin remote runners use — the daemon writes
    // that fingerprint into `daemon.json` precisely so a client can.
    let client = match (status.scheme.as_deref(), status.fingerprint) {
        (Some("https"), Some(fingerprint)) => {
            EngineClient::new_pinned(base_url, token, fingerprint)
        }
        _ => EngineClient::new(base_url, token),
    };
    // Auth-verified liveness probe: /health is open, so probe an authed route.
    match client
        .rpc::<Option<String>>("get_setting", serde_json::json!({"key": "control_port"}))
        .await
    {
        Ok(_) => Some(client),
        Err(_) => None,
    }
}

/// Base URL for the LOCAL daemon that `daemon.json` describes.
///
/// A daemon Cockpit spawned is always loopback: the spawn sets
/// `RYUZI_MANAGED_HOST=1`, and `daemon_cmd::effective_listen_addr` in
/// `crates/control` forces `127.0.0.1` under that flag regardless of the
/// `listen_addr` setting — reproducing what the deleted in-process daemon
/// did. So for the common path this returns `http://127.0.0.1:{port}` by
/// construction, exactly as before.
///
/// This function still handles the other shapes `daemon.json` can carry,
/// because Cockpit also attaches to a daemon **it did not spawn** — one
/// already running from a standalone `ryuzi start`, which does honour
/// `listen_addr`:
///
/// - a **wildcard** bind (`0.0.0.0` / `::`) is not itself dialable, but
///   loopback is part of what it accepts and is the one route guaranteed to
///   exist from this machine. Dialing loopback also keeps the *peer* address
///   loopback, which `serve::authorize` requires before it will accept the
///   control token.
/// - a **specific** non-loopback bind (`listen_addr = 192.168.1.5`) is not
///   attachable from here. The URL and the TLS pin are both right, but
///   `serve::authorize` answers 401: the control token authenticates a
///   loopback peer only, and a connection to the host's own LAN address does
///   not have a loopback peer address. Reaching such a daemon needs a paired
///   device token (`ryuzi pair`) — the remote-client path, not this one.
///   `try_attach` returns `None`, and `connect_or_spawn` then spawns a
///   managed daemon of its own, which loopback-binds and is reachable.
/// - **absent** host (a pre-remote-runner status file, always loopback), an
///   **IPv6 literal** (needs brackets inside a URL authority), and anything
///   **unparsable** (falls back to loopback, mirroring `resolve_bind`'s own
///   "a bad setting value must never widen the surface" rule).
fn local_base_url(scheme: Option<&str>, host: Option<&str>, port: u16) -> String {
    use std::net::IpAddr;
    let scheme = scheme.unwrap_or("http");
    let host = match host.and_then(|h| h.parse::<IpAddr>().ok()) {
        Some(IpAddr::V6(ip)) if ip.is_unspecified() => "[::1]".to_string(),
        Some(IpAddr::V6(ip)) => format!("[{ip}]"),
        Some(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        Some(ip) => ip.to_string(),
        None => "127.0.0.1".to_string(),
    };
    format!("{scheme}://{host}:{port}")
}

/// Open `<dir>/daemon.log` for append, creating the state dir if missing.
///
/// The `create_dir_all` is load-bearing on a fresh install: `dir` is
/// `paths::state_dir()`, which nothing has created yet (the installer does
/// not, and the daemon's own `Store::open` cannot run until it has been
/// spawned — which is what this log file is for). `File::create` does not
/// create parent directories, so without this the very first thing Cockpit
/// does on a clean machine fails with NotFound, the daemon never spawns, and
/// `setup()` dies before the window is ever shown.
fn open_daemon_log(dir: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("daemon.log");
    // Rotate before the handle is handed to the spawned child: afterwards this
    // file is the child's inherited stdout/stderr fd and cannot be swapped.
    ryuzi_core::logging::rotate_if_large(&path, ryuzi_core::logging::MAX_DAEMON_LOG_BYTES);
    std::fs::File::options()
        .append(true)
        .create(true)
        .open(path)
}

/// On-disk filename of the control-plane binary Cockpit spawns.
///
/// Three places must agree on this name and there is no build-time link
/// between them: this resolver, `tauri.conf.json`'s
/// `bundle.externalBin = ["binaries/ryuzi"]`, and
/// `scripts/cockpit/stage-sidecar.ts`, which copies the built binary into that
/// slot. Tauri strips the `-<target-triple>` suffix when it installs the
/// sidecar, so the installed name is exactly this.
pub(crate) const CONTROL_BIN: &str = if cfg!(windows) { "ryuzi.exe" } else { "ryuzi" };

/// Locate the `ryuzi` control-plane binary. See
/// [`resolve_control_binary_in`] for the search order.
fn resolve_control_binary() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let exe_dir = exe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let override_path = std::env::var_os("RYUZI_CONTROL_BIN").map(std::path::PathBuf::from);
    resolve_control_binary_in(override_path, &exe_dir, lookup_on_path)
}

/// Search order, most specific first:
///
/// 1. `override_path` — the `RYUZI_CONTROL_BIN` environment variable. For
///    tests, and for running Cockpit against a control plane built elsewhere.
/// 2. A sibling of the running executable. This one branch covers both real
///    layouts: Tauri installs an `externalBin` sidecar next to the app
///    executable, and `cargo` puts `ryuzi` and `ryuzi-cockpit` in the same
///    `target/<profile>/` directory, so `cargo tauri dev` works with no extra
///    wiring.
/// 3. `PATH`, for a separately installed control plane.
fn resolve_control_binary_in(
    override_path: Option<std::path::PathBuf>,
    exe_dir: &std::path::Path,
    path_lookup: impl FnOnce(&str) -> Option<std::path::PathBuf>,
) -> anyhow::Result<std::path::PathBuf> {
    if let Some(path) = override_path {
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "RYUZI_CONTROL_BIN points at {} but that is not a file",
            path.display()
        );
    }
    let sibling = exe_dir.join(CONTROL_BIN);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if let Some(found) = path_lookup(CONTROL_BIN) {
        return Ok(found);
    }
    anyhow::bail!(
        "control-plane binary `{CONTROL_BIN}` not found next to {} or on PATH. \
         Build one with `cargo build -p ryuzi-control`, or point RYUZI_CONTROL_BIN at an existing binary.",
        exe_dir.display()
    )
}

/// First entry in `PATH` that holds a file called `name`.
fn lookup_on_path(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn spawn_control_plane(dir: &std::path::Path) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    let bin = resolve_control_binary()?;
    let log = open_daemon_log(dir)?;
    let log2 = log.try_clone()?;
    let mut cmd = Command::new(bin);
    // `start` is the foreground daemon entry point; this process is detached
    // below, so it keeps running after Cockpit exits — the same lifetime the
    // in-process daemon mode Cockpit used to host had.
    //
    // RYUZI_MANAGED_HOST: Cockpit ships this binary inside its own bundle and
    // updates it as one unit, so the daemon must not run its own self-updater.
    // See `daemon_cmd::self_update_enabled` in crates/control.
    cmd.arg("start")
        .env("RYUZI_MANAGED_HOST", "1")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log2);
    // The control plane is a CONSOLE-subsystem binary and a release Cockpit is
    // a GUI process (`windows_subsystem = "windows"` in main.rs), so Windows
    // allocates a brand-new console window for the child unless the spawn asks
    // it not to — redirecting stdout/stderr does NOT suppress it. That window
    // would sit in the taskbar for the daemon's whole detached lifetime, and
    // closing it would deliver a Ctrl-Close event that kills the engine. No-op
    // off Windows. See `ryuzi_core::process_util`'s module doc.
    ryuzi_core::process_util::no_window_std(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

// ---- P3-2: pinned-TLS client for remote runners ----
//
// Ported verbatim from `crates/core/tests/control_api.rs`'s
// `remote_pair_then_authed_rpc_and_sse_over_pinned_tls` test support code
// (FingerprintPin + pinned_client), which is exercised end-to-end there
// against a real ring `ServerConfig` (`tls::load_or_generate` +
// `tls::server_config`). Reuses `ryuzi_core::tls::fingerprint_cert_der` —
// never reimplement the hash, or client and server pins drift and nothing
// connects.

/// A `rustls::client::danger::ServerCertVerifier` that trusts ONE
/// certificate: the one whose SHA-256 fingerprint (base64, standard
/// alphabet — computed via the real `tls::fingerprint_cert_der`, not a
/// reimplementation, so the two can never drift) matches
/// `expected_fingerprint`. This is TOFU certificate pinning, not
/// `danger_accept_invalid_certs` — a presented cert with the WRONG
/// fingerprint is rejected, `danger_accept_invalid_certs` would accept it.
///
/// Signature verification is NOT skipped: `verify_tls12_signature` /
/// `verify_tls13_signature` delegate to the ring provider's own algorithms
/// (`rustls::crypto::verify_tls12_signature` / `verify_tls13_signature`), so
/// this verifier only relaxes the CA-chain-of-trust check (there is no CA —
/// see `tls.rs`'s module docs), not cryptographic signature validation.
#[derive(Debug)]
struct FingerprintPin {
    expected_fingerprint: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for FingerprintPin {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let presented = ryuzi_core::tls::fingerprint_cert_der(end_entity.as_ref());
        if presented == self.expected_fingerprint {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "fingerprint pin mismatch: expected {}, got {presented}",
                self.expected_fingerprint
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A `reqwest::Client` wired to a ring-provider `rustls::ClientConfig` whose
/// only trust decision is [`FingerprintPin`] — built via
/// `ClientBuilder::use_preconfigured_tls`, the supported way to hand reqwest
/// a fully custom rustls config (confirmed against the vendored reqwest
/// 0.12.28 / rustls 0.23.41 sources: `use_preconfigured_tls` downcasts to
/// `rustls::ClientConfig` and, when it matches, routes straight to
/// `Connector::new_rustls_tls`, bypassing reqwest's own root-store/verifier
/// setup entirely).
fn pinned_client(fingerprint: &str) -> reqwest::Client {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(FingerprintPin {
        expected_fingerprint: fingerprint.to_string(),
        provider: provider.clone(),
    });
    let client_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring provider supports default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    reqwest::Client::builder()
        .use_preconfigured_tls(client_config)
        .build()
        .expect("reqwest client builds over the preconfigured pinned rustls config")
}

/// P3-6: pair with a remote runner the user just typed Host/Port/Fingerprint
/// for. No [`EngineClient`] exists yet for this runner — pairing IS the
/// bootstrap that produces its device token — so this is a free function
/// rather than an instance method. Builds a one-shot [`pinned_client`]
/// trusting `fingerprint` (TOFU: the operator copied it from `ryuzi pair`'s
/// printout on the remote host) and POSTs the pairing code to
/// `{base_url}/pair`, mirroring the wire shape `serve.rs`'s `PairRequest`
/// /pair handler expects and returns (see
/// `remote_pair_then_authed_rpc_and_sse_over_pinned_tls` in
/// `crates/core/tests/control_api.rs`, which exercises the exact same call
/// shape end-to-end). On success, returns the plaintext `device_token`;
/// the only caller (`gateways_cmd::add_runner`) hands it straight to the
/// LOCAL engine's `save_runner` RPC and to `EngineManager::add_runner` —
/// it is never returned to a `#[tauri::command]`'s own return value, so it
/// never reaches the webview.
pub async fn pair_over_pinned_tls(
    base_url: &str,
    fingerprint: &str,
    code: &str,
    device_name: &str,
) -> Result<String, CmdError> {
    let client = pinned_client(fingerprint);
    let resp = client
        .post(format!("{base_url}/pair"))
        .json(&serde_json::json!({ "code": code, "device_name": device_name }))
        .send()
        .await
        .map_err(|e| CmdError {
            message: format!("pairing request failed: {e}"),
        })?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| CmdError {
        message: format!("pairing response decode failed: {e}"),
    })?;
    if status.is_success() {
        body.get("device_token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| CmdError {
                message: "pairing response missing device_token".to_string(),
            })
    } else {
        Err(CmdError {
            message: body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("pairing failed")
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    async fn test_server(token: &str) -> (String, Arc<ryuzi_core::ControlPlane>) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(ryuzi_core::Store::open(tmp.path()).await.unwrap());
        std::mem::forget(tmp);
        let persistence = ryuzi_core::agents::bootstrap::AgentPersistence::temporary(store.clone())
            .await
            .unwrap();
        let handles = persistence.handles();
        let cp = ryuzi_core::ControlPlane::new(
            store.clone(),
            ryuzi_core::Registries::new(),
            persistence,
        )
        .await;
        let state = ryuzi_core::serve::ApiState {
            router_server: Arc::new(ryuzi_core::llm_router::server::RouterServer::new(
                cp.store().clone(),
            )),
            cp: cp.clone(),
            agents: handles.registry,
            agent_knowledge: handles.knowledge,
            learning_queue: handles.learning,
            control_token: token.to_string(),
        };
        let opts = ryuzi_core::serve::ServeOpts {
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            tls: None,
        };
        let port = ryuzi_core::serve::serve(state, opts).await.unwrap();
        (format!("http://127.0.0.1:{port}"), cp)
    }

    /// Points `cp`'s `attachments_root()` at `<dir>/.harness-attachments` by
    /// writing the `workdir_root` setting the real method reads — same
    /// pattern `serve.rs`'s own attachment-route tests use.
    async fn set_attachments_root(
        cp: &ryuzi_core::ControlPlane,
        dir: &std::path::Path,
    ) -> std::path::PathBuf {
        ryuzi_core::settings::SettingsStore::new(cp.store().clone())
            .set("workdir_root", dir.to_str().unwrap())
            .await
            .unwrap();
        cp.attachments_root().await
    }

    #[tokio::test]
    async fn get_attachment_bytes_round_trips_a_real_file_over_the_authed_route() {
        let (base, cp) = test_server("tok").await;
        let dir = tempfile::tempdir().unwrap();
        let root = set_attachments_root(&cp, dir.path()).await;
        std::fs::create_dir_all(root.join("sess-1")).unwrap();
        std::fs::write(
            root.join("sess-1").join("shot.png"),
            [0x89, 0x50, 0x4e, 0x47],
        )
        .unwrap();

        let client = EngineClient::new(base, "tok".into());
        let (bytes, content_type) = client
            .get_attachment_bytes("sess-1/shot.png")
            .await
            .unwrap();
        assert_eq!(bytes, vec![0x89, 0x50, 0x4e, 0x47]);
        assert_eq!(content_type.as_deref(), Some("image/png"));
    }

    /// A missing attachment surfaces the engine's own JSON error message
    /// (`"attachment not found"`, from `serve.rs::attachment_not_found`)
    /// rather than a decode failure or a generic transport error string.
    #[tokio::test]
    async fn get_attachment_bytes_surfaces_the_engines_error_message_on_404() {
        let (base, cp) = test_server("tok").await;
        let dir = tempfile::tempdir().unwrap();
        set_attachments_root(&cp, dir.path()).await;

        let client = EngineClient::new(base, "tok".into());
        let err = client
            .get_attachment_bytes("sess-1/nope.png")
            .await
            .unwrap_err();
        assert_eq!(err.message, "attachment not found");
    }

    /// Fresh install: `paths::state_dir()` does not exist yet, and opening the
    /// daemon log is the FIRST thing Cockpit does there — before the daemon it
    /// would spawn (and that daemon's `Store::open`) can create the directory.
    /// Without the `create_dir_all`, this returns NotFound, `connect_or_spawn`
    /// bails, and `lib.rs`'s `setup()` panics on
    /// `.expect("engine daemon unreachable")` while the window is still
    /// `visible: false` — the app exits (code 101) having shown nothing at all.
    ///
    /// Windows note: see `pinned_client_and_new_pinned_construct_without_panicking`
    /// below — `cargo test -p ryuzi-cockpit` can't run locally (tauri#13419),
    /// so `cargo check --tests -p ryuzi-cockpit` is the local evidence here.
    #[test]
    fn open_daemon_log_creates_the_state_dir_when_it_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("ryuzi");
        assert!(!fresh.exists(), "precondition: state dir must be missing");

        let _log = open_daemon_log(&fresh).expect("open the log in a not-yet-created state dir");

        assert!(fresh.join("daemon.log").exists(), "daemon.log not created");
    }

    #[tokio::test]
    async fn rpc_ok_and_error_envelope() {
        let (base, _cp) = test_server("tok").await;
        let client = EngineClient::new(base, "tok".into());

        let projects: Vec<serde_json::Value> =
            client.rpc("list_projects", json!({})).await.unwrap();
        assert!(projects.is_empty());

        let err = client
            .rpc::<serde_json::Value>("nope", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.message, "unknown method: nope");
    }

    #[tokio::test]
    async fn events_stream_yields_core_events() {
        use futures::StreamExt;
        let (base, cp) = test_server("tok").await;
        let client = EngineClient::new(base, "tok".into());
        let mut stream = Box::pin(client.events().await.unwrap());
        cp.emit(ryuzi_core::CoreEvent::Notice {
            session_pk: "s".into(),
            text: "ping".into(),
        });
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev["kind"], "notice");
    }

    /// A multi-byte UTF-8 char (emoji, 4 bytes) split across two reqwest
    /// chunks must survive intact — not get replaced with U+FFFD by
    /// per-chunk lossy decoding.
    #[test]
    fn drain_lines_reassembles_utf8_split_across_chunks() {
        let payload = json!({"kind": "notice", "text": "hello 🎉 world"});
        let line = format!("data: {}\n", payload);
        let bytes = line.into_bytes();
        // Split the buffer mid-sequence, inside the 4-byte emoji encoding,
        // so neither half is valid UTF-8 on its own.
        let emoji_pos = bytes
            .windows(4)
            .position(|w| w == "🎉".as_bytes())
            .expect("emoji bytes present in payload");
        let split_at = emoji_pos + 2; // land inside the 4-byte sequence

        let mut buf: Vec<u8> = Vec::new();
        // First chunk: ends mid-character. No complete line yet, so no
        // events should be produced (and no premature lossy decode).
        buf.extend_from_slice(&bytes[..split_at]);
        let first = drain_lines(&mut buf);
        assert!(first.is_empty(), "no newline yet, nothing should drain");

        // Second chunk: completes the character and the line.
        buf.extend_from_slice(&bytes[split_at..]);
        let events = drain_lines(&mut buf);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["text"], "hello 🎉 world");
        assert!(
            !events[0]["text"].as_str().unwrap().contains('\u{FFFD}'),
            "multi-byte char must not be corrupted into U+FFFD"
        );
    }

    /// Construction-only smoke test: `pinned_client` builds a `reqwest::Client`
    /// over a preconfigured, ring-provider rustls `ClientConfig` without
    /// panicking (the `.expect()`s inside `pinned_client` would panic on a
    /// bad provider/config or a `use_preconfigured_tls` downcast mismatch —
    /// see the rustls-unification note on the Cargo.toml dependency). A full
    /// handshake test (correct fingerprint accepted, wrong one rejected) is
    /// already covered end-to-end against a real ring `ServerConfig` in
    /// `crates/core/tests/control_api.rs`'s
    /// `remote_pair_then_authed_rpc_and_sse_over_pinned_tls` — this is the
    /// SAME `FingerprintPin`/`pinned_client` code, ported verbatim, so that
    /// coverage applies here too. Standing up a second TLS server in this
    /// crate's unit tests would be redundant; `EngineClient::new_pinned`
    /// itself is a plain field-assignment wrapper with no branching logic to
    /// cover beyond "it constructs".
    ///
    /// Note: on this Windows dev box, `cargo test -p ryuzi-cockpit` can crash
    /// the test binary for unrelated reasons (tauri#13419); `cargo check
    /// --tests -p ryuzi-cockpit` compiling this test is acceptable evidence
    /// when the binary itself cannot be run locally.
    #[test]
    fn pinned_client_and_new_pinned_construct_without_panicking() {
        let _client = pinned_client("somefp");
        let engine = EngineClient::new_pinned(
            "https://127.0.0.1:9999".into(),
            "device-token".into(),
            "somefp".into(),
        );
        assert_eq!(engine.base_url, "https://127.0.0.1:9999");
        assert_eq!(engine.token, "device-token");
    }

    /// A TLS-enabled variant of `test_server`, standing up a real ring
    /// `ServerConfig` (same construction `control_api.rs`'s
    /// `remote_pair_then_authed_rpc_and_sse_over_pinned_tls` uses) so
    /// `pair_over_pinned_tls`'s pinned-client `/pair` POST performs a
    /// genuine TLS handshake, not a plaintext connection. Returns the base
    /// URL, the server's real cert fingerprint, and an `Arc<Store>` handle
    /// so tests can mint pairing codes against the same backing store.
    async fn test_tls_server(token: &str) -> (String, String, std::sync::Arc<ryuzi_core::Store>) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("ryuzi.sqlite");
        let store = Arc::new(ryuzi_core::Store::open(&db_path).await.unwrap());
        let persistence = ryuzi_core::agents::bootstrap::AgentPersistence::temporary(store.clone())
            .await
            .unwrap();
        let handles = persistence.handles();
        let cp = ryuzi_core::ControlPlane::new(
            store.clone(),
            ryuzi_core::Registries::new(),
            persistence,
        )
        .await;
        let material = ryuzi_core::tls::load_or_generate(tmp.path()).unwrap();
        let tls_cfg = ryuzi_core::tls::server_config(&material).unwrap();
        let store_handle = store;
        let state = ryuzi_core::serve::ApiState {
            router_server: Arc::new(ryuzi_core::llm_router::server::RouterServer::new(
                store_handle.clone(),
            )),
            cp: cp.clone(),
            agents: handles.registry,
            agent_knowledge: handles.knowledge,
            learning_queue: handles.learning,
            control_token: token.to_string(),
        };
        let opts = ryuzi_core::serve::ServeOpts {
            addr: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            tls: Some(tls_cfg),
        };
        let port = ryuzi_core::serve::serve(state, opts).await.unwrap();
        // Keep the temp dir (and its TLS key material / sqlite file) alive
        // for the lifetime of the test — same pattern `test_server` uses.
        std::mem::forget(tmp);
        (
            format!("https://127.0.0.1:{port}"),
            material.fingerprint,
            store_handle,
        )
    }

    #[tokio::test]
    async fn pair_over_pinned_tls_redeems_a_valid_code() {
        let (base, fingerprint, store) = test_tls_server("tok").await;
        let code = ryuzi_core::pairing::mint_code(&store, 60_000, ryuzi_core::paths::now_ms())
            .await
            .unwrap();

        let token = pair_over_pinned_tls(&base, &fingerprint, &code, "cockpit-test")
            .await
            .expect("a freshly minted code pairs successfully");
        assert_eq!(token.len(), 64);
    }

    #[tokio::test]
    async fn pair_over_pinned_tls_rejects_a_wrong_code() {
        let (base, fingerprint, _store) = test_tls_server("tok").await;

        let err = pair_over_pinned_tls(&base, &fingerprint, "not-a-real-code", "cockpit-test")
            .await
            .unwrap_err();
        assert_eq!(err.message, "invalid or expired pairing code");
    }

    /// The whole point of `fingerprint` pinning: a wrong fingerprint must
    /// reject the TLS handshake itself, before the pairing code is even
    /// sent — distinct from `pair_over_pinned_tls_rejects_a_wrong_code`,
    /// which exercises the correct-pin/wrong-code case.
    #[tokio::test]
    async fn pair_over_pinned_tls_rejects_a_wrong_fingerprint() {
        let (base, _fingerprint, store) = test_tls_server("tok").await;
        let code = ryuzi_core::pairing::mint_code(&store, 60_000, ryuzi_core::paths::now_ms())
            .await
            .unwrap();

        let err = pair_over_pinned_tls(&base, "wrong-fingerprint==", &code, "cockpit-test")
            .await
            .unwrap_err();
        assert!(
            err.message.contains("pairing request failed"),
            "a wrong pin should fail the TLS handshake itself: {}",
            err.message
        );
    }

    // ---------- local daemon base URL ----------

    /// A status file written before `scheme`/`host` existed (or by a daemon
    /// still connecting) must keep resolving to the historical loopback HTTP
    /// endpoint.
    #[test]
    fn base_url_defaults_to_loopback_http_when_the_status_file_says_nothing() {
        assert_eq!(
            super::local_base_url(None, None, 4483),
            "http://127.0.0.1:4483"
        );
    }

    /// `listen_addr = 0.0.0.0` is a documented, user-settable value. The
    /// daemon then binds every interface over TLS, and 0.0.0.0 is not an
    /// address anything can dial — but loopback is part of what that bind
    /// accepts, so Cockpit must still be able to reach its own engine.
    #[test]
    fn base_url_dials_loopback_for_a_wildcard_bind() {
        assert_eq!(
            super::local_base_url(Some("https"), Some("0.0.0.0"), 4483),
            "https://127.0.0.1:4483"
        );
        assert_eq!(
            super::local_base_url(Some("https"), Some("::"), 4483),
            "https://[::1]:4483"
        );
    }

    /// URL *construction* only — do not read this as "a specific non-loopback
    /// bind works". It does not: `serve::authorize` answers 401 to a
    /// non-loopback peer presenting the control token, so such a daemon is
    /// still unattachable from Cockpit (see [`super::local_base_url`]'s doc).
    /// What this pins is that the address is not silently rewritten to
    /// loopback and that an IPv6 literal is bracketed.
    #[test]
    fn base_url_keeps_a_specific_bind_address_and_brackets_ipv6() {
        assert_eq!(
            super::local_base_url(Some("https"), Some("192.168.1.5"), 9000),
            "https://192.168.1.5:9000"
        );
        assert_eq!(
            super::local_base_url(Some("https"), Some("fd00::1"), 9000),
            "https://[fd00::1]:9000"
        );
    }

    #[test]
    fn base_url_falls_back_to_loopback_when_the_host_is_not_an_ip() {
        assert_eq!(
            super::local_base_url(Some("http"), Some("not-an-ip"), 4483),
            "http://127.0.0.1:4483"
        );
    }

    // ---------- control-plane binary resolution ----------

    #[test]
    fn resolver_prefers_the_explicit_override_over_a_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("app");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let sibling = exe_dir.join(super::CONTROL_BIN);
        std::fs::write(&sibling, b"sibling").unwrap();

        let chosen = tmp.path().join("elsewhere-ryuzi");
        std::fs::write(&chosen, b"override").unwrap();

        let found =
            super::resolve_control_binary_in(Some(chosen.clone()), &exe_dir, |_| None).unwrap();
        assert_eq!(found, chosen);
    }

    #[test]
    fn resolver_finds_the_binary_next_to_the_running_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().to_path_buf();
        let sibling = exe_dir.join(super::CONTROL_BIN);
        std::fs::write(&sibling, b"sidecar").unwrap();

        let found = super::resolve_control_binary_in(None, &exe_dir, |_| None).unwrap();
        assert_eq!(found, sibling);
    }

    #[test]
    fn resolver_falls_back_to_path_when_there_is_no_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("empty");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let on_path = tmp.path().join("from-path");
        std::fs::write(&on_path, b"installed").unwrap();

        let expected = on_path.clone();
        let found =
            super::resolve_control_binary_in(None, &exe_dir, move |_| Some(expected)).unwrap();
        assert_eq!(found, on_path);
    }

    /// Cockpit dies with an invisible window when the engine is unreachable
    /// (see `open_daemon_log_creates_the_state_dir_when_it_does_not_exist_yet`
    /// above), so this error is often the only thing a user will ever see
    /// about it. It must name the binary and say how to get one.
    #[test]
    fn resolver_error_names_the_binary_and_how_to_build_it() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("empty");
        std::fs::create_dir_all(&exe_dir).unwrap();

        let err = super::resolve_control_binary_in(None, &exe_dir, |_| None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(super::CONTROL_BIN),
            "message must name the binary: {msg}"
        );
        assert!(
            msg.contains("cargo build -p ryuzi-control"),
            "message must say how to build one: {msg}"
        );
        assert!(
            msg.contains("RYUZI_CONTROL_BIN"),
            "message must mention the override: {msg}"
        );
    }
}
