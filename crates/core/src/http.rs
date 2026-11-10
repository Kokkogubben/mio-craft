use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::Launcher;

pub const USER_AGENT: &str = concat!("ClaudeLauncher/", env!("CARGO_PKG_VERSION"));

pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()?)
}

pub async fn get_json<T: DeserializeOwned>(http: &reqwest::Client, url: &str) -> Result<T> {
    let resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} returned {}", resp.status());
    }
    resp.json::<T>()
        .await
        .with_context(|| format!("parsing JSON from {url}"))
}

pub async fn get_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} returned {}", resp.status());
    }
    Ok(resp.text().await?)
}

/// Write JSON atomically (temp file + rename) so a crash never leaves half a file.
pub async fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(value)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

pub fn sha1_file_sync(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

/// True when the file already exists with the right hash (or size, if no hash).
pub async fn is_current(d: &Download) -> bool {
    let Ok(meta) = tokio::fs::metadata(&d.path).await else {
        return false;
    };
    if let Some(size) = d.size {
        if meta.len() != size {
            return false;
        }
    }
    match &d.sha1 {
        Some(expected) => {
            let p = d.path.clone();
            let expected = expected.to_lowercase();
            tokio::task::spawn_blocking(move || sha1_file_sync(&p).map(|h| h == expected))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(false)
        }
        None => meta.len() > 0,
    }
}

pub async fn download(http: &reqwest::Client, d: &Download) -> Result<()> {
    if is_current(d).await {
        return Ok(());
    }
    let mut last_err = None;
    for attempt in 0..3 {
        match download_once(http, d).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
            }
        }
    }
    Err(last_err.unwrap())
}

async fn download_once(http: &reqwest::Client, d: &Download) -> Result<()> {
    if let Some(parent) = d.path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let resp = http
        .get(&d.url)
        .send()
        .await
        .with_context(|| format!("GET {}", d.url))?;
    if !resp.status().is_success() {
        bail!("GET {} returned {}", d.url, resp.status());
    }
    let tmp = d.path.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha1::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if let Some(expected) = &d.sha1 {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(&tmp).await;
            bail!("checksum mismatch for {} (expected {expected}, got {got})", d.url);
        }
    }
    tokio::fs::rename(&tmp, &d.path).await?;
    Ok(())
}

/// Download many files in parallel, reporting progress under `task`.
pub async fn download_many(launcher: &Launcher, task: &str, items: Vec<Download>) -> Result<()> {
    let total = items.len() as u64;
    if total == 0 {
        return Ok(());
    }
    let done = Arc::new(AtomicU64::new(0));
    launcher.progress(task, 0, total, format!("Downloading {total} files"));
    let results: Vec<Result<()>> = futures::stream::iter(items)
        .map(|d| {
            let http = launcher.http.clone();
            let done = done.clone();
            let launcher = launcher.clone();
            let task = task.to_string();
            async move {
                let r = download(&http, &d).await;
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 25 == 0 || n == total {
                    launcher.progress(&task, n, total, format!("Downloaded {n}/{total}"));
                }
                r
            }
        })
        .buffer_unordered(16)
        .collect()
        .await;
    let errors: Vec<_> = results.into_iter().filter_map(|r| r.err()).collect();
    if !errors.is_empty() {
        bail!(
            "{} of {total} downloads failed; first error: {:#}",
            errors.len(),
            errors[0]
        );
    }
    Ok(())
}

/// Extract a zip archive. Entries are filtered by `keep` (given the entry name).
pub fn extract_zip(
    archive: &Path,
    dest: &Path,
    keep: impl Fn(&str) -> bool,
    strip_first_dir: bool,
) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let name_str = name.to_string_lossy().replace('\\', "/");
        if !keep(&name_str) {
            continue;
        }
        let rel: PathBuf = if strip_first_dir {
            let mut comps = name.components();
            comps.next();
            comps.as_path().to_path_buf()
        } else {
            name.to_path_buf()
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// Extract a .tar.gz, dropping the first directory level.
pub fn extract_tar_gz_strip(archive: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(archive)?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let mut comps = path.components();
        comps.next();
        let rel = comps.as_path();
        if rel.as_os_str().is_empty()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let out = dest.join(rel);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}
