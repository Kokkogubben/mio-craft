//! Desktop shell. All launcher logic lives in `launcher-core`; this starts it,
//! serves the local API, and gives the web UI the API address plus a few
//! OS helpers (open a folder, open a URL, find the bundled MCP server).

use launcher_core::api::ApiInfo;
use launcher_core::{Launcher, Paths};
use tauri::Manager;

struct AppState {
    api: ApiInfo,
    data_dir: std::path::PathBuf,
}

#[tauri::command]
fn get_api(state: tauri::State<'_, AppState>) -> ApiInfo {
    state.api.clone()
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    }
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links can be opened".into());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Path of the bundled MCP server script, for the "Connect Claude Code" command.
#[tauri::command]
fn mcp_server_path(app: tauri::AppHandle) -> Result<String, String> {
    let p = app
        .path()
        .resolve("mcp/claude-launcher-mcp.mjs", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;
    // Strip Windows' verbatim prefix so the path pastes cleanly into a terminal.
    let s = p.to_string_lossy().to_string();
    Ok(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

#[tauri::command]
fn data_dir(state: tauri::State<'_, AppState>) -> String {
    state.data_dir.to_string_lossy().to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let (launcher, api) = tauri::async_runtime::block_on(async {
                let l = Launcher::new(Paths::new(Paths::default_root())).await?;
                let api = launcher_core::api::serve(l.clone()).await?;
                Ok::<_, anyhow::Error>((l, api))
            })
            .map_err(|e| e.to_string())?;
            app.manage(AppState { api, data_dir: launcher.paths.root.clone() });
            app.manage(launcher);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_api, open_path, open_url, mcp_server_path, data_dir])
        .build(tauri::generate_context!())
        .expect("error while building Claude Launcher")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Tell the MCP server we're gone.
                if let Some(l) = app.try_state::<Launcher>() {
                    let _ = std::fs::remove_file(l.paths.api_file());
                }
            }
        });
}
