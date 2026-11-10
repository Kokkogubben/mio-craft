//! Java runtimes. The launcher downloads full JDKs from Eclipse Adoptium
//! (Temurin) because the same runtime is used both to play and to compile mods.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::http::{self, Download};
use crate::{platform, Launcher};

pub fn jdk_home(l: &Launcher, major: u32) -> PathBuf {
    l.paths.java().join(format!("jdk-{major}"))
}

pub fn java_bin(home: &Path) -> PathBuf {
    home.join("bin").join(platform::exe("java"))
}

/// The JDK major version used to run Gradle (independent of the mod's target).
pub fn gradle_jdk_for(target_major: u32) -> u32 {
    target_major.max(21)
}

/// Make sure a JDK of the given major version is installed; return its home.
pub async fn ensure_jdk(l: &Launcher, major: u32) -> Result<PathBuf> {
    let home = jdk_home(l, major);
    if java_bin(&home).exists() {
        return Ok(home);
    }
    let os = platform::adoptium_os();
    let mut arch = platform::adoptium_arch();
    // Temurin has no Apple-silicon builds of Java 8; use the Intel one via Rosetta.
    if major == 8 && os == "mac" && arch == "aarch64" {
        arch = "x64";
    }
    let url = format!(
        "https://api.adoptium.net/v3/binary/latest/{major}/ga/{os}/{arch}/jdk/hotspot/normal/eclipse"
    );
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    let archive = l.paths.cache().join(format!("jdk-{major}.{ext}"));
    l.progress("java", 0, 1, format!("Downloading Java {major}"));
    let _ = tokio::fs::remove_file(&archive).await;
    http::download(
        &l.http,
        &Download { url, path: archive.clone(), sha1: None, size: None },
    )
    .await
    .with_context(|| format!("downloading Java {major}"))?;

    l.progress("java", 0, 1, format!("Unpacking Java {major}"));
    let tmp = l.paths.java().join(format!("jdk-{major}.tmp"));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await?;
    let (a, t) = (archive.clone(), tmp.clone());
    tokio::task::spawn_blocking(move || {
        if a.extension().and_then(|e| e.to_str()) == Some("zip") {
            http::extract_zip(&a, &t, |_| true, true)
        } else {
            http::extract_tar_gz_strip(&a, &t)
        }
    })
    .await??;

    // macOS JDKs nest the real home under Contents/Home.
    let real_home = if tmp.join("Contents/Home").exists() {
        tmp.join("Contents/Home")
    } else {
        tmp.clone()
    };
    if !java_bin(&real_home).exists() {
        bail!("the Java {major} download did not contain a java executable");
    }
    let _ = tokio::fs::remove_dir_all(&home).await;
    if real_home == tmp {
        tokio::fs::rename(&tmp, &home).await?;
    } else {
        tokio::fs::rename(&real_home, &home).await?;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
    let _ = tokio::fs::remove_file(&archive).await;
    l.progress("java", 1, 1, format!("Java {major} ready"));
    Ok(home)
}

/// JetBrains Runtime, pinned. Builds are listed at
/// https://github.com/JetBrains/JetBrainsRuntime/releases.
pub const JBR_25: (&str, &str) = ("25.0.4", "b508.27");

/// JetBrains Runtime (needed for enhanced class redefinition) for a Java major.
pub async fn ensure_jbr(l: &Launcher, major: u32) -> Result<PathBuf> {
    let (ver, build) = match major {
        0..=25 => JBR_25,
        _ => bail!("Live hotswap has no JetBrains Runtime for Java {major} yet; update JBR_25 in java.rs"),
    };
    let home = l.paths.java().join(format!("jbr-{ver}"));
    if java_bin(&home).exists() {
        return Ok(home);
    }
    let os = match platform::mojang_os() {
        "osx" => "osx",
        o => o,
    };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    let url = format!("https://cache-redirector.jetbrains.com/intellij-jbr/jbr-{ver}-{os}-{arch}-{build}.{ext}");
    let archive = l.paths.cache().join(format!("jbr-{ver}.{ext}"));
    l.progress("java", 0, 1, "Downloading JetBrains Runtime for live hotswap");
    http::download(&l.http, &Download { url, path: archive.clone(), sha1: None, size: None })
        .await
        .context("downloading JetBrains Runtime")?;
    let tmp = l.paths.java().join(format!("jbr-{ver}.tmp"));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await?;
    let (a, t) = (archive.clone(), tmp.clone());
    tokio::task::spawn_blocking(move || {
        if a.extension().and_then(|e| e.to_str()) == Some("zip") {
            http::extract_zip(&a, &t, |_| true, true)
        } else {
            http::extract_tar_gz_strip(&a, &t)
        }
    })
    .await??;
    let real = if tmp.join("Contents/Home").exists() { tmp.join("Contents/Home") } else { tmp.clone() };
    if !java_bin(&real).exists() {
        bail!("the JetBrains Runtime download didn't contain java");
    }
    let _ = tokio::fs::remove_dir_all(&home).await;
    tokio::fs::rename(&real, &home).await?;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let _ = tokio::fs::remove_file(&archive).await;
    l.progress("java", 1, 1, "JetBrains Runtime ready");
    Ok(home)
}

/// Java major version required by a Minecraft version JSON (8 if unspecified).
pub fn required_major(v: &crate::mojang::VersionJson) -> u32 {
    v.java_version.as_ref().map(|j| j.major_version).unwrap_or(8)
}
