//! The player's outfit: changing the skin and cape through the Minecraft
//! profile API, and a local library of saved skins to switch between.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::auth;
use crate::scene::base64;
use crate::{http, Launcher};

/// A cape the account owns.
#[derive(Debug, Clone, Serialize)]
pub struct Cape {
    pub id: String,
    /// Mojang's name for it ("Migrator", "Pan"…).
    pub alias: String,
    pub active: bool,
    /// The cape texture as base64 PNG, or empty if it couldn't be downloaded.
    pub png: String,
}

/// What the account is wearing, and the capes it can pick from.
#[derive(Debug, Clone, Serialize)]
pub struct Outfit {
    /// The current skin as base64 PNG.
    pub skin: String,
    pub slim: bool,
    pub capes: Vec<Cape>,
}

/// A skin saved in the launcher's library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSkin {
    pub id: String,
    pub name: String,
    pub slim: bool,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub added_at: u64,
    /// Filled in for the API; not stored in the index.
    #[serde(default, skip_deserializing, skip_serializing_if = "String::is_empty")]
    pub png: String,
}

const MAX_SKIN_BYTES: usize = 256 * 1024;
const MAX_LIBRARY: usize = 200;
const MAX_NAME: usize = 40;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Check that `png` is a skin Minecraft accepts: a PNG of 64×64, or the old 64×32.
pub fn check_skin(png: &[u8]) -> Result<()> {
    if png.len() > MAX_SKIN_BYTES {
        bail!("That image is too big for a skin");
    }
    if png.len() < 24 || !png.starts_with(b"\x89PNG\r\n\x1a\n") || &png[12..16] != b"IHDR" {
        bail!("A skin has to be a PNG image");
    }
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if !(w == 64 && (h == 64 || h == 32)) {
        bail!("A skin has to be 64×64 pixels (or the old 64×32), but this image is {w}×{h}");
    }
    Ok(())
}

async fn account(l: &Launcher, id: &str) -> Result<auth::Account> {
    let acc = auth::usable_account(l, Some(id)).await?;
    if acc.id != id {
        bail!("No signed-in account with ID {id}");
    }
    Ok(acc)
}

/// Turn a failed profile API call into a message for the player.
async fn api_error(resp: reqwest::Response, what: &str) -> anyhow::Error {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let detail = body["errorMessage"].as_str().or(body["error"].as_str()).unwrap_or("").to_string();
    match status.as_u16() {
        429 => anyhow!("Minecraft only allows a few outfit changes a minute. Wait a moment and try again."),
        401 | 403 => anyhow!("Minecraft didn't accept the sign-in. Sign in again and retry."),
        _ if detail.is_empty() => anyhow!("Couldn't {what} ({status})"),
        _ => anyhow!("Couldn't {what}: {detail}"),
    }
}

/// The account's skin and capes, read from Minecraft. Cape images are cached,
/// since they never change.
pub async fn outfit(l: &Launcher, id: &str) -> Result<Outfit> {
    let acc = account(l, id).await?;
    let resp = l.http.get(auth::profile_api()).bearer_auth(&acc.mc_token).send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp, "read your Minecraft profile").await);
    }
    let profile: serde_json::Value = resp.json().await?;
    outfit_from_profile(l, id, &profile).await
}

async fn outfit_from_profile(l: &Launcher, id: &str, profile: &serde_json::Value) -> Result<Outfit> {
    let skin = match auth::skin_from_profile(l, profile).await {
        Ok(s) => {
            auth::cache_skin(l, id, &s).await?;
            s
        }
        Err(e) => auth::cached_skin(l, id).await.ok_or(e)?,
    };
    let mut capes = vec![];
    for c in profile["capes"].as_array().into_iter().flatten() {
        let Some(cid) = c["id"].as_str().filter(|s| valid_cape_id(s)) else {
            continue;
        };
        let png = match c["url"].as_str().and_then(auth::texture_url) {
            Some(url) => cape_png(l, cid, &url).await.map(|b| base64(&b)).unwrap_or_default(),
            None => String::new(),
        };
        capes.push(Cape {
            id: cid.to_string(),
            alias: c["alias"].as_str().unwrap_or("Cape").to_string(),
            active: c["state"] == "ACTIVE",
            png,
        });
    }
    Ok(Outfit { skin: base64(&skin.png), slim: skin.slim, capes })
}

pub(crate) fn valid_cape_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The image of the cape the account wore when last seen (from the cache).
pub async fn worn_cape(l: &Launcher, id: &str) -> Result<Vec<u8>> {
    let cape = auth::cached_skin(l, id).await.and_then(|s| s.cape).ok_or_else(|| anyhow!("No cape"))?;
    if !valid_cape_id(&cape) {
        bail!("No cape");
    }
    Ok(tokio::fs::read(l.paths.cache().join("capes").join(format!("{cape}.png"))).await?)
}

pub(crate) async fn cape_png(l: &Launcher, cape_id: &str, url: &str) -> Result<Vec<u8>> {
    let path = l.paths.cache().join("capes").join(format!("{cape_id}.png"));
    if let Ok(b) = tokio::fs::read(&path).await {
        return Ok(b);
    }
    let png = l.http.get(url).send().await?.error_for_status()?.bytes().await?.to_vec();
    if !png.starts_with(b"\x89PNG") {
        bail!("The cape from Mojang isn't a PNG image");
    }
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(&path, &png).await?;
    Ok(png)
}

/// Upload a new skin. The skin being replaced is kept in the library first,
/// so changing never loses one.
pub async fn set_skin(l: &Launcher, id: &str, png: Vec<u8>, slim: bool) -> Result<Outfit> {
    check_skin(&png)?;
    let acc = account(l, id).await?;
    if let Some(old) = auth::cached_skin(l, id).await {
        if old.png != png && check_skin(&old.png).is_ok() {
            let _ = save(l, "Previous skin", old.slim, &old.png).await;
        }
    }
    let form = reqwest::multipart::Form::new()
        .text("variant", if slim { "slim" } else { "classic" })
        .part("file", reqwest::multipart::Part::bytes(png).file_name("skin.png").mime_str("image/png")?);
    let resp = l.http.post(format!("{}/skins", auth::profile_api())).bearer_auth(&acc.mc_token).multipart(form).send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp, "change your skin").await);
    }
    after_change(l, id, resp).await
}

/// Go back to one of Minecraft's default skins.
pub async fn reset_skin(l: &Launcher, id: &str) -> Result<Outfit> {
    let acc = account(l, id).await?;
    if let Some(old) = auth::cached_skin(l, id).await {
        if check_skin(&old.png).is_ok() {
            let _ = save(l, "Previous skin", old.slim, &old.png).await;
        }
    }
    let resp = l.http.delete(format!("{}/skins/active", auth::profile_api())).bearer_auth(&acc.mc_token).send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp, "reset your skin").await);
    }
    after_change(l, id, resp).await
}

/// Wear one of the account's capes, or none.
pub async fn set_cape(l: &Launcher, id: &str, cape: Option<&str>) -> Result<Outfit> {
    let acc = account(l, id).await?;
    let url = format!("{}/capes/active", auth::profile_api());
    let req = match cape {
        Some(c) => l.http.put(url).json(&serde_json::json!({ "capeId": c })),
        None => l.http.delete(url),
    };
    let resp = req.bearer_auth(&acc.mc_token).send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp, "change your cape").await);
    }
    after_change(l, id, resp).await
}

/// The changes return the updated profile; read the outfit from it, or ask again.
async fn after_change(l: &Launcher, id: &str, resp: reqwest::Response) -> Result<Outfit> {
    match resp.json::<serde_json::Value>().await {
        Ok(p) if p["skins"].is_array() => outfit_from_profile(l, id, &p).await,
        _ => outfit(l, id).await,
    }
}

// ---------- the skin library ----------

fn library_dir(l: &Launcher) -> std::path::PathBuf {
    l.paths.root.join("skins")
}

async fn load_index(l: &Launcher) -> Vec<SavedSkin> {
    http::read_json(&library_dir(l).join("library.json")).await.unwrap_or_default()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Saved skins, newest first, with their images.
pub async fn library(l: &Launcher) -> Vec<SavedSkin> {
    let dir = library_dir(l);
    let mut out = vec![];
    for mut s in load_index(l).await {
        if !valid_id(&s.id) {
            continue;
        }
        if let Ok(b) = tokio::fs::read(dir.join(format!("{}.png", s.id))).await {
            s.png = base64(&b);
            out.push(s);
        }
    }
    out.sort_by(|a, b| b.added_at.cmp(&a.added_at));
    out
}

/// Save a skin to the library. The same image saved twice is kept once
/// (the name and model are updated).
pub async fn save(l: &Launcher, name: &str, slim: bool, png: &[u8]) -> Result<SavedSkin> {
    check_skin(png)?;
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let name = if name.is_empty() { "Skin".to_string() } else { name };
    let sha = hex::encode(Sha1::digest(png));
    let dir = library_dir(l);
    tokio::fs::create_dir_all(&dir).await?;
    let mut index = load_index(l).await;
    if let Some(s) = index.iter_mut().find(|s| s.sha1 == sha) {
        // Keep a name the player gave; only replace the automatic one.
        if s.name == "Previous skin" || name != "Previous skin" {
            s.name = name;
        }
        s.slim = slim;
        let out = s.clone();
        http::write_json(&dir.join("library.json"), &index).await?;
        return Ok(out);
    }
    if index.len() >= MAX_LIBRARY {
        bail!("The skin library is full ({MAX_LIBRARY} skins). Delete some first.");
    }
    let entry = SavedSkin { id: uuid::Uuid::new_v4().simple().to_string(), name, slim, sha1: sha, added_at: now(), png: String::new() };
    tokio::fs::write(dir.join(format!("{}.png", entry.id)), png).await.context("saving the skin")?;
    index.push(entry.clone());
    http::write_json(&dir.join("library.json"), &index).await?;
    Ok(entry)
}

/// Rename a saved skin or change its model.
pub async fn update(l: &Launcher, id: &str, name: Option<&str>, slim: Option<bool>) -> Result<()> {
    let mut index = load_index(l).await;
    let s = index.iter_mut().find(|s| s.id == id).ok_or_else(|| anyhow!("No saved skin {id}"))?;
    if let Some(n) = name {
        let n: String = n.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        if !n.is_empty() {
            s.name = n;
        }
    }
    if let Some(v) = slim {
        s.slim = v;
    }
    http::write_json(&library_dir(l).join("library.json"), &index).await
}

pub async fn delete(l: &Launcher, id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("No saved skin {id}");
    }
    let mut index = load_index(l).await;
    let before = index.len();
    index.retain(|s| s.id != id);
    if index.len() == before {
        bail!("No saved skin {id}");
    }
    http::write_json(&library_dir(l).join("library.json"), &index).await?;
    let _ = tokio::fs::remove_file(library_dir(l).join(format!("{id}.png"))).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    /// A minimal PNG header with the given size (enough for `check_skin`).
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn checks_skin_size() {
        assert!(check_skin(&png(64, 64)).is_ok());
        assert!(check_skin(&png(64, 32)).is_ok());
        assert!(check_skin(&png(128, 128)).unwrap_err().to_string().contains("128×128"));
        assert!(check_skin(b"GIF89a not a png at all......").is_err());
        assert!(check_skin(&[]).is_err());
    }

    /// Against a fake Minecraft API: read the outfit, upload a skin (the old one
    /// lands in the library), and wear or take off a cape.
    #[tokio::test]
    async fn changes_skin_and_cape_on_a_fake_api() {
        use axum::{body::Bytes, http::HeaderMap, routing::{delete as del, get}, Json, Router};
        use serde_json::json;
        use std::sync::{Arc, Mutex};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        // (skin png, slim, active cape)
        let state = Arc::new(Mutex::new((png(64, 64), false, Some("c1".to_string()))));
        let profile = {
            let base = base.clone();
            move |st: &(Vec<u8>, bool, Option<String>)| {
                json!({ "id": "abc", "name": "Mio",
                    "skins": [{ "id": "s", "state": "ACTIVE", "url": format!("{base}/tex/skin"), "variant": if st.1 { "SLIM" } else { "CLASSIC" } }],
                    "capes": [
                        { "id": "c1", "state": if st.2.as_deref() == Some("c1") { "ACTIVE" } else { "INACTIVE" }, "url": format!("{base}/tex/c1"), "alias": "Migrator" },
                        { "id": "c2", "state": if st.2.as_deref() == Some("c2") { "ACTIVE" } else { "INACTIVE" }, "url": "https://evil.example/c2.png", "alias": "Pan" },
                    ]})
            }
        };
        let auth_ok = |h: &HeaderMap| h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer fake-mc");
        let app = Router::new()
            .route("/", get({ let (s, p) = (state.clone(), profile.clone()); move |h: HeaderMap| async move {
                assert!(auth_ok(&h));
                Json(p(&s.lock().unwrap()))
            }}))
            .route("/tex/skin", get({ let s = state.clone(); move || async move { s.lock().unwrap().0.clone() } }))
            .route("/tex/c1", get(|| async { png(64, 32) }))
            .route("/skins", axum::routing::post({ let (s, p) = (state.clone(), profile.clone()); move |h: HeaderMap, body: Bytes| async move {
                assert!(auth_ok(&h));
                let ct = h.get("content-type").unwrap().to_str().unwrap().to_string();
                assert!(ct.starts_with("multipart/form-data"), "{ct}");
                let text = String::from_utf8_lossy(&body).to_string();
                assert!(text.contains("name=\"variant\"") && text.contains("name=\"file\"; filename=\"skin.png\""), "{text}");
                let slim = text.contains("\r\n\r\nslim\r\n");
                // The uploaded PNG is the part after the file headers.
                let start = body.windows(4).position(|w| w == b"\x89PNG").unwrap();
                let mut st = s.lock().unwrap();
                st.0 = body[start..start + png(1, 1).len()].to_vec();
                st.1 = slim;
                Json(p(&st))
            }}))
            .route("/capes/active", axum::routing::put({ let (s, p) = (state.clone(), profile.clone()); move |Json(b): Json<serde_json::Value>| async move {
                let mut st = s.lock().unwrap();
                st.2 = b["capeId"].as_str().map(String::from);
                Json(p(&st))
            }}).merge(del({ let (s, p) = (state.clone(), profile.clone()); move || async move {
                let mut st = s.lock().unwrap();
                st.2 = None;
                Json(p(&st))
            }})));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        std::env::set_var("MIO_CRAFT_MC_API", &base);

        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let l = Launcher(std::sync::Arc::new(crate::Inner {
            paths: l.paths.clone(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            events: l.events.clone(),
            config: tokio::sync::RwLock::new(Default::default()),
            processes: Default::default(),
            install_lock: tokio::sync::Mutex::new(()),
        }));
        // A placeholder token for the fake server only.
        std::fs::write(l.paths.accounts_file(), json!([{ "id": "abc", "name": "Mio", "mc_token": "fake-mc", "mc_expires_at": now() + 3600 }]).to_string()).unwrap();

        let o = outfit(&l, "abc").await.unwrap();
        assert!(!o.slim);
        assert_eq!(o.capes.len(), 2);
        assert!(o.capes[0].active && !o.capes[0].png.is_empty());
        assert!(o.capes[1].png.is_empty(), "never fetched from another host");
        assert_eq!(auth::cached_skin(&l, "abc").await.unwrap().cape.as_deref(), Some("c1"));
        assert_eq!(worn_cape(&l, "abc").await.unwrap(), png(64, 32));

        let o = set_skin(&l, "abc", png(64, 32), true).await.unwrap();
        assert!(o.slim);
        assert_eq!(o.skin, base64(&png(64, 32)));
        let lib = library(&l).await;
        assert_eq!(lib.len(), 1, "the old skin was kept");
        assert_eq!(lib[0].name, "Previous skin");
        assert_eq!(lib[0].png, base64(&png(64, 64)));
        assert!(set_skin(&l, "abc", png(32, 32), false).await.is_err());

        let o = set_cape(&l, "abc", Some("c2")).await.unwrap();
        assert!(o.capes[1].active && !o.capes[0].active);
        let o = set_cape(&l, "abc", None).await.unwrap();
        assert!(o.capes.iter().all(|c| !c.active));
        assert!(auth::cached_skin(&l, "abc").await.unwrap().cape.is_none());
        assert!(worn_cape(&l, "abc").await.is_err());
    }

    #[tokio::test]
    async fn library_saves_dedupes_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let a = save(&l, "Knight", false, &png(64, 64)).await.unwrap();
        // The same image again keeps one entry; an automatic name doesn't replace a chosen one.
        let again = save(&l, "Previous skin", true, &png(64, 64)).await.unwrap();
        assert_eq!(again.id, a.id);
        assert_eq!(again.name, "Knight");
        assert!(again.slim);
        let b = save(&l, "  Old\none  ", false, &png(64, 32)).await.unwrap();
        assert_eq!(b.name, "Oldone");
        let list = library(&l).await;
        assert_eq!(list.len(), 2);
        assert!(!list[0].png.is_empty());
        update(&l, &a.id, Some("Paladin"), Some(false)).await.unwrap();
        assert!(library(&l).await.iter().any(|s| s.name == "Paladin" && !s.slim));
        delete(&l, &a.id).await.unwrap();
        assert_eq!(library(&l).await.len(), 1);
        assert!(delete(&l, "../x").await.is_err());
        assert!(save(&l, "Big", false, &png(128, 128)).await.is_err());
    }
}
