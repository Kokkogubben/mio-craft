//! Mod project templates for Fabric and NeoForge, adapted to the instance's
//! Minecraft version.
//!
//! Build tool versions are picked per Minecraft "era". They are a best
//! starting point: if a build fails because a version moved on, Claude Code
//! can read the build output (via the MCP `build_mod` tool) and fix the
//! Gradle files directly. See NOTES-FOR-CLAUDE-CODE.md.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::loaders::Loader;
use crate::maven::cmp_versions;

pub const GRADLE_8: &str = "8.14.3";
pub const GRADLE_9: &str = "9.2.1";
pub const FABRIC_LOOM_OBF: &str = "1.11-SNAPSHOT";
pub const FABRIC_LOOM_UNOBF_FALLBACK: &str = "1.14-SNAPSHOT";
pub const MODDEV_FALLBACK: &str = "2.0.107";

pub fn mc_at_least(mc: &str, min: &str) -> bool {
    cmp_versions(mc, min) != std::cmp::Ordering::Less
}

/// Minecraft 26.1 and later ship without obfuscation (no mappings needed).
pub fn is_unobfuscated(mc: &str) -> bool {
    mc.split('.').next().and_then(|p| p.parse::<u32>().ok()).map(|y| y >= 25).unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Toolchain {
    pub java_target: u32,
    pub gradle_version: String,
    pub gradle_jdk: u32,
    /// Fabric: Loom version. NeoForge: ModDevGradle version.
    pub plugin_version: String,
}

pub fn toolchain(loader: Loader, mc: &str, java_target: u32, latest_plugin: Option<String>) -> Result<Toolchain> {
    let unobf = is_unobfuscated(mc);
    let gradle_jdk = java_target.max(21);
    let gradle_version = if unobf || java_target > 24 { GRADLE_9 } else { GRADLE_8 }.to_string();
    let plugin_version = match loader {
        Loader::Fabric => {
            if !mc_at_least(mc, "1.14.4") {
                bail!("Mod templates need Minecraft 1.14.4 or newer (the first version with official mappings)");
            }
            if unobf {
                latest_plugin
                    .map(|v| loom_snapshot(&v))
                    .unwrap_or_else(|| FABRIC_LOOM_UNOBF_FALLBACK.into())
            } else {
                FABRIC_LOOM_OBF.into()
            }
        }
        Loader::Neoforge => {
            if !mc_at_least(mc, "1.20.2") {
                bail!("NeoForge supports Minecraft 1.20.2 and newer");
            }
            latest_plugin.unwrap_or_else(|| MODDEV_FALLBACK.into())
        }
        Loader::Vanilla => bail!("Vanilla instances can't load mods; create a Fabric or NeoForge instance"),
    };
    Ok(Toolchain { java_target, gradle_version, gradle_jdk, plugin_version })
}

/// `1.14.3` → `1.14-SNAPSHOT` (Loom's recommended moving version).
fn loom_snapshot(v: &str) -> String {
    let mut parts = v.split('.');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => format!("{a}.{}-SNAPSHOT", b.split('-').next().unwrap_or(b)),
        _ => v.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModSpec {
    pub mod_id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub mc_version: String,
    pub loader: Loader,
    pub loader_version: String,
    /// Fabric only.
    pub fabric_api_version: Option<String>,
    pub toolchain: Toolchain,
}

impl ModSpec {
    pub fn package(&self) -> String {
        format!("com.claudemods.{}", self.mod_id)
    }
    pub fn class_name(&self) -> String {
        self.mod_id
            .split('_')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<String>()
            + "Mod"
    }
}

/// Mod ids must be lowercase `[a-z0-9_]`, start with a letter, 2–64 chars.
pub fn normalize_mod_id(input: &str) -> Result<String> {
    let mut id: String = input
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while id.contains("__") {
        id = id.replace("__", "_");
    }
    let id = id.trim_matches('_').to_string();
    if id.len() < 2 || id.len() > 64 || !id.chars().next().unwrap().is_ascii_lowercase() {
        bail!("mod id {input:?} must be 2–64 characters of a-z, 0-9 and _, starting with a letter");
    }
    Ok(id)
}

fn esc_json(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn esc_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// All files of a new project, as relative path → contents.
pub fn render(spec: &ModSpec) -> BTreeMap<String, String> {
    let mut f = BTreeMap::new();
    let pkg = spec.package();
    let pkg_path = pkg.replace('.', "/");
    let class = spec.class_name();
    f.insert(".gitignore".into(), GITIGNORE.into());
    f.insert("CLAUDE.md".into(), claude_md(spec));
    match spec.loader {
        Loader::Fabric => {
            let unobf = is_unobfuscated(&spec.mc_version);
            f.insert("settings.gradle".into(), fabric_settings(spec));
            f.insert("build.gradle".into(), fabric_build(spec, unobf));
            f.insert("gradle.properties".into(), fabric_props(spec));
            f.insert(format!("src/main/java/{pkg_path}/{class}.java"), fabric_main(spec, &pkg, &class));
            f.insert("src/main/resources/fabric.mod.json".into(), fabric_mod_json(spec, &pkg, &class));
            f.insert(
                format!("src/main/resources/assets/{}/lang/en_us.json", spec.mod_id),
                "{\n}\n".into(),
            );
        }
        Loader::Neoforge => {
            f.insert("settings.gradle".into(), NEO_SETTINGS.into());
            f.insert("build.gradle".into(), neo_build(spec));
            f.insert("gradle.properties".into(), neo_props(spec));
            f.insert(format!("src/main/java/{pkg_path}/{class}.java"), neo_main(spec, &pkg, &class));
            let toml_name = if mc_at_least(&spec.mc_version, "1.20.5") { "neoforge.mods.toml" } else { "mods.toml" };
            f.insert(format!("src/main/resources/META-INF/{toml_name}"), neo_mods_toml(spec));
            f.insert(
                format!("src/main/resources/assets/{}/lang/en_us.json", spec.mod_id),
                "{\n}\n".into(),
            );
        }
        Loader::Vanilla => {}
    }
    f
}

const GITIGNORE: &str = "build/\n.gradle/\nrun/\nout/\n*.iml\n.idea/\n.vscode/\nbin/\n";

fn fabric_settings(_s: &ModSpec) -> String {
    r#"pluginManagement {
    repositories {
        maven {
            name = 'Fabric'
            url = 'https://maven.fabricmc.net/'
        }
        mavenCentral()
        gradlePluginPortal()
    }
}
"#
    .into()
}

fn fabric_build(s: &ModSpec, unobf: bool) -> String {
    let (plugin, mappings, conf) = if unobf {
        ("net.fabricmc.fabric-loom", "", "implementation")
    } else {
        ("fabric-loom", "    mappings loom.officialMojangMappings()\n", "modImplementation")
    };
    let j = s.toolchain.java_target;
    let java_enum = if j <= 8 { "VERSION_1_8".to_string() } else { format!("VERSION_{j}") };
    format!(
        r#"plugins {{
    id '{plugin}' version '{loom}'
}}

version = project.mod_version
group = project.maven_group

base {{
    archivesName = project.archives_base_name
}}

repositories {{
    // Add repositories for extra dependencies here.
}}

dependencies {{
    minecraft "com.mojang:minecraft:${{project.minecraft_version}}"
{mappings}    {conf} "net.fabricmc:fabric-loader:${{project.loader_version}}"
    {conf} "net.fabricmc.fabric-api:fabric-api:${{project.fabric_version}}"
}}

{live_api}processResources {{
    inputs.property "version", project.version
    filesMatching("fabric.mod.json") {{
        expand "version": inputs.properties.version
    }}
}}

tasks.withType(JavaCompile).configureEach {{
    it.options.release = {j}
    it.options.encoding = 'UTF-8'
}}

java {{
    sourceCompatibility = JavaVersion.{java_enum}
    targetCompatibility = JavaVersion.{java_enum}
}}

jar {{
    from("LICENSE") {{
        rename {{ "${{it}}_${{project.base.archivesName.get()}}" }}
    }}
}}
"#,
        loom = s.toolchain.plugin_version,
        live_api = if unobf { LIVE_API_BLOCK } else { "" },
    )
}

/// MioCraft passes `-Pclaude_live_api=<jar>` so mods can implement live
/// behaviours (dev.miocraft.live.api) without bundling the companion.
const LIVE_API_BLOCK: &str = r#"// MioCraft live mode: behaviour interfaces for live content.
if (project.hasProperty('claude_live_api') && file(project.claude_live_api).exists()) {
    dependencies {
        compileOnly files(project.claude_live_api)
    }
}

"#;

fn fabric_props(s: &ModSpec) -> String {
    format!(
        "org.gradle.jvmargs=-Xmx2G\norg.gradle.parallel=true\n\n\
         minecraft_version={mc}\nloader_version={loader}\nfabric_version={api}\n\n\
         mod_version=1.0.0\nmaven_group={group}\narchives_base_name={id}\n",
        mc = s.mc_version,
        loader = s.loader_version,
        api = s.fabric_api_version.clone().unwrap_or_else(|| "CHANGE_ME".into()),
        group = s.package(),
        id = s.mod_id,
    )
}

fn fabric_main(s: &ModSpec, pkg: &str, class: &str) -> String {
    format!(
        r#"package {pkg};

import net.fabricmc.api.ModInitializer;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public class {class} implements ModInitializer {{
    public static final String MOD_ID = "{id}";
    public static final Logger LOGGER = LoggerFactory.getLogger(MOD_ID);

    @Override
    public void onInitialize() {{
        LOGGER.info("{name} loaded");
    }}
}}
"#,
        id = s.mod_id,
        name = s.name.replace('"', "'"),
    )
}

fn fabric_mod_json(s: &ModSpec, pkg: &str, class: &str) -> String {
    let java_dep = if s.toolchain.java_target <= 8 { "8".to_string() } else { s.toolchain.java_target.to_string() };
    format!(
        r#"{{
  "schemaVersion": 1,
  "id": {id},
  "version": "${{version}}",
  "name": {name},
  "description": {desc},
  "authors": [{author}],
  "license": "All-Rights-Reserved",
  "environment": "*",
  "entrypoints": {{
    "main": ["{pkg}.{class}"]
  }},
  "depends": {{
    "fabricloader": ">={loader}",
    "minecraft": "~{mc}",
    "java": ">={java_dep}",
    "fabric-api": "*"
  }}
}}
"#,
        id = esc_json(&s.mod_id),
        name = esc_json(&s.name),
        desc = esc_json(&s.description),
        author = esc_json(&s.author),
        loader = s.loader_version,
        mc = s.mc_version,
    )
}

const NEO_SETTINGS: &str = r#"pluginManagement {
    repositories {
        gradlePluginPortal()
        maven { url = 'https://maven.neoforged.net/releases' }
    }
}
"#;

fn neo_build(s: &ModSpec) -> String {
    format!(
        r#"plugins {{
    id 'java-library'
    id 'net.neoforged.moddev' version '{mdg}'
}}

version = project.mod_version
group = project.mod_group_id

base {{
    archivesName = project.mod_id
}}

java.toolchain.languageVersion = JavaLanguageVersion.of({j})

neoForge {{
    version = project.neo_version

    runs {{
        client {{
            client()
            gameDirectory = project.file('run')
            systemProperty 'neoforge.enabledGameTestNamespaces', project.mod_id
        }}
    }}

    mods {{
        "${{mod_id}}" {{
            sourceSet(sourceSets.main)
        }}
    }}
}}

repositories {{
    // Add repositories for extra dependencies here.
}}

tasks.withType(JavaCompile).configureEach {{
    options.encoding = 'UTF-8'
}}
"#,
        mdg = s.toolchain.plugin_version,
        j = s.toolchain.java_target,
    )
}

fn neo_props(s: &ModSpec) -> String {
    format!(
        "org.gradle.jvmargs=-Xmx2G\norg.gradle.parallel=true\n\n\
         minecraft_version={mc}\nneo_version={neo}\n\n\
         mod_id={id}\nmod_version=1.0.0\nmod_group_id={group}\n",
        mc = s.mc_version,
        neo = s.loader_version,
        id = s.mod_id,
        group = s.package(),
    )
}

fn neo_main(s: &ModSpec, pkg: &str, class: &str) -> String {
    format!(
        r#"package {pkg};

import com.mojang.logging.LogUtils;
import net.neoforged.bus.api.IEventBus;
import net.neoforged.fml.common.Mod;
import org.slf4j.Logger;

@Mod({class}.MOD_ID)
public class {class} {{
    public static final String MOD_ID = "{id}";
    public static final Logger LOGGER = LogUtils.getLogger();

    public {class}(IEventBus modEventBus) {{
        LOGGER.info("{name} loaded");
    }}
}}
"#,
        id = s.mod_id,
        name = s.name.replace('"', "'"),
    )
}

fn neo_mods_toml(s: &ModSpec) -> String {
    // 1.20.5+ uses `type = "required"`; earlier NeoForge uses `mandatory = true`.
    let required = if mc_at_least(&s.mc_version, "1.20.5") { "type=\"required\"" } else { "mandatory=true" };
    let id = &s.mod_id;
    format!(
        r#"modLoader="javafml"
loaderVersion="[1,)"
license="All Rights Reserved"

[[mods]]
modId="{id}"
version="1.0.0"
displayName="{name}"
authors="{author}"
description='''
{desc}
'''

[[dependencies.{id}]]
modId="neoforge"
{required}
versionRange="[{neo},)"
ordering="NONE"
side="BOTH"

[[dependencies.{id}]]
modId="minecraft"
{required}
versionRange="[{mc},)"
ordering="NONE"
side="BOTH"
"#,
        name = esc_toml(&s.name),
        author = esc_toml(&s.author),
        desc = s.description.replace("'''", "'"),
        neo = s.loader_version,
        mc = s.mc_version,
    )
}

fn claude_md(s: &ModSpec) -> String {
    let loader = match s.loader {
        Loader::Fabric => "Fabric",
        Loader::Neoforge => "NeoForge",
        Loader::Vanilla => "Vanilla",
    };
    let mappings = if is_unobfuscated(&s.mc_version) {
        "none (Minecraft is unobfuscated from 26.1)".to_string()
    } else {
        "official Mojang mappings".to_string()
    };
    let docs = match s.loader {
        Loader::Fabric => "https://docs.fabricmc.net/develop/",
        _ => "https://docs.neoforged.net/",
    };
    let version_file = if s.loader == Loader::Fabric { "fabric.mod.json" } else { "the mods.toml" };
    format!(
        r#"# {name}

A {loader} mod for Minecraft {mc}, created in MioCraft.

- Mod id: `{id}`
- Main class: `{pkg}.{class}`
- Java target: {java}
- Mappings: {mappings}
- Loader version: {loaderv}
- Build tools: Gradle {gradle}, {plugin_name} {plugin}
- Docs: {docs}

## Working on this mod

Use the `mio-craft` MCP tools instead of running Gradle yourself; they use
the launcher's own Java and Gradle so nothing needs installing:

1. Edit the source under `src/main`.
2. `build_mod` compiles it. Read the errors it returns and fix them.
3. `install_mod` builds and copies the jar into the instance, where it shows up
   in the launcher's mod list tagged as made by Claude.
4. `run_dev_client` starts a development client (offline, no account needed) to
   test the mod; `read_dev_log` shows its output. `launch_game` plays the real
   instance with the user's account.

Bump `mod_version` in `gradle.properties` for releases (and in {version_file}
for NeoForge). Keep the mod id unchanged.
"#,
        name = s.name,
        mc = s.mc_version,
        id = s.mod_id,
        pkg = s.package(),
        class = s.class_name(),
        java = s.toolchain.java_target,
        loaderv = s.loader_version,
        gradle = s.toolchain.gradle_version,
        plugin_name = if s.loader == Loader::Fabric { "Fabric Loom" } else { "ModDevGradle" },
        plugin = s.toolchain.plugin_version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(loader: Loader, mc: &str, java: u32) -> ModSpec {
        ModSpec {
            mod_id: "glow_blocks".into(),
            name: "Glow \"Blocks\"".into(),
            description: "Things glow.".into(),
            author: "Mio".into(),
            mc_version: mc.into(),
            loader,
            loader_version: "0.17.2".into(),
            fabric_api_version: Some("0.110.0+1.21.1".into()),
            toolchain: toolchain(loader, mc, java, None).unwrap(),
        }
    }

    #[test]
    fn ids() {
        assert_eq!(normalize_mod_id("Glow Blocks!").unwrap(), "glow_blocks");
        assert!(normalize_mod_id("1abc").is_err());
        assert!(normalize_mod_id("a").is_err());
    }

    #[test]
    fn fabric_files() {
        let s = spec(Loader::Fabric, "1.21.1", 21);
        assert_eq!(s.class_name(), "GlowBlocksMod");
        let f = render(&s);
        assert!(f.contains_key("src/main/java/com/claudemods/glow_blocks/GlowBlocksMod.java"));
        let mj: serde_json::Value = serde_json::from_str(&f["src/main/resources/fabric.mod.json"]).unwrap();
        assert_eq!(mj["name"], "Glow \"Blocks\"");
        assert!(f["build.gradle"].contains("officialMojangMappings"));
        assert!(f["build.gradle"].contains("id 'fabric-loom'"));
        assert_eq!(s.toolchain.gradle_version, GRADLE_8);
    }

    #[test]
    fn fabric_unobfuscated() {
        let s = spec(Loader::Fabric, "26.1", 25);
        let f = render(&s);
        assert!(!f["build.gradle"].contains("mappings"));
        assert!(f["build.gradle"].contains("net.fabricmc.fabric-loom"));
        assert!(f["build.gradle"].contains("claude_live_api"));
        assert_eq!(s.toolchain.gradle_version, GRADLE_9);
        assert_eq!(s.toolchain.gradle_jdk, 25);
    }

    #[test]
    fn neoforge_files() {
        let s = spec(Loader::Neoforge, "1.21.1", 21);
        let f = render(&s);
        let t = &f["src/main/resources/META-INF/neoforge.mods.toml"];
        let parsed: toml::Value = toml::from_str(t).unwrap();
        assert_eq!(parsed["mods"][0]["displayName"].as_str(), Some("Glow \"Blocks\""));
        assert!(t.contains("type=\"required\""));
        let old = render(&spec(Loader::Neoforge, "1.20.4", 17));
        assert!(old.contains_key("src/main/resources/META-INF/mods.toml"));
        assert!(old["src/main/resources/META-INF/mods.toml"].contains("mandatory=true"));
    }

    #[test]
    fn version_floors() {
        assert!(toolchain(Loader::Neoforge, "1.20.1", 17, None).is_err());
        assert!(toolchain(Loader::Fabric, "1.12.2", 8, None).is_err());
        assert!(toolchain(Loader::Fabric, "1.16.5", 8, None).is_ok());
    }

    #[test]
    fn loom_snap() {
        assert_eq!(loom_snapshot("1.14.3"), "1.14-SNAPSHOT");
    }
}
