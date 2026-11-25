//! Checks an instance's mods without starting the game: required mods that are
//! missing, mods made for another loader, and the same mod installed twice.
//!
//! Only what the jars declare is checked. Version ranges aren't (a test launch
//! finds those), and mods bundled inside other jars count as installed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Cursor, Read, Seek};

use anyhow::Result;
use serde::Serialize;

use crate::loaders::Loader;
use crate::{instances, Launcher};

/// Dependencies the loader or the game itself provides.
const BUILT_IN: &[&str] = &[
    "minecraft", "java", "fabricloader", "fabric-loader", "quilt_loader", "quilt_base", "neoforge", "forge", "javafml", "lowcodefml", "mixinextras",
];
const MAX_NESTED: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    MissingDependency,
    WrongLoader,
    Duplicate,
}

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub kind: Kind,
    /// The jar with the problem.
    pub file: String,
    pub name: String,
    /// The missing mod id, the jar's loader, or the other copy.
    pub detail: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checked: usize,
    pub problems: Vec<Problem>,
}

/// What one jar declares.
#[derive(Debug, Default)]
struct Jar {
    name: Option<String>,
    /// Top-level mod ids (a NeoForge jar can hold several).
    ids: Vec<String>,
    /// Ids this jar makes available: its own, `provides` aliases and bundled jars.
    provides: BTreeSet<String>,
    requires: BTreeSet<String>,
    /// Loaders the jar has a manifest for.
    loaders: BTreeSet<&'static str>,
}

fn read<R: Read + Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn strs(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from).or_else(|| x["id"].as_str().map(String::from))).collect())
        .unwrap_or_default()
}

/// Read a jar's manifests, and those of the jars bundled in it.
fn scan<R: Read + Seek>(z: &mut zip::ZipArchive<R>, out: &mut Jar, top: bool, depth: u32) {
    let mut nested: Vec<String> = vec![];
    if let Some(j) = read(z, "fabric.mod.json").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        if let Some(id) = j["id"].as_str() {
            out.provides.insert(id.into());
            if top {
                out.ids.push(id.into());
                out.name = out.name.take().or_else(|| j["name"].as_str().map(String::from));
                out.loaders.insert("fabric");
                if let Some(d) = j["depends"].as_object() {
                    out.requires.extend(d.keys().cloned());
                }
            }
        }
        out.provides.extend(strs(&j["provides"]));
        nested.extend(j["jars"].as_array().into_iter().flatten().filter_map(|x| x["file"].as_str().map(String::from)));
    }
    if let Some(j) = read(z, "quilt.mod.json").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        let q = &j["quilt_loader"];
        if let Some(id) = q["id"].as_str() {
            out.provides.insert(id.into());
            if top {
                out.ids.push(id.into());
                out.name = out.name.take().or_else(|| q["metadata"]["name"].as_str().map(String::from));
                out.loaders.insert("quilt");
                for d in q["depends"].as_array().into_iter().flatten() {
                    let optional = d["optional"].as_bool().unwrap_or(false);
                    if let Some(id) = d.as_str().or_else(|| d["id"].as_str()).filter(|_| !optional) {
                        // Quilt ids may carry a maven group: "org.quiltmc:qsl".
                        out.requires.insert(id.rsplit(':').next().unwrap_or(id).into());
                    }
                }
            }
        }
        out.provides.extend(strs(&q["provides"]));
        nested.extend(strs(&q["jars"]));
    }
    for (file, loader) in [("META-INF/neoforge.mods.toml", "neoforge"), ("META-INF/mods.toml", "forge")] {
        let Some(t) = read(z, file).and_then(|s| toml::from_str::<toml::Value>(&s).ok()) else { continue };
        let mods: Vec<String> = t.get("mods").and_then(|m| m.as_array()).into_iter().flatten().filter_map(|m| m.get("modId")?.as_str().map(String::from)).collect();
        out.provides.extend(mods.iter().cloned());
        if top {
            out.loaders.insert(loader);
            out.name = out.name.take().or_else(|| t.get("mods")?.as_array()?.first()?.get("displayName")?.as_str().map(String::from));
            for id in &mods {
                if !out.ids.contains(id) {
                    out.ids.push(id.clone());
                }
            }
            if let Some(deps) = t.get("dependencies").and_then(|d| d.as_table()) {
                for list in deps.values().filter_map(|v| v.as_array()) {
                    for d in list {
                        let required = match d.get("type").and_then(|x| x.as_str()) {
                            Some(t) => t.eq_ignore_ascii_case("required"),
                            None => d.get("mandatory").and_then(|x| x.as_bool()).unwrap_or(false),
                        };
                        let side_ok = d.get("side").and_then(|x| x.as_str()).is_none_or(|s| !s.eq_ignore_ascii_case("SERVER"));
                        if let (true, true, Some(id)) = (required, side_ok, d.get("modId").and_then(|x| x.as_str())) {
                            out.requires.insert(id.into());
                        }
                    }
                }
            }
        }
    }
    // NeoForge / Forge bundle jars here.
    nested.extend((0..z.len()).filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string())).filter(|n| n.starts_with("META-INF/jarjar/") && n.ends_with(".jar")));
    if depth >= 3 {
        return;
    }
    for name in nested {
        let Ok(mut f) = z.by_name(&name) else { continue };
        if f.size() > MAX_NESTED {
            continue;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        if f.read_to_end(&mut buf).is_err() {
            continue;
        }
        drop(f);
        if let Ok(mut inner) = zip::ZipArchive::new(Cursor::new(buf)) {
            scan(&mut inner, out, false, depth + 1);
        }
    }
}

fn read_jar(path: &std::path::Path) -> Option<Jar> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let mut j = Jar::default();
    scan(&mut z, &mut j, true, 0);
    Some(j)
}

/// Can this loader run a jar made for these loaders? Unknown jars (no manifest) pass.
fn loader_ok(instance: Loader, jar: &BTreeSet<&'static str>) -> bool {
    if jar.is_empty() {
        return true;
    }
    let accepts: &[&str] = match instance {
        Loader::Fabric | Loader::LegacyFabric => &["fabric"],
        Loader::Quilt => &["quilt", "fabric"],
        // NeoForge for 1.20.1 still reads Forge's mods.toml.
        Loader::Neoforge => &["neoforge", "forge"],
        Loader::Forge => &["forge"],
        _ => return true,
    };
    jar.iter().any(|l| accepts.contains(l))
}

pub async fn check(l: &Launcher, instance_id: &str) -> Result<Report> {
    let inst = instances::get(l, instance_id).await?;
    let dir = l.paths.mods_dir(&inst.id);
    let mut files = vec![];
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let f = e.file_name().to_string_lossy().to_string();
            if f.ends_with(".jar") {
                files.push(f);
            }
        }
    }
    files.sort();
    let loader = inst.loader;
    tokio::task::spawn_blocking(move || {
        let jars: Vec<(String, Jar)> = files.into_iter().filter_map(|f| read_jar(&dir.join(&f)).map(|j| (f, j))).collect();
        Ok(report(loader, &jars))
    })
    .await?
}

fn report(loader: Loader, jars: &[(String, Jar)]) -> Report {
    let mut problems = vec![];
    let installed: HashSet<&str> = jars.iter().flat_map(|(_, j)| j.provides.iter().map(String::as_str)).collect();
    let label = |f: &str, j: &Jar| j.name.clone().unwrap_or_else(|| f.to_string());
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (f, j) in jars {
        let name = label(f, j);
        if !loader_ok(loader, &j.loaders) {
            let made_for = j.loaders.iter().copied().collect::<Vec<_>>().join(" and ");
            problems.push(Problem {
                kind: Kind::WrongLoader,
                file: f.clone(),
                name: name.clone(),
                detail: made_for.clone(),
                message: format!("{name} is a {made_for} mod, so this {} instance won't load it", crate::claude_code::loader_label(loader)),
            });
            continue;
        }
        for id in &j.ids {
            if let Some(other) = seen.insert(id, f) {
                problems.push(Problem {
                    kind: Kind::Duplicate,
                    file: f.clone(),
                    name: name.clone(),
                    detail: other.to_string(),
                    message: format!("{name} is installed twice ({other} and {f}); keep one"),
                });
            }
        }
        for dep in &j.requires {
            if BUILT_IN.contains(&dep.as_str()) || installed.contains(dep.as_str()) {
                continue;
            }
            problems.push(Problem {
                kind: Kind::MissingDependency,
                file: f.clone(),
                name: name.clone(),
                detail: dep.clone(),
                message: format!("{name} needs the mod \"{dep}\", which isn't installed (or is turned off)"),
            });
        }
    }
    Report { checked: jars.len(), problems }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(vec![]));
        for (n, b) in files {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn jar(files: &[(&str, &[u8])]) -> Jar {
        let mut z = zip::ZipArchive::new(Cursor::new(zip_bytes(files))).unwrap();
        let mut j = Jar::default();
        scan(&mut z, &mut j, true, 0);
        j
    }

    #[test]
    fn finds_missing_wrong_and_duplicate_mods() {
        // Fabric API: provides "fabric" and bundles its modules as nested jars.
        let module = zip_bytes(&[("fabric.mod.json", br#"{"id":"fabric-api-base"}"#)]);
        let api = jar(&[
            ("fabric.mod.json", br#"{"id":"fabric-api","name":"Fabric API","provides":["fabric"],"jars":[{"file":"META-INF/jars/base.jar"}],"depends":{"fabricloader":">=0.16"}}"#),
            ("META-INF/jars/base.jar", &module),
        ]);
        let a = jar(&[("fabric.mod.json", br#"{"id":"a","name":"Mod A","depends":{"minecraft":"~1.21","fabric":"*","fabric-api-base":"*","cloth-config":"*"},"recommends":{"modmenu":"*"}}"#)]);
        let a2 = jar(&[("fabric.mod.json", br#"{"id":"a","name":"Mod A"}"#)]);
        let neo = jar(&[("META-INF/neoforge.mods.toml", b"[[mods]]\nmodId=\"jei\"\ndisplayName=\"JEI\"\n[[dependencies.jei]]\nmodId=\"neoforge\"\ntype=\"required\"\n")]);
        let jars = vec![("api.jar".to_string(), api), ("a-1.jar".to_string(), a), ("a-2.jar".to_string(), a2), ("jei.jar".to_string(), neo)];
        let r = report(Loader::Fabric, &jars);
        let msgs: Vec<_> = r.problems.iter().map(|p| (p.kind.clone(), p.detail.clone())).collect();
        assert_eq!(
            msgs,
            vec![
                (Kind::MissingDependency, "cloth-config".into()),
                (Kind::Duplicate, "a-1.jar".into()),
                (Kind::WrongLoader, "neoforge".into()),
            ],
            "{:#?}",
            r.problems
        );
        assert!(r.problems[0].message.contains("Mod A needs the mod \"cloth-config\""));
    }

/// Read-only: check every instance in a real data folder for false alarms.
    /// MIO_CHECK_ROOT=<data folder> cargo test -p launcher-core scans_real_instances -- --ignored --nocapture
    #[test]
    #[ignore]
    fn scans_real_instances() {
        let root = std::path::PathBuf::from(std::env::var("MIO_CHECK_ROOT").expect("set MIO_CHECK_ROOT"));
        for e in std::fs::read_dir(root.join("instances")).unwrap().flatten() {
            let Ok(inst) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(e.path().join("instance.json")).unwrap_or_default()) else { continue };
            let Ok(loader) = serde_json::from_value::<Loader>(inst["loader"].clone()) else { continue };
            let dir = e.path().join("minecraft").join("mods");
            let mut files: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|f| f.file_name().to_string_lossy().to_string()).filter(|f| f.ends_with(".jar")).collect();
            files.sort();
            let jars: Vec<(String, Jar)> = files.into_iter().filter_map(|f| read_jar(&dir.join(&f)).map(|j| (f, j))).collect();
            let r = report(loader, &jars);
            println!("{:?}: {} mods, {} problems", loader, r.checked, r.problems.len());
            for p in r.problems {
                println!("   {}", p.message);
            }
        }
    }

    #[test]
    fn reads_neoforge_and_forge_dependencies() {
        let lib = zip_bytes(&[("META-INF/neoforge.mods.toml", b"[[mods]]\nmodId=\"bundled_lib\"\n")]);
        let j = jar(&[
            ("META-INF/neoforge.mods.toml", b"[[mods]]\nmodId=\"create\"\n[[dependencies.create]]\nmodId=\"flywheel\"\ntype=\"required\"\n[[dependencies.create]]\nmodId=\"jei\"\ntype=\"optional\"\n[[dependencies.create]]\nmodId=\"bundled_lib\"\ntype=\"required\"\n[[dependencies.create]]\nmodId=\"server_only\"\ntype=\"required\"\nside=\"SERVER\"\n"),
            ("META-INF/jarjar/lib.jar", &lib),
        ]);
        let r = report(Loader::Neoforge, &[("create.jar".into(), j)]);
        assert_eq!(r.problems.iter().map(|p| p.detail.as_str()).collect::<Vec<_>>(), vec!["flywheel"]);
        let old = jar(&[("META-INF/mods.toml", b"[[mods]]\nmodId=\"x\"\n[[dependencies.x]]\nmodId=\"y\"\nmandatory=true\n")]);
        assert_eq!(old.requires.iter().collect::<Vec<_>>(), vec!["y"]);
        // Quilt runs Fabric mods; Fabric doesn't run Quilt-only ones.
        let q = jar(&[("quilt.mod.json", br#"{"quilt_loader":{"id":"q","depends":["minecraft",{"id":"org.quiltmc:qsl"},{"id":"opt","optional":true}]}}"#)]);
        assert_eq!(q.requires.iter().collect::<Vec<_>>(), vec!["minecraft", "qsl"]);
        assert!(loader_ok(Loader::Quilt, &BTreeSet::from(["fabric"])));
        assert!(!loader_ok(Loader::Fabric, &BTreeSet::from(["quilt"])));
        assert!(loader_ok(Loader::Fabric, &BTreeSet::new()));
    }
}
