//! Building the Java command line and starting the game.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::auth::Account;
use crate::mojang::{rules_allow_with, Argument, VersionJson};
use crate::{auth, http, instances, java, mojang, platform, process, Launcher};

pub struct LaunchPlan {
    pub java: PathBuf,
    pub args: Vec<String>,
    pub game_dir: PathBuf,
}

/// Substitute `${name}` placeholders. Unknown names are left as-is.
pub fn substitute(s: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push_str("${");
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn expand(args: &[Argument], vars: &HashMap<&str, String>, features: &[&str]) -> Vec<String> {
    let mut out = vec![];
    for a in args {
        match a {
            Argument::Plain(s) => out.push(substitute(s, vars)),
            Argument::Ruled { rules, value } => {
                if rules_allow_with(&Some(rules.clone()), features) {
                    out.extend(value.to_vec().iter().map(|s| substitute(s, vars)));
                }
            }
        }
    }
    out
}

fn extract_natives(v: &VersionJson, libs: &Path, natives_dir: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(natives_dir);
    std::fs::create_dir_all(natives_dir)?;
    for lib in &v.libraries {
        if let Some(d) = lib.natives_download(libs) {
            http::extract_zip(&d.path, natives_dir, |n| !n.starts_with("META-INF/"), false)
                .with_context(|| format!("extracting natives from {}", d.path.display()))?;
        }
    }
    Ok(())
}

/// Does this version's launch arguments offer joining a server with Quick Play (1.20+)?
fn has_quick_play(v: &VersionJson) -> bool {
    v.arguments.as_ref().is_some_and(|a| {
        a.game.iter().any(|x| match x {
            Argument::Ruled { rules, .. } => rules.iter().any(|r| r.features.as_ref().is_some_and(|f| f.contains_key("is_quick_play_multiplayer"))),
            _ => false,
        })
    })
}

/// The game arguments that join `server` straight away: Quick Play where the
/// version has it, otherwise the older `--server` and `--port`.
fn join_server(v: &VersionJson, server: &str, vars: &mut HashMap<&str, String>) -> (Vec<&'static str>, Vec<String>) {
    if has_quick_play(v) {
        vars.insert("quickPlayMultiplayer", server.to_string());
        (vec!["is_quick_play_multiplayer"], vec![])
    } else {
        let (host, port) = crate::servers::split_address(server);
        (vec![], vec!["--server".into(), host, "--port".into(), port.to_string()])
    }
}

pub async fn plan(
    l: &Launcher,
    inst: &instances::Instance,
    account: &Account,
    server: Option<&str>,
) -> Result<LaunchPlan> {
    let version_id = inst
        .version_id
        .clone()
        .ok_or_else(|| anyhow!("instance is not installed yet"))?;
    let (v, root_id) = mojang::resolve(l, &version_id).await?;
    let libs = l.paths.libraries();
    let game_dir = l.paths.game_dir(&inst.id);
    tokio::fs::create_dir_all(&game_dir).await?;

    // Client jar. Loaders that inherit from vanilla expect the jar to carry
    // their own version name (NeoForge ignores `${version_name}.jar`).
    let mut client_jar = l.paths.version_jar(&root_id);
    if version_id != root_id {
        let own = l.paths.version_jar(&version_id);
        if !own.exists() {
            if std::fs::hard_link(&client_jar, &own).is_err() {
                tokio::fs::copy(&client_jar, &own).await?;
            }
        }
        client_jar = own;
    }

    let mut cp: Vec<String> = vec![];
    let mut seen = std::collections::HashSet::new();
    for lib in &v.libraries {
        if let Some(d) = lib.artifact_download(&libs)? {
            let p = d.path.to_string_lossy().to_string();
            if seen.insert(p.clone()) {
                cp.push(p);
            }
        }
    }
    cp.push(client_jar.to_string_lossy().to_string());

    let natives_dir = l.paths.instance_dir(&inst.id).join("natives");
    {
        let (v2, libs2, nd) = (v.clone(), libs.clone(), natives_dir.clone());
        tokio::task::spawn_blocking(move || extract_natives(&v2, &libs2, &nd)).await??;
    }

    let assets_index = v.asset_index.as_ref().map(|a| a.id.clone()).or(v.assets.clone()).unwrap_or_else(|| "legacy".into());
    let assets_root = l.paths.assets();
    let virtual_dir = assets_root.join("virtual").join(&assets_index);

    let sep = platform::classpath_separator();
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", account.name.clone());
    vars.insert("version_name", version_id.clone());
    vars.insert("game_directory", game_dir.to_string_lossy().into());
    vars.insert("assets_root", assets_root.to_string_lossy().into());
    vars.insert("game_assets", virtual_dir.to_string_lossy().into());
    vars.insert("assets_index_name", assets_index);
    vars.insert("auth_uuid", account.id.clone());
    vars.insert("auth_access_token", account.mc_token.clone());
    vars.insert("auth_session", format!("token:{}:{}", account.mc_token, account.id));
    vars.insert("auth_xuid", "0".into());
    vars.insert("clientid", "0".into());
    vars.insert("user_type", "msa".into());
    vars.insert("user_properties", "{}".into());
    vars.insert("version_type", v.kind.clone().unwrap_or_else(|| "release".into()));
    vars.insert("natives_directory", natives_dir.to_string_lossy().into());
    vars.insert("launcher_name", "mio-craft".into());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").into());
    vars.insert("classpath", cp.join(sep));
    vars.insert("classpath_separator", sep.into());
    vars.insert("library_directory", libs.to_string_lossy().into());
    let (features, join_args) = match server {
        Some(s) => join_server(&v, s, &mut vars),
        None => (vec![], vec![]),
    };

    let cfg = l.config.read().await.clone();
    let ram = inst.ram_mb.unwrap_or(cfg.default_ram_mb);
    let mut args = vec![
        format!("-Xmx{ram}M"),
        "-Xms512M".to_string(),
        // Belt-and-braces Log4Shell mitigation for old versions.
        "-Dlog4j2.formatMsgNoLookups=true".to_string(),
    ];
    args.extend(inst.jvm_args.as_ref().unwrap_or(&cfg.extra_jvm_args).iter().cloned());
    let live = crate::live::plan(l, inst, &v).await?;
    args.extend(live.jvm_args);

    match &v.arguments {
        Some(a) if !a.jvm.is_empty() => args.extend(expand(&a.jvm, &vars, &features)),
        _ => {
            args.push(format!("-Djava.library.path={}", natives_dir.display()));
            args.push("-cp".into());
            args.push(cp.join(sep));
        }
    }
    args.push(v.main_class.clone().ok_or_else(|| anyhow!("version has no main class"))?);
    if let Some(a) = &v.arguments {
        args.extend(expand(&a.game, &vars, &features));
    }
    if let Some(legacy) = &v.minecraft_arguments {
        args.extend(legacy.split_whitespace().map(|s| substitute(s, &vars)));
    }
    args.extend(join_args);

    let jdk = match live.java_home {
        Some(jbr) => jbr,
        None => java::ensure_jdk(l, java::required_major(&v)).await?,
    };
    Ok(LaunchPlan { java: java::java_bin(&jdk), args, game_dir })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

pub fn game_source(instance_id: &str) -> String {
    format!("game:{instance_id}")
}

/// Start the game. With `server`, it joins that server (`host` or `host:port`) straight away.
pub async fn launch(l: &Launcher, instance_id: &str, account_id: Option<&str>, server: Option<&str>) -> Result<()> {
    if let Some(s) = server {
        crate::servers::check_address(s)?;
    }
    let mut inst = instances::get(l, instance_id).await?;
    if inst.version_id.is_none() {
        inst = instances::install(l, instance_id).await?;
    } else {
        // Cheap when everything is already there; repairs missing files.
        let _guard = l.install_lock.lock().await;
        mojang::install_files(l, inst.version_id.as_ref().unwrap()).await?;
    }
    crate::companion::sync(l, &inst).await?;
    let account = auth::usable_account(l, account_id).await?;
    let plan = plan(l, &inst, &account, server).await?;

    let mut cmd = tokio::process::Command::new(&plan.java);
    cmd.args(&plan.args).current_dir(&plan.game_dir);
    // Mark the launch before starting, so a crash that happens instantly is
    // recorded after this, not overwritten by it.
    inst.last_played = Some(now_secs());
    inst.last_exit = None;
    instances::save(l, &inst).await?;

    let log = l.paths.instance_dir(&inst.id).join("launcher-output.log");
    // Record how the game exits so crashes can be shown after a restart.
    let mut rx = l.events.subscribe();
    let source = game_source(&inst.id);
    let watcher = {
        let (l2, id, source) = (l.clone(), inst.id.clone(), source.clone());
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let crate::Event::ProcessExited { source: s, code } = ev {
                    if s == source {
                        if let Ok(mut i) = instances::get(&l2, &id).await {
                            i.last_exit = Some(instances::LastExit { code, at: now_secs() });
                            let _ = instances::save(&l2, &i).await;
                            if code.map(|c| c != 0).unwrap_or(false) {
                                l2.emit(crate::Event::InstancesChanged);
                            }
                            // Save hotswapped code into the mod jars now the files are unlocked.
                            crate::projects::install_pending(&l2, &id).await;
                        }
                        break;
                    }
                }
            }
        })
    };
    if let Err(e) = process::spawn_tracked(l, &source, cmd, log, vec![account.mc_token.clone()]).await {
        watcher.abort();
        return Err(e);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution() {
        let mut v = HashMap::new();
        v.insert("a", "1".to_string());
        assert_eq!(substitute("x${a}y${b}z", &v), "x1y${b}z");
        assert_eq!(substitute("${a", &v), "${a");
        assert_eq!(substitute("-Dp=${a}:${a}", &v), "-Dp=1:1");
    }

    /// 1.20+ joins with Quick Play from the version's own arguments; older
    /// versions get --server and --port.
    #[test]
    fn joins_a_server() {
        let modern: VersionJson = serde_json::from_value(serde_json::json!({
            "id": "1.21.1", "mainClass": "net.minecraft.client.main.Main",
            "arguments": { "jvm": [], "game": [
                "--username", "${auth_player_name}",
                { "rules": [{ "action": "allow", "features": { "is_demo_user": true } }], "value": "--demo" },
                { "rules": [{ "action": "allow", "features": { "has_quick_plays_support": true } }], "value": ["--quickPlayPath", "${quickPlayPath}"] },
                { "rules": [{ "action": "allow", "features": { "is_quick_play_multiplayer": true } }], "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"] },
            ]}
        }))
        .unwrap();
        let mut vars = HashMap::new();
        vars.insert("auth_player_name", "Mio".to_string());
        let (features, extra) = join_server(&modern, "play.example.net:25570", &mut vars);
        assert!(extra.is_empty());
        let args = expand(&modern.arguments.as_ref().unwrap().game, &vars, &features);
        assert_eq!(args, vec!["--username", "Mio", "--quickPlayMultiplayer", "play.example.net:25570"]);
        // Without a server none of the optional arguments appear.
        assert_eq!(expand(&modern.arguments.as_ref().unwrap().game, &vars, &[]), vec!["--username", "Mio"]);

        let old: VersionJson = serde_json::from_value(serde_json::json!({
            "id": "1.12.2", "mainClass": "net.minecraft.client.main.Main",
            "minecraftArguments": "--username ${auth_player_name}"
        }))
        .unwrap();
        let (features, extra) = join_server(&old, "[::1]:25566", &mut vars);
        assert!(features.is_empty());
        assert_eq!(extra, vec!["--server", "::1", "--port", "25566"]);
    }
}
