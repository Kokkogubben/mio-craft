//! Desktop shell. All launcher logic lives in `launcher-core`; this starts it,
//! serves the local API, and gives the web UI the API address plus a few
//! OS helpers (open a folder, open a URL, find the bundled MCP server) and
//! app updates.

use launcher_core::api::ApiInfo;
use launcher_core::{Launcher, Paths};
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

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
        .resolve("mcp/mio-craft-mcp.mjs", tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;
    // Strip Windows' verbatim prefix so the path pastes cleanly into a terminal.
    let s = p.to_string_lossy().to_string();
    Ok(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

#[tauri::command]
fn data_dir(state: tauri::State<'_, AppState>) -> String {
    state.data_dir.to_string_lossy().to_string()
}

#[derive(serde::Serialize)]
struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
}

/// Look for a newer release. Returns nothing when up to date, and always in
/// development builds (there is no installed app to replace).
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| UpdateInfo { version: u.version.clone(), current_version: u.current_version.clone(), notes: u.body.clone() }))
}

/// Download the newest release, check its signature, install it and restart.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(l) = app.try_state::<Launcher>() {
        if !l.processes.running().await.is_empty() {
            return Err("Close the game and any test client before updating.".into());
        }
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "MioCraft is already up to date.".to_string())?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    if let Some(l) = app.try_state::<Launcher>() {
        let _ = std::fs::remove_file(l.paths.api_file());
    }
    app.restart()
}

#[tauri::command]
fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
        .invoke_handler(tauri::generate_handler![
            get_api,
            open_path,
            open_url,
            mcp_server_path,
            data_dir,
            check_update,
            install_update,
            app_version
        ])
        .build(tauri::generate_context!())
        .expect("error while building MioCraft")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Tell the MCP server we're gone.
                if let Some(l) = app.try_state::<Launcher>() {
                    let _ = std::fs::remove_file(l.paths.api_file());
                }
            }
        });
}
