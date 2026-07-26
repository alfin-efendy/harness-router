//! Pet catalog RPC family: fetch the petdex manifest (`list_pet_manifest`),
//! download a pet's spritesheet into local state after validating its
//! source host against a strict allowlist (`download_pet`), and serve a
//! previously-downloaded sprite back to the UI as base64 (`get_pet_sprite`).
//!
//! Bundled pets (shipped under `apps/cockpit/public/pets/`) are NEVER
//! served here — the frontend loads those directly from `/pets/`. This
//! module only ever touches `paths::state_dir()/pets/<slug>/sprite.webp`,
//! the downloaded-pet cache.
//!
//! SECURITY: `download_pet` fetches a URL supplied by the caller (sourced
//! from the petdex manifest, itself untrusted network input). Before ANY
//! network request or disk write, the URL is checked against
//! [`is_allowed_pet_host`] — https only, host must equal
//! [`ALLOWED_PET_HOST`] exactly (no subdomain, no userinfo trick). The
//! download client also disables redirect-following entirely
//! (`reqwest::redirect::Policy::none()`), so a compromised or malicious
//! response from the allowlisted host cannot 30x the fetch to an arbitrary
//! off-allowlist origin. `slug` is independently validated by
//! [`sanitize_slug`] against path traversal before it is ever joined onto a
//! filesystem path, in both `download_pet` (write) and `get_pet_sprite`
//! (read).

use super::{ok, params, ApiError};
use crate::api::types::PetManifestEntryInfo;
use crate::serve::ApiState;
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub(crate) const HANDLES: &[&str] = &["list_pet_manifest", "download_pet", "get_pet_sprite"];

/// The only host `download_pet` may fetch a spritesheet from. Petdex serves
/// manifest requests from `petdex.dev` but 307-redirects to this CDN host
/// for the actual asset bytes — `list_pet_manifest`'s client follows that
/// redirect once (a normal, trusted GET to the manifest endpoint itself),
/// so by the time a `spritesheetUrl` reaches `download_pet` it should
/// already point directly here.
const ALLOWED_PET_HOST: &str = "assets.petdex.dev";

const PET_MANIFEST_URL: &str = "https://petdex.dev/api/manifest";

static PET_MANIFEST_CACHE: OnceLock<Mutex<Option<Vec<PetManifestEntryInfo>>>> = OnceLock::new();

fn pet_manifest_cache() -> &'static Mutex<Option<Vec<PetManifestEntryInfo>>> {
    PET_MANIFEST_CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PetManifestResponse {
    #[serde(default)]
    #[allow(dead_code)] // parsed for shape-fidelity, not currently surfaced
    generated_at: i64,
    #[serde(default)]
    #[allow(dead_code)]
    total: u32,
    #[serde(default)]
    pets: Vec<PetManifestEntryInfo>,
}

#[derive(Debug, Deserialize)]
struct SlugP {
    slug: String,
}

#[derive(Debug, Deserialize)]
struct DownloadPetP {
    slug: String,
    spritesheet_url: String,
}

/// Validates `url_str` is `https://assets.petdex.dev/...` — exactly, not a
/// subdomain of it and not merely containing it. `url::Url::host_str()`
/// already resolves userinfo-prefixed URLs
/// (`https://assets.petdex.dev@evil.com/...`) to the real host (`evil.com`
/// here), so no separate userinfo check is needed; this function only has
/// to compare the parsed host verbatim.
fn is_allowed_pet_host(url_str: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url_str).map_err(|e| format!("invalid url: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(format!(
            "unsupported scheme {:?}: only https is allowed",
            parsed.scheme()
        ));
    }
    match parsed.host_str() {
        Some(host) if host == ALLOWED_PET_HOST => Ok(()),
        Some(host) => Err(format!(
            "host {host:?} is not allowed: only {ALLOWED_PET_HOST:?} may be fetched"
        )),
        None => Err("url has no host".to_string()),
    }
}

/// Validates `slug` is safe to join onto a filesystem path: non-empty and
/// restricted to `[a-z0-9-]+`. That charset can never contain `/`, `\`, or
/// `..`, so a slug can never escape `state_dir()/pets/<slug>/` — used by
/// both `download_pet` (write target) and `get_pet_sprite` (read target).
fn sanitize_slug(slug: &str) -> Result<&str, String> {
    if slug.is_empty() {
        return Err("pet slug cannot be empty".to_string());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "invalid pet slug {slug:?}: only lowercase letters, digits, and hyphens are allowed"
        ));
    }
    Ok(slug)
}

fn pet_sprite_path(slug: &str) -> std::path::PathBuf {
    crate::paths::state_dir()
        .join("pets")
        .join(slug)
        .join("sprite.webp")
}

async fn list_pet_manifest() -> Result<Vec<PetManifestEntryInfo>, ApiError> {
    if let Some(cached) = pet_manifest_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Ok(cached);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError {
            status: 500,
            message: e.to_string(),
        })?;
    let resp = client
        .get(PET_MANIFEST_URL)
        .header("User-Agent", "ryuzi")
        .send()
        .await
        .map_err(|e| ApiError {
            status: 502,
            message: format!("pet manifest fetch failed: {e}"),
        })?;
    if !resp.status().is_success() {
        return Err(ApiError {
            status: 502,
            message: format!("pet manifest fetch failed: HTTP {}", resp.status()),
        });
    }
    let body: PetManifestResponse = resp.json().await.map_err(|e| ApiError {
        status: 502,
        message: format!("pet manifest parse failed: {e}"),
    })?;
    *pet_manifest_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(body.pets.clone());
    Ok(body.pets)
}

async fn download_pet(slug: &str, spritesheet_url: &str) -> Result<(), ApiError> {
    let slug = sanitize_slug(slug).map_err(ApiError::bad_request)?;
    is_allowed_pet_host(spritesheet_url).map_err(ApiError::bad_request)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // No redirects: `spritesheet_url` was already validated against the
        // allowlist above, and we never want to trust a 3xx from that host
        // to send us somewhere else — see the module doc's SECURITY note.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ApiError {
            status: 500,
            message: e.to_string(),
        })?;
    let resp = client
        .get(spritesheet_url)
        .header("User-Agent", "ryuzi")
        .send()
        .await
        .map_err(|e| ApiError {
            status: 502,
            message: format!("sprite download failed: {e}"),
        })?;
    if !resp.status().is_success() {
        return Err(ApiError {
            status: 502,
            message: format!("sprite download failed: HTTP {}", resp.status()),
        });
    }
    let bytes = resp.bytes().await.map_err(|e| ApiError {
        status: 502,
        message: format!("sprite download failed: {e}"),
    })?;

    let path = pet_sprite_path(slug);
    let dir = path.parent().expect("sprite path always has a parent");
    std::fs::create_dir_all(dir).map_err(|e| ApiError {
        status: 500,
        message: format!("failed to create pet dir: {e}"),
    })?;
    std::fs::write(&path, &bytes).map_err(|e| ApiError {
        status: 500,
        message: format!("failed to write pet sprite: {e}"),
    })?;
    Ok(())
}

async fn get_pet_sprite(slug: &str) -> Result<Option<String>, ApiError> {
    let slug = sanitize_slug(slug).map_err(ApiError::bad_request)?;
    match std::fs::read(pet_sprite_path(slug)) {
        Ok(bytes) => {
            use base64::Engine as _;
            Ok(Some(
                base64::engine::general_purpose::STANDARD.encode(bytes),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError {
            status: 500,
            message: format!("failed to read pet sprite: {e}"),
        }),
    }
}

pub(crate) async fn dispatch(_state: &ApiState, method: &str, p: Value) -> Result<Value, ApiError> {
    match method {
        "list_pet_manifest" => ok(list_pet_manifest().await?),
        "download_pet" => {
            let a: DownloadPetP = params(p)?;
            download_pet(&a.slug, &a.spritesheet_url).await?;
            ok(())
        }
        "get_pet_sprite" => {
            let a: SlugP = params(p)?;
            ok(get_pet_sprite(&a.slug).await?)
        }
        _ => unreachable!("dispatch is guarded by HANDLES"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests_support::state;

    // --- is_allowed_pet_host ------------------------------------------------

    #[test]
    fn allows_the_exact_cdn_host_over_https() {
        assert!(is_allowed_pet_host("https://assets.petdex.dev/sprout/sprite.webp").is_ok());
    }

    #[test]
    fn rejects_an_unrelated_host() {
        let err = is_allowed_pet_host("https://evil.example/sprite.webp").unwrap_err();
        assert!(err.contains("evil.example"), "{err}");
    }

    #[test]
    fn rejects_a_lookalike_subdomain_suffix_trick() {
        // `assets.petdex.dev` appears as a PREFIX of the host, but the real
        // host is `evil.com` — url::Url::host_str() must return the whole
        // authority host, not the prefix.
        let err =
            is_allowed_pet_host("https://assets.petdex.dev.evil.com/sprite.webp").unwrap_err();
        assert!(err.contains("assets.petdex.dev.evil.com"), "{err}");
    }

    #[test]
    fn rejects_a_genuine_subdomain_of_the_allowed_host() {
        // Only the exact host is allowed — not any subdomain of it, even a
        // superficially plausible one.
        let err = is_allowed_pet_host("https://cdn.assets.petdex.dev/sprite.webp").unwrap_err();
        assert!(err.contains("cdn.assets.petdex.dev"), "{err}");
    }

    #[test]
    fn rejects_a_userinfo_smuggling_trick() {
        // The authority before `@` is userinfo, not the host; the real host
        // here is `evil.com`.
        let err =
            is_allowed_pet_host("https://assets.petdex.dev@evil.com/sprite.webp").unwrap_err();
        assert!(err.contains("evil.com"), "{err}");
    }

    #[test]
    fn rejects_plain_http() {
        let err = is_allowed_pet_host("http://assets.petdex.dev/sprite.webp").unwrap_err();
        assert!(err.contains("http"), "{err}");
    }

    #[test]
    fn rejects_an_unparseable_url() {
        assert!(is_allowed_pet_host("not a url").is_err());
    }

    // --- sanitize_slug -------------------------------------------------------

    #[test]
    fn allows_a_normal_slug() {
        assert_eq!(sanitize_slug("sprout").unwrap(), "sprout");
        assert_eq!(sanitize_slug("paper-clip-9").unwrap(), "paper-clip-9");
    }

    #[test]
    fn rejects_empty_slug() {
        assert!(sanitize_slug("").is_err());
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(sanitize_slug("../../etc/passwd").is_err());
        assert!(sanitize_slug("..").is_err());
    }

    #[test]
    fn rejects_path_separators() {
        assert!(sanitize_slug("a/b").is_err());
        assert!(sanitize_slug("a\\b").is_err());
    }

    #[test]
    fn rejects_uppercase_and_other_disallowed_characters() {
        assert!(sanitize_slug("Sprout").is_err());
        assert!(sanitize_slug("sprout.webp").is_err());
        assert!(sanitize_slug("sprout ").is_err());
        assert!(sanitize_slug("sprout/../..").is_err());
    }

    // --- download_pet: rejection never touches disk --------------------------

    #[tokio::test]
    async fn download_pet_rejects_a_non_allowlisted_host_before_any_io() {
        // Host validation runs before the network fetch or any filesystem
        // write, so this errors out having never touched disk or network.
        let err = download_pet("evil-slug", "https://evil.example/sprite.webp")
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert!(err.message.contains("evil.example"), "{}", err.message);
        // The rejection path never calls `paths::state_dir()`-based I/O, so
        // no directory was created for this slug.
        assert!(!pet_sprite_path("evil-slug").exists());
    }

    #[tokio::test]
    async fn download_pet_rejects_an_invalid_slug_before_any_io() {
        let err = download_pet("../escape", "https://assets.petdex.dev/x/sprite.webp")
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert!(err.message.contains("invalid pet slug"), "{}", err.message);
    }

    // --- get_pet_sprite --------------------------------------------------------

    #[tokio::test]
    async fn get_pet_sprite_is_none_when_absent() {
        let result = get_pet_sprite("definitely-not-a-real-downloaded-pet-slug-xyz")
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn get_pet_sprite_rejects_an_invalid_slug() {
        let err = get_pet_sprite("../../etc/passwd").await.unwrap_err();
        assert_eq!(err.status, 400);
    }

    // --- manifest wire-shape parsing (no network) -----------------------------

    #[test]
    fn manifest_response_parses_camel_case_and_ignores_unknown_per_pet_fields() {
        let raw = serde_json::json!({
            "generatedAt": 1_700_000_000_i64,
            "total": 1,
            "pets": [{
                "slug": "sprout",
                "displayName": "Sprout",
                "kind": "bundled",
                "submittedBy": null,
                "spritesheetUrl": "https://assets.petdex.dev/sprout/sprite.webp",
                "petJsonUrl": "https://assets.petdex.dev/sprout/pet.json",
                "zipUrl": "https://assets.petdex.dev/sprout/sprout.zip"
            }]
        });
        let parsed: PetManifestResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.total, 1);
        assert_eq!(parsed.pets.len(), 1);
        let entry = &parsed.pets[0];
        assert_eq!(entry.slug, "sprout");
        assert_eq!(entry.display_name, "Sprout");
        assert_eq!(entry.kind, "bundled");
        assert_eq!(entry.submitted_by, None);
        assert_eq!(
            entry.spritesheet_url,
            "https://assets.petdex.dev/sprout/sprite.webp"
        );
    }

    // --- dispatch wiring ---------------------------------------------------

    #[tokio::test]
    async fn dispatch_rejects_a_bad_host_through_the_rpc_surface() {
        let state = state().await;
        // Exercise the top-level RPC router (`crate::api::dispatch`), not
        // this module's own `dispatch`, to prove `pets_api` is actually
        // registered in `api::mod`'s dispatch table.
        let err = crate::api::dispatch(
            &state,
            "download_pet",
            serde_json::json!({"slug": "evil", "spritesheet_url": "https://evil.example/x.webp"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[tokio::test]
    async fn dispatch_get_pet_sprite_returns_null_when_absent() {
        let state = state().await;
        let value = crate::api::dispatch(
            &state,
            "get_pet_sprite",
            serde_json::json!({"slug": "no-such-pet-slug-abc"}),
        )
        .await
        .unwrap();
        assert_eq!(value, Value::Null);
    }
}
