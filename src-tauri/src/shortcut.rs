//! Desktop shortcuts that play an instance: `MioCraft.exe --play <instance id>`.
//! A second MioCraft started that way hands the request to the one already open
//! (see the single-instance plugin in `lib.rs`), which plays it the normal way.

use std::path::{Path, PathBuf};

pub const PLAY_ARG: &str = "--play";

/// The instance a command line asks to play, if any.
pub fn play_arg(args: &[String]) -> Option<String> {
    let i = args.iter().position(|a| a == PLAY_ARG)?;
    let id = args.get(i + 1)?;
    valid_id(id).then(|| id.clone())
}

/// Instance ids are folder names made from the instance's name: letters, digits and dashes.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 120 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !id.starts_with('-')
}

/// A file name for the shortcut, from the instance's name.
pub fn file_name(name: &str) -> String {
    let mut s: String = name.chars().filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c)).collect();
    s = s.trim().trim_end_matches(['.', ' ']).chars().take(60).collect::<String>().trim().to_string();
    if s.is_empty() {
        s = "Minecraft".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        s.push_str(" instance");
    }
    format!("{s}.lnk")
}

/// An `.ico` file made by the UI: the ICO header, and not huge.
pub fn valid_ico(bytes: &[u8]) -> bool {
    bytes.len() > 6 && bytes.len() < 1024 * 1024 && bytes[..4] == [0, 0, 1, 0]
}

/// Write a shortcut that runs `exe --play <id>`. Replaces an existing file only when asked.
#[cfg(windows)]
pub fn write(path: &Path, exe: &Path, id: &str, name: &str, icon: Option<&Path>) -> Result<(), String> {
    // Written next to it first, so a failed write never leaves half a shortcut.
    let tmp = path.with_extension("part.lnk");
    let (t, exe, args, desc, icon) = (
        tmp.clone(),
        exe.to_path_buf(),
        format!("{PLAY_ARG} {id}"),
        format!("Play {name} in MioCraft"),
        icon.map(Path::to_path_buf),
    );
    // Windows' own shell link object, on a thread of its own with its own COM setup.
    std::thread::spawn(move || -> windows::core::Result<()> {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        unsafe {
            let inited = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let r = (|| {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(exe.as_os_str()))?;
                link.SetArguments(&HSTRING::from(args.as_str()))?;
                link.SetDescription(&HSTRING::from(desc.as_str()))?;
                if let Some(dir) = exe.parent() {
                    link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                }
                if let Some(icon) = &icon {
                    link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
                }
                link.cast::<IPersistFile>()?.Save(&HSTRING::from(t.as_os_str()), true)
            })();
            if inited {
                CoUninitialize();
            }
            r
        }
    })
    .join()
    .map_err(|_| "Couldn't make the shortcut".to_string())?
    .map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't make the shortcut: {}", e.message())
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't save the shortcut: {e}")
    })
}

#[cfg(not(windows))]
pub fn write(_path: &Path, _exe: &Path, _id: &str, _name: &str, _icon: Option<&Path>) -> Result<(), String> {
    Err("Desktop shortcuts can only be made on Windows for now.".into())
}

/// Where the shortcut goes, and whether something is already there.
pub fn target(desktop: &Path, name: &str) -> (PathBuf, bool) {
    let p = desktop.join(file_name(name));
    let exists = p.exists();
    (p, exists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_play_argument() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(play_arg(&a(&["MioCraft.exe", "--play", "my-pack"])), Some("my-pack".into()));
        assert_eq!(play_arg(&a(&["MioCraft.exe"])), None);
        assert_eq!(play_arg(&a(&["MioCraft.exe", "--play"])), None);
        assert_eq!(play_arg(&a(&["MioCraft.exe", "--play", "../evil"])), None);
        assert_eq!(play_arg(&a(&["MioCraft.exe", "--play", "--other"])), None);
    }

    #[test]
    fn makes_safe_file_names() {
        assert_eq!(file_name("My Pack: 1.21?"), "My Pack 1.21.lnk");
        assert_eq!(file_name("  ...  "), "Minecraft.lnk");
        assert_eq!(file_name("con"), "con instance.lnk");
        assert_eq!(file_name("COM1.x"), "COM1.x instance.lnk");
        assert_eq!(file_name("Ünïcode ✦"), "Ünïcode ✦.lnk");
        assert_eq!(file_name(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn checks_icons() {
        assert!(valid_ico(&[0, 0, 1, 0, 1, 0, 0]));
        assert!(!valid_ico(b"\x89PNG\r\n\x1a\n"));
    }

    #[cfg(windows)]
    #[test]
    fn writes_a_shortcut_windows_reads() {
        let dir = std::env::temp_dir().join(format!("miocraft-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        let icon = dir.join("i.ico");
        std::fs::write(&icon, [0u8, 0, 1, 0, 0, 0]).unwrap();
        let (path, exists) = target(&dir, "Test ✦ pack");
        assert!(!exists);
        write(&path, &exe, "test-pack", "Test ✦ pack", Some(&icon)).unwrap();
        // Ask Windows itself what the shortcut points to.
        // Shell.Application, since WScript.Shell can't open paths with characters like ✦.
        let script = format!(
            "$f=(New-Object -ComObject Shell.Application).Namespace('{}'); $k=$f.ParseName('{}').GetLink; $k.Path; $k.Arguments; $p=''; [void]$k.GetIconLocation([ref]$p); $p",
            dir.display().to_string().replace('\'', "''"),
            path.file_name().unwrap().to_string_lossy().replace('\'', "''")
        );
        // A UTF-8 script file with a BOM, so Windows PowerShell reads the ✦ in the path correctly.
        let ps1 = dir.join("read.ps1");
        std::fs::write(&ps1, [b"\xEF\xBB\xBF".as_slice(), script.as_bytes()].concat()).unwrap();
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&ps1).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0].to_lowercase(), exe.to_string_lossy().to_lowercase(), "{text}");
        assert_eq!(lines[1], "--play test-pack");
        assert!(lines[2].to_lowercase().starts_with(&icon.to_string_lossy().to_lowercase()), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
