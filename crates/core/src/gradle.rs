//! Gradle distributions (downloaded on demand) and running builds.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::http::{self, Download};
use crate::{platform, Launcher};

pub fn gradle_home(l: &Launcher, version: &str) -> PathBuf {
    l.paths.gradle().join(format!("gradle-{version}"))
}

pub fn gradle_bin(home: &Path) -> PathBuf {
    home.join("bin").join(platform::script("gradle"))
}

pub async fn ensure_gradle(l: &Launcher, version: &str) -> Result<PathBuf> {
    let home = gradle_home(l, version);
    if gradle_bin(&home).exists() {
        return Ok(home);
    }
    let zip = l.paths.cache().join(format!("gradle-{version}-bin.zip"));
    l.progress("gradle", 0, 1, format!("Downloading Gradle {version}"));
    http::download(
        &l.http,
        &Download {
            url: format!("https://services.gradle.org/distributions/gradle-{version}-bin.zip"),
            path: zip.clone(),
            sha1: None,
            size: None,
        },
    )
    .await
    .with_context(|| format!("downloading Gradle {version}"))?;
    let tmp = l.paths.gradle().join(format!("gradle-{version}.tmp"));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let (z, t) = (zip.clone(), tmp.clone());
    tokio::task::spawn_blocking(move || http::extract_zip(&z, &t, |_| true, true)).await??;
    if !gradle_bin(&tmp).exists() {
        bail!("the Gradle {version} download looks broken");
    }
    tokio::fs::rename(&tmp, &home).await?;
    let _ = tokio::fs::remove_file(&zip).await;
    Ok(home)
}
