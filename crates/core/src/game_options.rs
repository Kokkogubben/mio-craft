//! Copying game settings (`options.txt`: keybinds, video, sound, accessibility…)
//! from one instance, the official launcher's folder or a file into other instances.
//!
//! What the game does with the file decides the rules (checked against 26.3):
//! - Every value is read according to the `version:` line, and older files are
//!   upgraded on load. A file without that line is treated as very old, so modern
//!   keybind names in it make the whole file fail to load.
//! - A value the game can't read is skipped on its own and the rest still loads.
//! - Keybinds changed format in 1.13 (17w43a): numbers (LWJGL 2) before,
//!   names like `key.keyboard.w` after. A number in a modern file unbinds the key.
//!
//! So into an existing file we merge the chosen settings, keep its `version:` line
//! and convert keybinds to its format. A new file gets the source's settings as
//! they are, with the source's `version:` line, and the game upgrades them
//! (except for a pre-1.13 instance, which gets converted keybinds and no line).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::{instances, Launcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Keybinds (including mods' keybinds) and mouse settings.
    Controls,
    Video,
    Sound,
    Accessibility,
    Chat,
    /// Skin parts and main hand.
    Skin,
    Language,
}

const CONTROLS: &[&str] = &[
    "mouseSensitivity", "invertYMouse", "invertXMouse", "rawMouseInput", "discrete_mouse_scroll",
    "mouseWheelSensitivity", "toggleCrouch", "toggleSprint", "toggleAttack", "toggleUse", "sprintWindow",
    "autoJump", "touchscreen", "operatorItemsTab", "rotateWithMinecart", "allowCursorChanges",
    "quitShortcuts", "ctrlClickEmulatesRightClick",
];
const VIDEO: &[&str] = &[
    "fov", "gamma", "renderDistance", "simulationDistance", "guiScale", "maxFps", "inactivityFpsLimit",
    "enableVsync", "fullscreen", "exclusiveFullscreen", "fullscreenResolution", "graphicsMode", "graphicsPreset",
    "preferredGraphicsBackend", "fancyGraphics", "ao", "renderClouds", "cloudRange", "particles",
    "entityShadows", "entityDistanceScaling", "biomeBlendRadius", "mipmapLevels", "maxAnisotropyBit",
    "textureFiltering", "improvedTransparency", "cutoutLeaves", "chunkSectionFadeInTime", "weatherRadius",
    "vignette", "bobView", "prioritizeChunkUpdates", "attackIndicator", "showAutosaveIndicator",
    "saturation", "anaglyph3d", "useVbo", "fboEnable",
];
const SOUND: &[&str] = &["soundDevice", "showSubtitles", "directionalAudio", "musicToast", "musicFrequency"];
const ACCESSIBILITY: &[&str] = &[
    "narrator", "narratorHotkey", "highContrast", "highContrastBlockOutline", "textBackgroundOpacity",
    "backgroundForChatOnly", "chatLineSpacing", "darkMojangStudiosBackground", "hideLightningFlashes",
    "hideSplashTexts", "screenEffectScale", "fovEffectScale", "darknessEffectScale", "damageTiltStrength",
    "glintSpeed", "glintStrength", "notificationDisplayTime", "panoramaScrollSpeed",
    "menuBackgroundBlurriness", "forceUnicodeFont", "japaneseGlyphVariants",
];
const CHAT: &[&str] = &[
    "chatVisibility", "chatColors", "chatLinks", "chatLinksPrompt", "chatOpacity", "chatScale", "chatWidth",
    "chatHeightFocused", "chatHeightUnfocused", "chatDelay", "autoSuggestions", "onlyShowSecureChat",
    "hideMatchedNames", "saveChatDrafts",
];

/// The category a setting belongs to. Settings in none (the version line, resource
/// packs, last server, first-run flags and the like) are never copied.
pub fn category_of(key: &str) -> Option<Category> {
    if key.starts_with("key_") {
        return Some(Category::Controls);
    }
    if key.starts_with("soundCategory_") {
        return Some(Category::Sound);
    }
    if key.starts_with("modelPart_") || key == "mainHand" {
        return Some(Category::Skin);
    }
    if key == "lang" {
        return Some(Category::Language);
    }
    for (list, cat) in [
        (CONTROLS, Category::Controls),
        (VIDEO, Category::Video),
        (SOUND, Category::Sound),
        (ACCESSIBILITY, Category::Accessibility),
        (CHAT, Category::Chat),
    ] {
        if list.contains(&key) {
            return Some(cat);
        }
    }
    None
}

/// `options.txt` as ordered key/value pairs (split at the first `:`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options(pub Vec<(String, String)>);

impl Options {
    pub fn parse(text: &str) -> Self {
        Options(
            text.lines()
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .filter(|(k, _)| !k.is_empty())
                .collect(),
        )
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub(crate) fn set(&mut self, key: &str, value: String) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(e) => e.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
    pub fn render(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}:{v}\n")).collect()
    }
    /// Whether keybinds are names (1.13+) rather than numbers, judged from the keybinds themselves.
    fn named_keys(&self) -> Option<bool> {
        self.0
            .iter()
            .filter(|(k, _)| k.starts_with("key_"))
            .find_map(|(_, v)| if v.starts_with("key.") || v.starts_with("scancode.") { Some(true) } else if v.parse::<i32>().is_ok() { Some(false) } else { None })
    }
}

/// Whether a Minecraft version uses named keybinds (1.13, from snapshot 17w43a).
pub fn version_named_keys(mc: &str) -> bool {
    if let Some((yy, rest)) = mc.split_once('w') {
        if let (Ok(y), Ok(w)) = (yy.parse::<u32>(), rest.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<u32>()) {
            return (y, w) >= (17, 43);
        }
    }
    if mc.starts_with(['a', 'b', 'c']) || mc.starts_with("rd-") || mc.starts_with("inf-") {
        return false;
    }
    let mut parts = mc.split(['.', '-', ' ']).map(|p| p.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(1), Some(minor)) => minor >= 13,
        (Some(major), _) => major > 1,
        _ => true,
    }
}

/// Whether a version wants language codes like `en_US` (before 1.11) rather than `en_us`.
fn version_upper_lang(mc: &str) -> bool {
    if let Some((yy, rest)) = mc.split_once('w') {
        if let (Ok(y), Ok(w)) = (yy.parse::<u32>(), rest.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<u32>()) {
            return (y, w) < (16, 32);
        }
    }
    let mut parts = mc.split(['.', '-', ' ']).map(|p| p.parse::<u32>().ok());
    matches!((parts.next().flatten(), parts.next().flatten()), (Some(1), Some(minor)) if minor < 11)
        || mc.starts_with(['a', 'b', 'c'])
}

fn lang_for(value: &str, upper: bool) -> String {
    match value.split_once('_') {
        Some((l, r)) if upper => format!("{}_{}", l.to_ascii_lowercase(), r.to_ascii_uppercase()),
        _ => value.to_ascii_lowercase(),
    }
}

/// LWJGL 2 key codes (keybinds before 1.13) and the names that replaced them.
const KEYS: &[(i32, &str)] = &[
    (0, "key.keyboard.unknown"), (1, "key.keyboard.escape"), (2, "key.keyboard.1"), (3, "key.keyboard.2"),
    (4, "key.keyboard.3"), (5, "key.keyboard.4"), (6, "key.keyboard.5"), (7, "key.keyboard.6"),
    (8, "key.keyboard.7"), (9, "key.keyboard.8"), (10, "key.keyboard.9"), (11, "key.keyboard.0"),
    (12, "key.keyboard.minus"), (13, "key.keyboard.equal"), (14, "key.keyboard.backspace"), (15, "key.keyboard.tab"),
    (16, "key.keyboard.q"), (17, "key.keyboard.w"), (18, "key.keyboard.e"), (19, "key.keyboard.r"),
    (20, "key.keyboard.t"), (21, "key.keyboard.y"), (22, "key.keyboard.u"), (23, "key.keyboard.i"),
    (24, "key.keyboard.o"), (25, "key.keyboard.p"), (26, "key.keyboard.left.bracket"), (27, "key.keyboard.right.bracket"),
    (28, "key.keyboard.enter"), (29, "key.keyboard.left.control"), (30, "key.keyboard.a"), (31, "key.keyboard.s"),
    (32, "key.keyboard.d"), (33, "key.keyboard.f"), (34, "key.keyboard.g"), (35, "key.keyboard.h"),
    (36, "key.keyboard.j"), (37, "key.keyboard.k"), (38, "key.keyboard.l"), (39, "key.keyboard.semicolon"),
    (40, "key.keyboard.apostrophe"), (41, "key.keyboard.grave.accent"), (42, "key.keyboard.left.shift"), (43, "key.keyboard.backslash"),
    (44, "key.keyboard.z"), (45, "key.keyboard.x"), (46, "key.keyboard.c"), (47, "key.keyboard.v"),
    (48, "key.keyboard.b"), (49, "key.keyboard.n"), (50, "key.keyboard.m"), (51, "key.keyboard.comma"),
    (52, "key.keyboard.period"), (53, "key.keyboard.slash"), (54, "key.keyboard.right.shift"), (55, "key.keyboard.keypad.multiply"),
    (56, "key.keyboard.left.alt"), (57, "key.keyboard.space"), (58, "key.keyboard.caps.lock"), (59, "key.keyboard.f1"),
    (60, "key.keyboard.f2"), (61, "key.keyboard.f3"), (62, "key.keyboard.f4"), (63, "key.keyboard.f5"),
    (64, "key.keyboard.f6"), (65, "key.keyboard.f7"), (66, "key.keyboard.f8"), (67, "key.keyboard.f9"),
    (68, "key.keyboard.f10"), (69, "key.keyboard.num.lock"), (70, "key.keyboard.scroll.lock"), (71, "key.keyboard.keypad.7"),
    (72, "key.keyboard.keypad.8"), (73, "key.keyboard.keypad.9"), (74, "key.keyboard.keypad.subtract"), (75, "key.keyboard.keypad.4"),
    (76, "key.keyboard.keypad.5"), (77, "key.keyboard.keypad.6"), (78, "key.keyboard.keypad.add"), (79, "key.keyboard.keypad.1"),
    (80, "key.keyboard.keypad.2"), (81, "key.keyboard.keypad.3"), (82, "key.keyboard.keypad.0"), (83, "key.keyboard.keypad.decimal"),
    (87, "key.keyboard.f11"), (88, "key.keyboard.f12"), (100, "key.keyboard.f13"), (101, "key.keyboard.f14"),
    (102, "key.keyboard.f15"), (156, "key.keyboard.keypad.enter"), (157, "key.keyboard.right.control"), (181, "key.keyboard.keypad.divide"),
    (183, "key.keyboard.print.screen"), (184, "key.keyboard.right.alt"), (197, "key.keyboard.pause"), (199, "key.keyboard.home"),
    (200, "key.keyboard.up"), (201, "key.keyboard.page.up"), (203, "key.keyboard.left"), (205, "key.keyboard.right"),
    (207, "key.keyboard.end"), (208, "key.keyboard.down"), (209, "key.keyboard.page.down"), (210, "key.keyboard.insert"),
    (211, "key.keyboard.delete"), (219, "key.keyboard.left.win"), (220, "key.keyboard.right.win"), (221, "key.keyboard.menu"),
];

/// A keybind value in the other format; None when it has no match (the key is left out).
fn convert_key(value: &str, to_named: bool) -> Option<String> {
    if to_named {
        let code: i32 = value.parse().ok()?;
        // Mouse buttons were -100 (left), -99 (right), -98 (middle), -97 (button 4)…
        if (-100..0).contains(&code) {
            return Some(match code + 100 {
                0 => "key.mouse.left".into(),
                1 => "key.mouse.right".into(),
                2 => "key.mouse.middle".into(),
                n => format!("key.mouse.{}", n + 1),
            });
        }
        KEYS.iter().find(|(c, _)| *c == code).map(|(_, n)| n.to_string())
    } else {
        if let Some(m) = value.strip_prefix("key.mouse.") {
            let n = match m {
                "left" => 0,
                "right" => 1,
                "middle" => 2,
                n => n.parse::<i32>().ok()? - 1,
            };
            return Some((n - 100).to_string());
        }
        KEYS.iter().find(|(_, n)| *n == value).map(|(c, _)| c.to_string())
    }
}

/// The chosen settings of `src` written over `dst`, an instance's existing file
/// (or None when it has none). `named_keys`/`upper_lang` describe the instance's
/// Minecraft version. Returns the new file and how many settings were copied.
pub fn merge(src: &Options, dst: Option<&Options>, cats: &HashSet<Category>, named_keys: bool, upper_lang: bool) -> (Options, usize) {
    let picked: Vec<&(String, String)> = src.0.iter().filter(|(k, _)| category_of(k).is_some_and(|c| cats.contains(&c))).collect();
    let src_named = src.named_keys();
    let mut count = 0;
    let mut out = match dst {
        Some(d) => d.clone(),
        None => {
            // A new file: the source's own settings and version line, which the game
            // upgrades itself, unless the instance predates named keybinds.
            if !(src_named == Some(true) && !named_keys) {
                let mut o = Options::default();
                match src.get("version") {
                    Some(v) => o.set("version", v.to_string()),
                    // Named keybinds need a version line, or the game reads them as numbers
                    // and throws the whole file away. 1519 is 1.13.
                    None if src_named == Some(true) => o.set("version", "1519".into()),
                    None => {}
                }
                for (k, v) in picked {
                    // Language codes are matched exactly, and the upgrade from older files doesn't always fix their case.
                    o.set(k, if k == "lang" { lang_for(v, upper_lang) } else { v.clone() });
                    count += 1;
                }
                return (o, count);
            }
            Options::default()
        }
    };
    // Into an existing file (or an old version): its format wins.
    let dst_named = dst.and_then(Options::named_keys).unwrap_or(named_keys);
    for (k, v) in picked {
        let value = if k.starts_with("key_") && src_named.is_some_and(|s| s != dst_named) {
            match convert_key(v, dst_named) {
                Some(c) => c,
                None => continue,
            }
        } else if k == "lang" {
            lang_for(v, upper_lang)
        } else {
            v.clone()
        };
        out.set(k, value);
        count += 1;
    }
    (out, count)
}

// ---------- files ----------

/// Where the official Minecraft launcher keeps the game folder.
pub fn official_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library/Application Support/minecraft"))
    } else if cfg!(windows) {
        dirs::config_dir().map(|d| d.join(".minecraft"))
    } else {
        dirs::home_dir().map(|h| h.join(".minecraft"))
    }
}

const MAX_OPTIONS: usize = 512 * 1024;

pub(crate) async fn read_options(path: &Path) -> Result<Option<Options>> {
    match tokio::fs::read(path).await {
        Ok(b) if b.len() > MAX_OPTIONS => bail!("{} is too big to be an options.txt", path.display()),
        Ok(b) => Ok(Some(Options::parse(&String::from_utf8_lossy(&b)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Serialize)]
pub struct Source {
    /// An instance id, or `minecraft` for the official launcher's folder.
    pub id: String,
    pub name: String,
    pub mc_version: Option<String>,
}

/// Places with an options.txt to copy from.
pub async fn sources(l: &Launcher) -> Result<Vec<Source>> {
    let mut out = vec![];
    if official_dir().is_some_and(|d| d.join("options.txt").is_file()) {
        out.push(Source { id: "minecraft".into(), name: "Minecraft Launcher".into(), mc_version: None });
    }
    for i in instances::list(l).await? {
        if l.paths.game_dir(&i.id).join("options.txt").is_file() {
            out.push(Source { id: i.id, name: i.name, mc_version: Some(i.mc_version) });
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct CopyRequest {
    /// An instance id or `minecraft`; or leave it out and send `text`.
    pub from: Option<String>,
    /// The contents of an options.txt file.
    pub text: Option<String>,
    pub to: Vec<String>,
    pub categories: Vec<Category>,
}

#[derive(Debug, Serialize)]
pub struct CopyResult {
    pub instance_id: String,
    pub copied: usize,
    pub error: Option<String>,
}

pub async fn copy(l: &Launcher, req: CopyRequest) -> Result<Vec<CopyResult>> {
    let cats: HashSet<Category> = req.categories.into_iter().collect();
    if cats.is_empty() {
        bail!("pick at least one kind of setting to copy");
    }
    let src = match (&req.from, &req.text) {
        (Some(id), _) if id == "minecraft" => {
            let dir = official_dir().ok_or_else(|| anyhow::anyhow!("couldn't find the Minecraft Launcher's folder"))?;
            read_options(&dir.join("options.txt")).await?
        }
        (Some(id), _) => {
            instances::get(l, id).await?;
            read_options(&l.paths.game_dir(id).join("options.txt")).await?
        }
        (None, Some(t)) if t.len() <= MAX_OPTIONS => Some(Options::parse(t)),
        (None, Some(_)) => bail!("that file is too big to be an options.txt"),
        (None, None) => bail!("say where to copy the settings from"),
    }
    .ok_or_else(|| anyhow::anyhow!("there are no settings there yet; play that instance once first"))?;
    if src.0.is_empty() {
        bail!("that file has no settings in it");
    }
    let mut results = vec![];
    for id in req.to {
        if req.from.as_deref() == Some(id.as_str()) {
            continue;
        }
        let r = copy_into(l, &src, &cats, &id).await;
        results.push(match r {
            Ok(copied) => CopyResult { instance_id: id, copied, error: None },
            Err(e) => CopyResult { instance_id: id, copied: 0, error: Some(e.to_string()) },
        });
    }
    Ok(results)
}

async fn copy_into(l: &Launcher, src: &Options, cats: &HashSet<Category>, id: &str) -> Result<usize> {
    let inst = instances::get(l, id).await?;
    if l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("{} is running; close the game first, since it saves its settings when it quits", inst.name);
    }
    let path = l.paths.game_dir(&inst.id).join("options.txt");
    let existing = read_options(&path).await?;
    let (merged, copied) = merge(src, existing.as_ref(), cats, version_named_keys(&inst.mc_version), version_upper_lang(&inst.mc_version));
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    if existing.is_some() {
        tokio::fs::copy(&path, path.with_extension("txt.bak")).await?;
    }
    let part = path.with_extension("txt.part");
    tokio::fs::write(&part, merged.render()).await?;
    tokio::fs::rename(&part, &path).await?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats(c: &[Category]) -> HashSet<Category> {
        c.iter().copied().collect()
    }

    const MODERN: &str = "version:5023\nfov:0.5\ngraphicsPreset:\"fancy\"\nlang:sv_se\nresourcePacks:[\"vanilla\"]\nlastServer:example.org\nnarrator:2\nkey_key.jump:key.keyboard.j\nkey_key.attack:key.mouse.right\nkey_key.zoom:key.mouse.4\nsoundCategory_master:0.25\nmodelPart_hat:false\n";
    const OLD: &str = "fov:0.25\nlang:de_DE\nkey_key.jump:36\nkey_key.forward:200\nkey_key.attack:-99\nfancyGraphics:true\n";

    #[test]
    fn categories() {
        assert_eq!(category_of("key_key.somemod.zoom"), Some(Category::Controls));
        assert_eq!(category_of("soundCategory_music"), Some(Category::Sound));
        assert_eq!(category_of("fov"), Some(Category::Video));
        assert_eq!(category_of("textBackgroundOpacity"), Some(Category::Accessibility));
        for never in ["version", "resourcePacks", "incompatibleResourcePacks", "lastServer", "onboardAccessibility", "tutorialStep"] {
            assert_eq!(category_of(never), None, "{never}");
        }
    }

    #[test]
    fn versions() {
        assert!(version_named_keys("1.13"));
        assert!(version_named_keys("1.21.1"));
        assert!(version_named_keys("26.3"));
        assert!(version_named_keys("17w43a"));
        assert!(!version_named_keys("17w42a"));
        assert!(!version_named_keys("1.12.2"));
        assert!(!version_named_keys("1.8.9"));
        assert!(!version_named_keys("b1.7.3"));
        assert!(version_upper_lang("1.8.9"));
        assert!(!version_upper_lang("1.12.2"));
    }

    #[test]
    fn keys_both_ways() {
        for (code, name) in [("36", "key.keyboard.j"), ("200", "key.keyboard.up"), ("-100", "key.mouse.left"), ("-99", "key.mouse.right"), ("-97", "key.mouse.4"), ("0", "key.keyboard.unknown")] {
            assert_eq!(convert_key(code, true).as_deref(), Some(name));
            assert_eq!(convert_key(name, false).as_deref(), Some(code));
        }
        assert_eq!(convert_key("key.keyboard.world.1", false), None);
    }

    #[test]
    fn merge_keeps_target_version_and_converts_keys() {
        let dst = Options::parse(OLD);
        let (out, n) = merge(&Options::parse(MODERN), Some(&dst), &cats(&[Category::Controls, Category::Language]), false, true);
        assert_eq!(out.get("version"), None);
        assert_eq!(out.get("key_key.jump"), Some("36"));
        assert_eq!(out.get("key_key.attack"), Some("-99"));
        assert_eq!(out.get("key_key.zoom"), Some("-97"));
        assert_eq!(out.get("key_key.forward"), Some("200"), "untouched");
        assert_eq!(out.get("fov"), Some("0.25"), "video wasn't picked");
        assert_eq!(out.get("lang"), Some("sv_SE"));
        assert_eq!(n, 4);

        let modern = Options::parse(MODERN);
        let (out, _) = merge(&Options::parse(OLD), Some(&modern), &cats(&[Category::Controls]), true, false);
        assert_eq!(out.get("version"), Some("5023"));
        assert_eq!(out.get("key_key.jump"), Some("key.keyboard.j"));
        assert_eq!(out.get("key_key.forward"), Some("key.keyboard.up"));
        assert_eq!(out.get("key_key.attack"), Some("key.mouse.right"));
    }

    #[test]
    fn never_copies_bookkeeping() {
        let all = cats(&[Category::Controls, Category::Video, Category::Sound, Category::Accessibility, Category::Chat, Category::Skin, Category::Language]);
        let dst = Options::parse("version:3465\nresourcePacks:[]\nlastServer:\n");
        let (out, _) = merge(&Options::parse(MODERN), Some(&dst), &all, true, false);
        assert_eq!(out.get("version"), Some("3465"));
        assert_eq!(out.get("resourcePacks"), Some("[]"));
        assert_eq!(out.get("lastServer"), Some(""));
        assert_eq!(out.get("narrator"), Some("2"));
        assert_eq!(out.get("modelPart_hat"), Some("false"));
    }

    #[test]
    fn new_files_let_the_game_upgrade() {
        let c = cats(&[Category::Controls, Category::Video]);
        // Old settings into a new modern instance: as they are, no version line.
        let (out, _) = merge(&Options::parse(OLD), None, &cats(&[Category::Controls, Category::Language]), true, false);
        assert_eq!(out.get("version"), None);
        assert_eq!(out.get("key_key.jump"), Some("36"));
        assert_eq!(out.get("lang"), Some("de_de"));
        // Modern into modern: with the source's version line.
        let (out, _) = merge(&Options::parse(MODERN), None, &c, true, false);
        assert_eq!(out.0[0], ("version".to_string(), "5023".to_string()));
        assert_eq!(out.get("key_key.jump"), Some("key.keyboard.j"));
        // Modern into an old instance: converted, no version line.
        let (out, _) = merge(&Options::parse(MODERN), None, &c, false, true);
        assert_eq!(out.get("version"), None);
        assert_eq!(out.get("key_key.jump"), Some("36"));
        // Named keybinds without a version line get one, or the game would drop the file.
        let (out, _) = merge(&Options::parse("key_key.jump:key.keyboard.j\n"), None, &c, true, false);
        assert_eq!(out.get("version"), Some("1519"));
    }

    #[test]
    fn parse_render_round_trip() {
        let o = Options::parse(MODERN);
        assert_eq!(o.render(), MODERN);
        assert_eq!(Options::parse("a:b:c\nnocolon\n").0, vec![("a".into(), "b:c".into())]);
    }
}
