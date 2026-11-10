//! Local control API (127.0.0.1 only, bearer token). Used by the launcher's
//! own UI and by the MCP server that Claude Code runs.

use std::convert::Infallible;
use std::net::SocketAddr;

use anyhow::Result;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::{auth, browse, config, instances, launch, loaders, mods, mojang, process, projects, Launcher};

#[derive(Clone)]
struct AppState {
    l: Launcher,
    token: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApiInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
}

struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{:#}", self.0) }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

type R<T> = std::result::Result<Json<T>, ApiError>;

fn ok() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn require_token(
    State(s): State<AppState>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string);
    let given = bearer.or_else(|| q.get("token").cloned());
    if given.as_deref() != Some(s.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad or missing token" }))).into_response();
    }
    next.run(req).await
}

pub fn router(l: Launcher, token: String) -> Router {
    let state = AppState { l, token };
    let api = Router::new()
        .route("/status", get(status))
        .route("/config", get(get_config).patch(patch_config))
        .route("/versions", get(versions))
        .route("/loaders/{loader}/{mc}", get(loader_versions))
        .route("/accounts", get(accounts))
        .route("/accounts/login", post(login))
        .route("/accounts/{id}", delete(remove_account))
        .route("/instances", get(list_instances).post(create_instance))
        .route("/instances/{id}", get(get_instance).patch(patch_instance).delete(delete_instance))
        .route("/instances/{id}/install", post(install_instance))
        .route("/instances/{id}/launch", post(launch_instance))
        .route("/instances/{id}/stop", post(stop_instance))
        .route("/instances/{id}/logs", get(instance_logs))
        .route("/instances/{id}/mods", get(list_mods))
        .route("/instances/{id}/mods/{file}", delete(delete_mod))
        .route("/instances/{id}/mods/{file}/enabled", post(set_mod_enabled))
        .route("/instances/{id}/online-mods", post(install_online))
        .route("/browse/{source}/search", get(browse_search))
        .route("/browse/{source}/projects/{pid}/versions", get(browse_versions))
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/{id}", get(get_project))
        .route("/projects/{id}/build", post(build_project))
        .route("/projects/{id}/install", post(install_project))
        .route("/projects/{id}/dev", post(start_dev).delete(stop_dev))
        .route("/projects/{id}/dev/logs", get(dev_logs))
        .route("/events", get(events))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state);
    Router::new()
        .nest("/api", api)
        .layer(tower_http::cors::CorsLayer::permissive())
}

/// Bind on 127.0.0.1 (random port), write `api.json`, and serve forever.
pub async fn serve(l: Launcher) -> Result<ApiInfo> {
    let token = {
        use rand::Rng;
        let bytes: [u8; 24] = rand::rng().random();
        hex::encode(bytes)
    };
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = listener.local_addr()?.port();
    let info = ApiInfo {
        port,
        token: token.clone(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    crate::http::write_json(&l.paths.api_file(), &info).await?;
    let app = router(l, token);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("API server stopped: {e}");
        }
    });
    Ok(info)
}

// ---------- handlers ----------

async fn status(State(s): State<AppState>) -> R<Value> {
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "data_dir": s.l.paths.root,
        "running": s.l.processes.running().await,
        "signed_in": !auth::load_accounts(&s.l).await.is_empty(),
        "has_client_id": s.l.config.read().await.ms_client_id.is_some(),
        "has_curseforge_key": s.l.config.read().await.curseforge_api_key.is_some(),
    })))
}

async fn get_config(State(s): State<AppState>) -> R<config::Config> {
    Ok(Json(s.l.config.read().await.clone()))
}

async fn patch_config(State(s): State<AppState>, Json(p): Json<config::ConfigPatch>) -> R<config::Config> {
    let mut c = s.l.config.write().await;
    c.apply(p);
    c.save(&s.l.paths).await?;
    Ok(Json(c.clone()))
}

#[derive(Deserialize)]
struct VersionsQuery {
    #[serde(default)]
    snapshots: bool,
}

async fn versions(State(s): State<AppState>, Query(q): Query<VersionsQuery>) -> R<Value> {
    let m = mojang::manifest(&s.l).await?;
    let list: Vec<_> = m
        .versions
        .iter()
        .filter(|v| v.kind == "release" || (q.snapshots && v.kind == "snapshot"))
        .map(|v| json!({ "id": v.id, "type": v.kind, "release_time": v.release_time }))
        .collect();
    Ok(Json(json!({ "latest": m.latest, "versions": list })))
}

async fn loader_versions(State(s): State<AppState>, Path((loader, mc)): Path<(String, String)>) -> R<Vec<loaders::LoaderVersion>> {
    Ok(Json(match loader.as_str() {
        "fabric" => loaders::fabric_versions(&s.l, &mc).await?,
        "neoforge" => loaders::neoforge_versions(&s.l, &mc).await?,
        _ => vec![],
    }))
}

async fn accounts(State(s): State<AppState>) -> R<Vec<auth::AccountInfo>> {
    Ok(Json(auth::list(&s.l).await))
}

async fn login(State(s): State<AppState>) -> R<auth::DeviceCode> {
    let mut code = auth::begin_login(&s.l).await?;
    code.device_code.clear(); // never hand out the polling secret
    Ok(Json(code))
}

async fn remove_account(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    auth::remove(&s.l, &id).await?;
    Ok(ok())
}

async fn list_instances(State(s): State<AppState>) -> R<Vec<Value>> {
    let mut out = vec![];
    for i in instances::list(&s.l).await? {
        out.push(instance_json(&s.l, &i).await);
    }
    Ok(Json(out))
}

async fn instance_json(l: &Launcher, i: &instances::Instance) -> Value {
    let mut v = serde_json::to_value(i).unwrap_or_default();
    v["installed"] = json!(i.version_id.is_some());
    v["running"] = json!(l.processes.is_running(&launch::game_source(&i.id)).await);
    v["game_dir"] = json!(l.paths.game_dir(&i.id));
    v["mods_dir"] = json!(l.paths.mods_dir(&i.id));
    v
}

async fn create_instance(State(s): State<AppState>, Json(req): Json<instances::NewInstance>) -> R<Value> {
    let i = instances::create(&s.l, req).await?;
    Ok(Json(instance_json(&s.l, &i).await))
}

async fn get_instance(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    let i = instances::get(&s.l, &id).await?;
    Ok(Json(instance_json(&s.l, &i).await))
}

async fn patch_instance(State(s): State<AppState>, Path(id): Path<String>, Json(p): Json<instances::InstancePatch>) -> R<Value> {
    let i = instances::update(&s.l, &id, p).await?;
    Ok(Json(instance_json(&s.l, &i).await))
}

async fn delete_instance(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    instances::delete(&s.l, &id).await?;
    Ok(ok())
}

async fn install_instance(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    let i = instances::install(&s.l, &id).await?;
    Ok(Json(instance_json(&s.l, &i).await))
}

#[derive(Deserialize, Default)]
struct LaunchBody {
    account_id: Option<String>,
}

async fn launch_instance(State(s): State<AppState>, Path(id): Path<String>, body: Option<Json<LaunchBody>>) -> R<Value> {
    let b = body.map(|b| b.0).unwrap_or_default();
    launch::launch(&s.l, &id, b.account_id.as_deref()).await?;
    Ok(ok())
}

async fn stop_instance(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    let stopped = s.l.processes.stop(&launch::game_source(&id)).await;
    Ok(Json(json!({ "stopped": stopped })))
}

#[derive(Deserialize)]
struct LogQuery {
    /// `game` (latest.log), `launcher` (raw process output) or `crash`.
    #[serde(default = "default_log_kind")]
    kind: String,
    #[serde(default = "default_lines")]
    lines: usize,
}
fn default_log_kind() -> String {
    "game".into()
}
fn default_lines() -> usize {
    200
}

async fn instance_logs(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<LogQuery>) -> R<Value> {
    let i = instances::get(&s.l, &id).await?;
    let game = s.l.paths.game_dir(&i.id);
    let path = match q.kind.as_str() {
        "launcher" => s.l.paths.instance_dir(&i.id).join("launcher-output.log"),
        "crash" => {
            let dir = game.join("crash-reports");
            let newest = std::fs::read_dir(&dir)
                .ok()
                .and_then(|rd| {
                    rd.filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
                        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
                });
            match newest {
                Some(p) => p,
                None => return Ok(Json(json!({ "path": null, "text": "No crash reports." }))),
            }
        }
        _ => game.join("logs").join("latest.log"),
    };
    let text = process::tail_file(&path, q.lines.min(5000))
        .await
        .unwrap_or_else(|_| "No log yet.".into());
    Ok(Json(json!({ "path": path, "text": text })))
}

async fn list_mods(State(s): State<AppState>, Path(id): Path<String>) -> R<Vec<mods::ModEntry>> {
    instances::get(&s.l, &id).await?;
    Ok(Json(mods::list(&s.l, &id).await?))
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

async fn set_mod_enabled(State(s): State<AppState>, Path((id, file)): Path<(String, String)>, Json(b): Json<EnabledBody>) -> R<Value> {
    instances::get(&s.l, &id).await?;
    let f = mods::set_enabled(&s.l, &id, &file, b.enabled).await?;
    Ok(Json(json!({ "file_name": f })))
}

async fn delete_mod(State(s): State<AppState>, Path((id, file)): Path<(String, String)>) -> R<Value> {
    instances::get(&s.l, &id).await?;
    mods::delete(&s.l, &id, &file).await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct BrowseQuery {
    instance_id: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: browse::Sort,
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_page")]
    limit: u64,
}
fn default_page() -> u64 {
    20
}

async fn browse_search(State(s): State<AppState>, Path(source): Path<String>, Query(q): Query<BrowseQuery>) -> R<browse::SearchPage> {
    let source = browse::Source::parse(&source)?;
    let inst = instances::get(&s.l, &q.instance_id).await?;
    let mut page = browse::search(&s.l, source, &inst, &q.q, q.sort, q.offset, q.limit).await?;
    let tags = browse::prune_tags(&s.l, &inst.id).await;
    let installed: std::collections::HashSet<_> =
        tags.values().filter(|t| t.source == source).map(|t| t.project_id.clone()).collect();
    for h in &mut page.hits {
        h.installed = installed.contains(&h.project_id);
    }
    Ok(Json(page))
}

#[derive(Deserialize)]
struct VersionsQ {
    instance_id: String,
}

async fn browse_versions(State(s): State<AppState>, Path((source, pid)): Path<(String, String)>, Query(q): Query<VersionsQ>) -> R<Vec<browse::ModVersion>> {
    let source = browse::Source::parse(&source)?;
    let inst = instances::get(&s.l, &q.instance_id).await?;
    Ok(Json(browse::versions(&s.l, source, &pid, &inst).await?))
}

async fn install_online(State(s): State<AppState>, Path(id): Path<String>, Json(req): Json<browse::InstallRequest>) -> R<browse::InstallReport> {
    Ok(Json(browse::install(&s.l, &id, req).await?))
}

async fn list_projects(State(s): State<AppState>) -> R<Vec<projects::ModProject>> {
    Ok(Json(projects::list(&s.l).await?))
}

async fn create_project(State(s): State<AppState>, Json(req): Json<projects::NewProject>) -> R<projects::ModProject> {
    Ok(Json(projects::create(&s.l, req).await?))
}

async fn get_project(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    let p = projects::get(&s.l, &id).await?;
    let mut v = serde_json::to_value(&p)?;
    v["dev_running"] = json!(s.l.processes.is_running(&projects::dev_source(&id)).await);
    Ok(Json(v))
}

async fn build_project(State(s): State<AppState>, Path(id): Path<String>) -> R<projects::BuildResult> {
    Ok(Json(projects::build(&s.l, &id).await?))
}

#[derive(Deserialize, Default)]
struct InstallBody {
    instance_id: Option<String>,
}

async fn install_project(State(s): State<AppState>, Path(id): Path<String>, body: Option<Json<InstallBody>>) -> R<projects::BuildResult> {
    let b = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(projects::install(&s.l, &id, b.instance_id.as_deref()).await?))
}

async fn start_dev(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    projects::run_dev(&s.l, &id).await?;
    Ok(ok())
}

async fn stop_dev(State(s): State<AppState>, Path(id): Path<String>) -> R<Value> {
    let stopped = s.l.processes.stop(&projects::dev_source(&id)).await;
    Ok(Json(json!({ "stopped": stopped })))
}

#[derive(Deserialize)]
struct LinesQuery {
    #[serde(default = "default_lines")]
    lines: usize,
}

async fn dev_logs(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<LinesQuery>) -> R<Value> {
    let text = projects::dev_log(&s.l, &id, q.lines.min(5000)).await?;
    Ok(Json(json!({ "text": text })))
}

async fn events(State(s): State<AppState>) -> Sse<impl Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(s.l.events.subscribe()).filter_map(|e| {
        e.ok().map(|ev| Ok(SseEvent::default().data(serde_json::to_string(&ev).unwrap_or_default())))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    #[tokio::test]
    async fn token_required_and_instances_work() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let info = serve(l.clone()).await.unwrap();
        let base = format!("http://127.0.0.1:{}/api", info.port);
        let c = reqwest::Client::builder().no_proxy().build().unwrap();

        let r = c.get(format!("{base}/status")).send().await.unwrap();
        assert_eq!(r.status(), 401);

        let r: Value = c.get(format!("{base}/status")).bearer_auth(&info.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(r["signed_in"], false);

        // Vanilla instance creation needs no network.
        let r: Value = c
            .post(format!("{base}/instances"))
            .bearer_auth(&info.token)
            .json(&json!({ "name": "Test Pack", "mc_version": "1.21.1", "loader": "vanilla" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["id"], "test-pack");
        assert_eq!(r["installed"], false);

        let list: Vec<Value> = c.get(format!("{base}/instances")).bearer_auth(&info.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(list.len(), 1);

        let mods: Vec<Value> = c.get(format!("{base}/instances/test-pack/mods")).bearer_auth(&info.token).send().await.unwrap().json().await.unwrap();
        assert!(mods.is_empty());

        let r = c.get(format!("{base}/instances/..%2Fx")).bearer_auth(&info.token).send().await.unwrap();
        assert_eq!(r.status(), 400);
    }
}
