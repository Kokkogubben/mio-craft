//! Mod project templates, adapted to the instance's loader and Minecraft version:
//! Fabric (also used for Quilt, which runs Fabric mods), NeoForge, Forge from the
//! instance's own MDK (1.17 and newer), and Forge 1.7.10 and 1.12.2 with
//! RetroFuturaGradle.
//!
//! Build tool versions are picked per Minecraft "era". They are a best
//! starting point: if a build fails because a version moved on, Claude Code
//! can read the build output (via the MCP `build_mod` tool) and fix the
//! Gradle files directly.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::loaders::Loader;
use crate::maven::cmp_versions;

pub const GRADLE_8: &str = "8.14.3";
/// Loom for unobfuscated Minecraft is taken at its latest version, which raises
/// its minimum Gradle over time (1.19 needs 9.8). Bump this when a build says so.
pub const GRADLE_9: &str = "9.8.2";
pub const FABRIC_LOOM_OBF: &str = "1.11-SNAPSHOT";
pub const FABRIC_LOOM_UNOBF_FALLBACK: &str = "1.14-SNAPSHOT";
pub const MODDEV_FALLBACK: &str = "2.0.107";
/// RetroFuturaGradle (Forge 1.7.10 and 1.12.2) and the Gradle its example builds with.
/// Pinned: a newer plugin can need a newer Gradle.
pub const RFG_VERSION: &str = "2.0.6";
pub const RFG_GRADLE: &str = "9.5.0";

/// Forge versions built with RetroFuturaGradle instead of the MDK.
pub fn is_rfg_forge(mc: &str) -> bool {
    mc == "1.7.10" || mc == "1.12.2"
}

/// Why Claude's templates can't make a mod for this loader and Minecraft version,
/// or None if they can. Shown in the launcher and returned by create_mod.
pub fn unsupported_reason(loader: Loader, mc: &str) -> Option<String> {
    match loader {
        Loader::Vanilla => Some("Vanilla instances can't load mods. Create an instance with a mod loader to make mods with Claude.".into()),
        Loader::Fabric | Loader::Quilt if !mc_at_least(mc, "1.14.4") => Some(format!(
            "Claude's {} mod templates need Minecraft 1.14.4 or newer.",
            loader.display_name()
        )),
        Loader::Neoforge if !mc_at_least(mc, "1.20.2") => Some("NeoForge supports Minecraft 1.20.2 and newer.".into()),
        Loader::Forge if !is_rfg_forge(mc) && !mc_at_least(mc, "1.17") => Some(format!(
            "Claude's Forge mod templates cover Minecraft 1.7.10, 1.12.2 and 1.17 and newer, not {mc}. This instance can still use mods from the Browse tab."
        )),
        Loader::LegacyFabric => Some("Claude's mod templates don't cover Legacy Fabric yet. This instance can still use mods from the Browse tab.".into()),
        _ => None,
    }
}

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

/// Build tools for a new project. Forge from 1.17 takes them from its MDK instead
/// (`forge_mdk_toolchain`).
pub fn toolchain(loader: Loader, mc: &str, java_target: u32, latest_plugin: Option<String>) -> Result<Toolchain> {
    if let Some(why) = unsupported_reason(loader, mc) {
        bail!(why);
    }
    let unobf = is_unobfuscated(mc);
    let gradle_jdk = java_target.max(21);
    let gradle_version = if unobf || java_target > 24 { GRADLE_9 } else { GRADLE_8 }.to_string();
    let plugin_version = match loader {
        Loader::Fabric | Loader::Quilt => {
            if unobf {
                latest_plugin
                    .map(|v| loom_snapshot(&v))
                    .unwrap_or_else(|| FABRIC_LOOM_UNOBF_FALLBACK.into())
            } else {
                FABRIC_LOOM_OBF.into()
            }
        }
        Loader::Neoforge => latest_plugin.unwrap_or_else(|| MODDEV_FALLBACK.into()),
        Loader::Forge if is_rfg_forge(mc) => {
            // Minecraft 1.7.10 and 1.12.2 run on Java 8; Gradle itself runs on a newer JDK.
            // RetroFuturaGradle 2 is built for Java 25, so Gradle runs on it.
            return Ok(Toolchain { java_target: 8, gradle_version: RFG_GRADLE.into(), gradle_jdk: 25, plugin_version: RFG_VERSION.into() });
        }
        Loader::Forge => bail!("Forge projects from 1.17 take their build tools from Forge's MDK"),
        Loader::Vanilla | Loader::LegacyFabric => unreachable!("refused by unsupported_reason"),
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
    /// The version the project builds against: for Quilt, a Fabric Loader version
    /// (the project is a Fabric mod, which Quilt runs).
    pub loader_version: String,
    /// Fabric and Quilt only.
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
        Loader::Fabric | Loader::Quilt => {
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
        Loader::Forge if is_rfg_forge(&spec.mc_version) => {
            f.insert("settings.gradle".into(), rfg_settings(spec));
            f.insert("build.gradle".into(), rfg_build(spec));
            f.insert("gradle.properties".into(), rfg_props(spec));
            f.insert(format!("src/main/java/{pkg_path}/{class}.java"), rfg_main(spec, &pkg, &class));
            f.insert("src/main/resources/mcmod.info".into(), rfg_mcmod_info(spec));
            f.insert("src/main/resources/pack.mcmeta".into(), rfg_pack_mcmeta(spec));
        }
        // Forge from 1.17 is made from its MDK (render_forge_mdk); toolchain() refuses the rest.
        Loader::Forge | Loader::Vanilla | Loader::LegacyFabric => {}
    }
    f
}

// ---------- Forge 1.7.10 and 1.12.2 (RetroFuturaGradle) ----------

fn rfg_settings(s: &ModSpec) -> String {
    format!(
        r#"pluginManagement {{
    repositories {{
        maven {{
            name = 'GTNH'
            url = 'https://nexus.gtnewhorizons.com/repository/public/'
            mavenContent {{
                includeGroupByRegex 'com\\.gtnewhorizons\\..+'
                includeGroup 'com.gtnewhorizons'
            }}
        }}
        gradlePluginPortal()
        mavenCentral()
    }}
}}

rootProject.name = '{id}'
"#,
        id = s.mod_id
    )
}

fn rfg_build(s: &ModSpec) -> String {
    format!(
        r#"buildscript {{
    repositories {{
        maven {{
            name = 'Fabric'
            url = 'https://maven.fabricmc.net/'
            mavenContent {{
                includeGroup 'net.fabricmc'
            }}
        }}
    }}
}}

plugins {{
    id 'java-library'
    id 'com.gtnewhorizons.retrofuturagradle' version '{rfg}'
}}

version = project.mod_version
group = project.maven_group

base {{
    archivesName = project.archives_base_name
}}

java {{
    toolchain {{
        languageVersion = JavaLanguageVersion.of(8)
    }}
}}

minecraft {{
    mcVersion.set('{mc}')
    username.set('Developer')
    injectedTags.put('VERSION', project.version)
}}

tasks.injectTags.configure {{
    outputClassName.set('{pkg}.Tags')
}}

repositories {{
    // Add repositories for extra dependencies here.
}}

processResources {{
    inputs.property 'version', project.version
    filesMatching('mcmod.info') {{
        expand 'version': project.version
    }}
}}

tasks.withType(JavaCompile).configureEach {{
    options.encoding = 'UTF-8'
}}
"#,
        rfg = s.toolchain.plugin_version,
        mc = s.mc_version,
        pkg = s.package(),
    )
}

fn rfg_props(s: &ModSpec) -> String {
    format!(
        "org.gradle.jvmargs=-Xmx3G

mod_version=1.0.0
maven_group={group}
archives_base_name={id}
",
        group = s.package(),
        id = s.mod_id,
    )
}

fn rfg_main(s: &ModSpec, pkg: &str, class: &str) -> String {
    let fml = if s.mc_version == "1.7.10" { "cpw.mods.fml" } else { "net.minecraftforge.fml" };
    format!(
        r#"package {pkg};

import {fml}.common.Mod;
import {fml}.common.event.FMLInitializationEvent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

@Mod(modid = {class}.MOD_ID, name = {class}.NAME, version = Tags.VERSION, acceptedMinecraftVersions = "[{mc}]")
public class {class} {{
    public static final String MOD_ID = "{id}";
    public static final String NAME = {name};
    public static final Logger LOGGER = LogManager.getLogger(MOD_ID);

    @Mod.EventHandler
    public void init(FMLInitializationEvent event) {{
        LOGGER.info(NAME + " loaded");
    }}
}}
"#,
        mc = s.mc_version,
        id = s.mod_id,
        name = esc_json(&s.name),
    )
}

/// Groovy's `expand` reads `$` as a placeholder, so user text escapes it.
fn esc_expand(json: String) -> String {
    json.replace('$', "\\$")
}

fn rfg_mcmod_info(s: &ModSpec) -> String {
    format!(
        r#"[
  {{
    "modid": {id},
    "name": {name},
    "description": {desc},
    "version": "${{version}}",
    "mcversion": "{mc}",
    "authorList": [{author}],
    "dependencies": []
  }}
]
"#,
        id = esc_json(&s.mod_id),
        name = esc_expand(esc_json(&s.name)),
        desc = esc_expand(esc_json(&s.description)),
        author = esc_expand(esc_json(&s.author)),
        mc = s.mc_version,
    )
}

/// Without its own pack.mcmeta, old Forge writes one from the mod name without escaping it,
/// so a name with a double quote crashes 1.12.2 at startup.
fn rfg_pack_mcmeta(s: &ModSpec) -> String {
    let format = if s.mc_version == "1.7.10" { 1 } else { 3 };
    format!(
        r#"{{
  "pack": {{
    "pack_format": {format},
    "description": {name}
  }}
}}
"#,
        name = esc_json(&s.name),
    )
}

// ---------- Forge 1.17 and newer (from the instance's own MDK) ----------

/// The text files of a Forge MDK, by path inside the zip.
pub type Mdk = BTreeMap<String, String>;

/// Build tools for an MDK: its Gradle and Java versions, with Gradle moved to a
/// JDK it can run on (Gradle before 8.5 can't run on Java 21).
pub fn forge_mdk_toolchain(mdk: &Mdk, fallback_java: u32) -> Result<Toolchain> {
    let wrapper = mdk.get("gradle/wrapper/gradle-wrapper.properties").map(String::as_str).unwrap_or("");
    let gradle = wrapper
        .lines()
        .find_map(|l| l.trim().strip_prefix("distributionUrl="))
        .and_then(|u| u.rsplit('/').next())
        .and_then(|f| f.strip_prefix("gradle-"))
        .and_then(|f| f.split('-').next())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("this Forge MDK doesn't say which Gradle it needs"))?;
    let build = mdk.get("build.gradle").map(String::as_str).unwrap_or("");
    let java_target = build
        .split("JavaLanguageVersion.of(")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(fallback_java);
    let plugin_version = build
        .split("net.minecraftforge.gradle' version '")
        .nth(1)
        .or_else(|| build.split("name: 'ForgeGradle', version: '").nth(1))
        .and_then(|r| r.split('\'').next())
        .unwrap_or("?")
        .to_string();
    let mut parts = gradle.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let (gradle_version, gradle_jdk) = match (major, minor) {
        (0..=6, _) => bail!("this Forge MDK needs Gradle {gradle}, which can't run on a current Java"),
        // VERIFY: ForgeGradle 5 (Minecraft 1.17–1.18.1) on Gradle 7.6 was never built.
        (7, _) => ("7.6.4".to_string(), 17),
        (8, 0..=4) => (gradle, 17),
        _ => (gradle, java_target.max(21)),
    };
    Ok(Toolchain { java_target, gradle_version, gradle_jdk, plugin_version })
}

/// A new project from a Forge MDK: its build files and resources with the example
/// mod's names replaced, and a minimal main class in place of the example's.
pub fn render_forge_mdk(spec: &ModSpec, mdk: &Mdk) -> BTreeMap<String, String> {
    let mut f = BTreeMap::new();
    let pkg = spec.package();
    let pkg_path = pkg.replace('.', "/");
    let class = spec.class_name();
    f.insert(".gitignore".into(), GITIGNORE.into());
    f.insert("CLAUDE.md".into(), claude_md(spec));
    let rename = |t: &str| t.replace("com.example.examplemod", &pkg).replace("examplemod", &spec.mod_id);
    for (path, text) in mdk {
        let keep = matches!(path.as_str(), "build.gradle" | "settings.gradle" | "gradle.properties")
            || path.starts_with("src/main/resources/");
        if !keep {
            continue;
        }
        let mut t = rename(text);
        match path.as_str() {
            "settings.gradle" => t = strip_foojay(&t),
            "gradle.properties" => t = mdk_props(&t, spec),
            p if p.ends_with("mods.toml") => t = mdk_mods_toml(&t, spec),
            _ => {}
        }
        f.insert(rename(path), t);
    }
    let java = mdk.iter().filter(|(p, _)| p.ends_with(".java")).map(|(_, t)| t.as_str()).collect::<String>();
    f.insert(format!("src/main/java/{pkg_path}/{class}.java"), forge_main(spec, &pkg, &class, &java));
    f.insert(format!("src/main/resources/assets/{}/lang/en_us.json", spec.mod_id), "{
}
".into());
    f
}

/// Drop the foojay toolchain-download plugin: the launcher supplies the JDKs.
fn strip_foojay(t: &str) -> String {
    let lines: Vec<&str> = t.lines().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("plugins") {
            if let Some(end) = (i..lines.len()).find(|&j| lines[j].trim() == "}") {
                if lines[i..=end].iter().any(|l| l.contains("foojay-resolver")) {
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push(lines[i]);
        i += 1;
    }
    out.join("
") + "
"
}

/// Properties-file value: backslashes escaped, newlines as `\n`.
fn prop_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn mdk_props(t: &str, s: &ModSpec) -> String {
    t.lines()
        .map(|l| {
            // Some MDKs paste these into quoted TOML strings, so no double quotes.
            let set = |k: &str, v: &str| format!("{k}={}", prop_value(&v.replace('"', "'")));
            if l.starts_with("mod_name=") {
                set("mod_name", &s.name)
            } else if l.starts_with("mod_authors=") {
                set("mod_authors", &s.author)
            } else if l.starts_with("mod_description=") {
                set("mod_description", &s.description)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("
")
        + "
"
}

/// Older MDKs write the example's name, authors and description into mods.toml
/// itself; newer ones fill them in from gradle.properties.
fn mdk_mods_toml(t: &str, s: &ModSpec) -> String {
    let mut t = t
        .replace("displayName=\"Example Mod\"", &format!("displayName=\"{}\"", esc_toml(&s.name)))
        .replace("authors=\"YourNameHere, OtherNameHere\"", &format!("authors=\"{}\"", esc_toml(&s.author)));
    if let Some(start) = t.find("description='''") {
        let body = start + "description='''".len();
        if let Some(len) = t[body..].find("'''") {
            if !t[body..body + len].contains("${") {
                t.replace_range(body..body + len, &s.description.replace("'''", "'"));
            }
        }
    }
    t
}

/// A main class that does nothing but log, built like the MDK's example (whose
/// constructor takes the loading context from Forge 1.21).
fn forge_main(s: &ModSpec, pkg: &str, class: &str, mdk_java: &str) -> String {
    let context = mdk_java.contains("FMLJavaModLoadingContext context");
    let (log_imports, logger) = if mdk_java.contains("com.mojang.logging.LogUtils") {
        ("import com.mojang.logging.LogUtils;\nimport org.slf4j.Logger;", "LogUtils.getLogger()")
    } else {
        ("import org.apache.logging.log4j.LogManager;\nimport org.apache.logging.log4j.Logger;", "LogManager.getLogger()")
    };
    format!(
        r#"package {pkg};

{log_imports}
import net.minecraftforge.fml.common.Mod;
{ctx_import}
@Mod({class}.MOD_ID)
public class {class} {{
    public static final String MOD_ID = "{id}";
    public static final Logger LOGGER = {logger};

    public {class}({ctx_param}) {{
        LOGGER.info("{name} loaded");
    }}
}}
"#,
        ctx_import = if context { "import net.minecraftforge.fml.javafmlmod.FMLJavaModLoadingContext;\n" } else { "" },
        ctx_param = if context { "FMLJavaModLoadingContext context" } else { "" },
        id = s.mod_id,
        name = s.name.replace('"', "'"),
    )
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
    "fabricloader": "{loader_dep}",
    "minecraft": "~{mc}",
    "java": ">={java_dep}",
    "fabric-api": "*"
  }}
}}
"#,
        id = esc_json(&s.mod_id),
        name = esc_expand(esc_json(&s.name)),
        desc = esc_expand(esc_json(&s.description)),
        author = esc_expand(esc_json(&s.author)),
        loader_dep = if s.loader == Loader::Quilt { "*".to_string() } else { format!(">={}", s.loader_version) },
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
    let loader = s.loader.display_name();
    let mappings = if is_unobfuscated(&s.mc_version) {
        "none (Minecraft is unobfuscated from 26.1)".to_string()
    } else {
        "official Mojang mappings".to_string()
    };
    let rfg = s.loader == Loader::Forge && is_rfg_forge(&s.mc_version);
    let mappings = if rfg { "MCP names (RetroFuturaGradle)".to_string() } else { mappings };
    let docs = match s.loader {
        Loader::Fabric | Loader::Quilt => "https://docs.fabricmc.net/develop/",
        Loader::Forge if s.mc_version == "1.12.2" => "https://docs.minecraftforge.net/en/1.12.x/",
        Loader::Forge if rfg => "https://github.com/GTNewHorizons/RetroFuturaGradle",
        Loader::Forge => "https://docs.minecraftforge.net/",
        _ => "https://docs.neoforged.net/",
    };
    let plugin_name = match s.loader {
        Loader::Fabric | Loader::Quilt => "Fabric Loom",
        Loader::Forge if rfg => "RetroFuturaGradle",
        Loader::Forge => "ForgeGradle",
        _ => "ModDevGradle",
    };
    let loader_line = if s.loader == Loader::Quilt {
        format!("Fabric Loader {} (to build; Quilt runs Fabric mods)", s.loader_version)
    } else {
        s.loader_version.clone()
    };
    let notes = match s.loader {
        Loader::Quilt => "\n## Quilt\n\nThis is a Fabric mod: Quilt loads Fabric mods, and the dev client runs it on\nFabric Loader. On Quilt, Fabric API comes from Quilted Fabric API (Modrinth\nproject `qsl`); if the instance doesn't have it, install it with\n`install_online_mod` before testing there.\n",
        Loader::Forge if rfg => "\n## Forge on Java 8\n\nMinecraft 1.7.10 and 1.12.2 run on Java 8, so write Java 8 code (no `var`,\nrecords or text blocks). Mod metadata is in `src/main/resources/mcmod.info`.\nLanguage files are `.lang` files: `assets/<mod id>/lang/en_us.lang` on 1.12.2,\n`en_US.lang` on 1.7.10. Assets are only loaded for the mod id in lowercase.\n",
        Loader::Forge => "\n## Forge\n\nThe build files come from Forge's own MDK for this Forge version, with the\nexample mod's names replaced. Mod details (name, authors, description) are in\n`gradle.properties` or `src/main/resources/META-INF/mods.toml`.\n",
        _ => "",
    };
    let version_file = match s.loader {
        Loader::Fabric | Loader::Quilt => "fabric.mod.json",
        Loader::Forge if rfg => "mcmod.info (it's filled in from gradle.properties)",
        _ => "the mods.toml",
    };
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

Bump `mod_version` in `gradle.properties` for releases (and check {version_file}).
Keep the mod id unchanged.
{notes}"#,
        name = s.name,
        mc = s.mc_version,
        id = s.mod_id,
        pkg = s.package(),
        class = s.class_name(),
        java = s.toolchain.java_target,
        loaderv = loader_line,
        gradle = s.toolchain.gradle_version,
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

    #[test]
    fn support() {
        assert!(unsupported_reason(Loader::Quilt, "1.20.1").is_none());
        assert!(unsupported_reason(Loader::Forge, "1.7.10").is_none());
        assert!(unsupported_reason(Loader::Forge, "1.12.2").is_none());
        assert!(unsupported_reason(Loader::Forge, "1.17.1").is_none());
        assert!(unsupported_reason(Loader::Forge, "26.3").is_none());
        assert!(unsupported_reason(Loader::Forge, "1.16.5").is_some());
        assert!(unsupported_reason(Loader::Forge, "1.8.9").is_some());
        assert!(unsupported_reason(Loader::LegacyFabric, "1.8.9").is_some());
        assert!(unsupported_reason(Loader::Vanilla, "1.21.1").is_some());
    }

    #[test]
    fn quilt_is_a_fabric_mod() {
        let f = render(&spec(Loader::Quilt, "1.20.1", 17));
        let mj: serde_json::Value = serde_json::from_str(&f["src/main/resources/fabric.mod.json"]).unwrap();
        assert_eq!(mj["depends"]["fabricloader"], "*");
        assert!(f["CLAUDE.md"].contains("Quilted Fabric API"));
    }

    #[test]
    fn rfg_files() {
        let s = spec(Loader::Forge, "1.12.2", 8);
        assert_eq!(s.toolchain.gradle_version, RFG_GRADLE);
        let f = render(&s);
        let main = &f["src/main/java/com/claudemods/glow_blocks/GlowBlocksMod.java"];
        assert!(main.contains("import net.minecraftforge.fml.common.Mod;"));
        assert!(main.contains(r#"NAME = "Glow \"Blocks\"""#));
        assert!(f["build.gradle"].contains("mcVersion.set('1.12.2')"));
        let info: serde_json::Value = serde_json::from_str(&f["src/main/resources/mcmod.info"]).unwrap();
        assert_eq!(info[0]["modid"], "glow_blocks");
        let pack: serde_json::Value = serde_json::from_str(&f["src/main/resources/pack.mcmeta"]).unwrap();
        assert_eq!(pack["pack"]["description"], "Glow \"Blocks\"");
        assert_eq!(pack["pack"]["pack_format"], 3);
        let old = render(&spec(Loader::Forge, "1.7.10", 8));
        assert!(old["src/main/java/com/claudemods/glow_blocks/GlowBlocksMod.java"].contains("import cpw.mods.fml.common.Mod;"));
    }

    fn mdk(gradle: &str, java: u32, ctx: bool) -> Mdk {
        let mut m = Mdk::new();
        m.insert(
            "gradle/wrapper/gradle-wrapper.properties".into(),
            format!("distributionUrl=https\\://services.gradle.org/distributions/gradle-{gradle}-bin.zip\n"),
        );
        m.insert(
            "build.gradle".into(),
            format!("plugins {{\n    id 'net.minecraftforge.gradle' version '[6.0,6.2)'\n}}\njava.toolchain.languageVersion = JavaLanguageVersion.of({java})\nrootProject.name = 'examplemod'\n"),
        );
        m.insert(
            "settings.gradle".into(),
            "pluginManagement {\n}\n\nplugins {\n    id 'org.gradle.toolchains.foojay-resolver-convention' version '0.7.0'\n}\n".into(),
        );
        m.insert("gradle.properties".into(), "mod_id=examplemod\nmod_name=Example Mod\nmod_group_id=com.example.examplemod\n".into());
        m.insert(
            "src/main/resources/META-INF/mods.toml".into(),
            "modId=\"examplemod\"\ndisplayName=\"Example Mod\"\ndescription='''Example mod description.\nMore.'''\n".into(),
        );
        m.insert(
            "src/main/java/com/example/examplemod/ExampleMod.java".into(),
            format!("import com.mojang.logging.LogUtils;\npublic ExampleMod({}) {{}}\n", if ctx { "FMLJavaModLoadingContext context" } else { "" }),
        );
        m
    }

    #[test]
    fn forge_mdk_tools() {
        let t = forge_mdk_toolchain(&mdk("8.8", 17, false), 17).unwrap();
        assert_eq!((t.gradle_version.as_str(), t.gradle_jdk, t.java_target), ("8.8", 21, 17));
        assert_eq!(t.plugin_version, "[6.0,6.2)");
        let t = forge_mdk_toolchain(&mdk("7.2", 16, false), 16).unwrap();
        assert_eq!((t.gradle_version.as_str(), t.gradle_jdk), ("7.6.4", 17));
        let t = forge_mdk_toolchain(&mdk("9.7.1", 25, true), 25).unwrap();
        assert_eq!((t.gradle_version.as_str(), t.gradle_jdk), ("9.7.1", 25));
        assert!(forge_mdk_toolchain(&mdk("6.9", 8, false), 8).is_err());
    }

    #[test]
    fn forge_mdk_files() {
        let m = mdk("9.7.1", 25, true);
        let mut s = spec(Loader::Fabric, "26.3", 25);
        s.loader = Loader::Forge;
        s.toolchain = forge_mdk_toolchain(&m, 25).unwrap();
        let f = render_forge_mdk(&s, &m);
        assert!(!f.keys().any(|k| k.contains("examplemod")));
        assert!(!f.values().any(|v| v.contains("examplemod")));
        assert!(!f["settings.gradle"].contains("foojay"));
        assert!(f["settings.gradle"].contains("pluginManagement"));
        assert_eq!(f["gradle.properties"].lines().nth(1), Some("mod_name=Glow 'Blocks'"));
        let toml = &f["src/main/resources/META-INF/mods.toml"];
        assert!(toml.contains(r#"displayName="Glow \"Blocks\"""#));
        assert!(toml.contains("description='''Things glow.'''"));
        let main = &f["src/main/java/com/claudemods/glow_blocks/GlowBlocksMod.java"];
        assert!(main.contains("public GlowBlocksMod(FMLJavaModLoadingContext context)"));
        let old = render_forge_mdk(&s, &mdk("8.8", 17, false));
        assert!(old["src/main/java/com/claudemods/glow_blocks/GlowBlocksMod.java"].contains("public GlowBlocksMod()"));
    }
}
