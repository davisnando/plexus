//! IPC commands for the dashboard. Thin wrappers over `plexus_core::Hub`.

use plexus_core::{
    ApprovalDecision, ApprovalRequest,
    model::{
        CallLogEntry, Connection, ConnectionInput, ConnectionStatus, Profile, ProfileInput,
        Settings, ToolPolicy,
    },
    naming,
};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

#[derive(Serialize)]
pub struct ConnectionView {
    #[serde(flatten)]
    connection: Connection,
    status: ConnectionStatus,
    tool_count: usize,
    secrets_present: Vec<String>,
}

fn connection_view(state: &AppState, connection: Connection) -> CmdResult<ConnectionView> {
    Ok(ConnectionView {
        status: state.hub.status(&connection.id),
        tool_count: state.hub.tools(&connection.id).len(),
        secrets_present: state.hub.secrets_present(&connection).map_err(err)?,
        connection,
    })
}

#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> CmdResult<Vec<ConnectionView>> {
    state
        .hub
        .list_connections()
        .map_err(err)?
        .into_iter()
        .map(|c| connection_view(&state, c))
        .collect()
}

#[tauri::command]
pub async fn create_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> CmdResult<ConnectionView> {
    let c = state.hub.create_connection(input).await.map_err(err)?;
    connection_view(&state, c)
}

#[tauri::command]
pub async fn update_connection(
    state: State<'_, AppState>,
    id: String,
    input: ConnectionInput,
) -> CmdResult<ConnectionView> {
    let c = state.hub.update_connection(&id, input).await.map_err(err)?;
    connection_view(&state, c)
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.hub.delete_connection(&id).await.map_err(err)
}

#[tauri::command]
pub async fn test_connection(state: State<'_, AppState>, id: String) -> CmdResult<usize> {
    state.hub.test_connection(&id).await.map_err(err)
}

#[tauri::command]
pub async fn stop_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.hub.stop_upstream(&id).await;
    Ok(())
}

#[derive(Serialize)]
pub struct ToolView {
    name: String,
    exposed_name: String,
    title: Option<String>,
    description: Option<String>,
    read_only: Option<bool>,
    destructive: Option<bool>,
}

#[tauri::command]
pub fn connection_tools(state: State<'_, AppState>, id: String) -> CmdResult<Vec<ToolView>> {
    let connection = state.hub.connection(&id).map_err(err)?;
    Ok(state
        .hub
        .tools(&id)
        .into_iter()
        .map(|t| ToolView {
            exposed_name: naming::exposed_tool_name(&connection.alias, &t.name),
            name: t.name.to_string(),
            title: t.title.clone(),
            description: t.description.as_deref().map(String::from),
            read_only: t.annotations.as_ref().and_then(|a| a.read_only_hint),
            destructive: t.annotations.as_ref().and_then(|a| a.destructive_hint),
        })
        .collect())
}

#[derive(Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    profile: Profile,
    url: String,
}

fn profile_view(state: &AppState, profile: Profile) -> ProfileView {
    ProfileView {
        url: state.profile_url(&profile.slug),
        profile,
    }
}

#[tauri::command]
pub fn list_profiles(state: State<'_, AppState>) -> CmdResult<Vec<ProfileView>> {
    Ok(state
        .hub
        .list_profiles()
        .map_err(err)?
        .into_iter()
        .map(|p| profile_view(&state, p))
        .collect())
}

#[tauri::command]
pub fn create_profile(state: State<'_, AppState>, input: ProfileInput) -> CmdResult<ProfileView> {
    let p = state.hub.create_profile(input).map_err(err)?;
    Ok(profile_view(&state, p))
}

#[tauri::command]
pub fn update_profile(
    state: State<'_, AppState>,
    id: String,
    input: ProfileInput,
) -> CmdResult<ProfileView> {
    let p = state.hub.update_profile(&id, input).map_err(err)?;
    Ok(profile_view(&state, p))
}

#[tauri::command]
pub fn delete_profile(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.hub.delete_profile(&id).map_err(err)
}

#[tauri::command]
pub fn profile_token(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    state.hub.profile_token(&id).map_err(err)
}

#[tauri::command]
pub fn rotate_profile_token(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    state.hub.rotate_profile_token(&id).map_err(err)
}

#[derive(Serialize)]
pub struct ProfileToolView {
    connection_id: String,
    connection_alias: String,
    connection_name: String,
    tool: String,
    exposed_name: String,
    description: Option<String>,
    policy: ToolPolicy,
}

/// Every tool of every connection in the profile, including hidden ones.
#[tauri::command]
pub fn profile_tools(state: State<'_, AppState>, id: String) -> CmdResult<Vec<ProfileToolView>> {
    let profile = state.hub.profile(&id).map_err(err)?;
    let policies = state.hub.tool_policies(&id).map_err(err)?;
    let mut out = vec![];
    for c in state.hub.list_connections().map_err(err)? {
        if !profile.connection_ids.contains(&c.id) {
            continue;
        }
        for t in state.hub.tools(&c.id) {
            out.push(ProfileToolView {
                policy: policies
                    .get(&(c.id.clone(), t.name.to_string()))
                    .copied()
                    .unwrap_or_default(),
                exposed_name: naming::exposed_tool_name(&c.alias, &t.name),
                connection_id: c.id.clone(),
                connection_alias: c.alias.clone(),
                connection_name: c.name.clone(),
                tool: t.name.to_string(),
                description: t.description.as_deref().map(String::from),
            });
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn set_tool_policy(
    state: State<'_, AppState>,
    profile_id: String,
    connection_id: String,
    tool: String,
    policy: ToolPolicy,
) -> CmdResult<()> {
    state
        .hub
        .set_tool_policy(&profile_id, &connection_id, &tool, policy)
        .map_err(err)
}

#[tauri::command]
pub fn recent_calls(
    state: State<'_, AppState>,
    limit: Option<u32>,
    before_id: Option<i64>,
) -> CmdResult<Vec<CallLogEntry>> {
    state
        .hub
        .recent_calls(limit.unwrap_or(100), before_id)
        .map_err(err)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    state.hub.settings().map_err(err)
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> CmdResult<()> {
    let old = state.hub.settings().map_err(err)?;
    state.hub.save_settings(&settings).map_err(err)?;
    if old.port != settings.port {
        crate::restart_server(&app).await;
    }
    Ok(())
}

#[derive(Serialize, Clone)]
pub struct ServerStatus {
    pub running: bool,
    pub port: u16,
    pub error: Option<String>,
}

#[tauri::command]
pub fn server_status(state: State<'_, AppState>) -> ServerStatus {
    state.server_status()
}

#[tauri::command]
pub fn get_approval(state: State<'_, AppState>, id: String) -> Option<ApprovalRequest> {
    state.approvals.get(&id)
}

#[tauri::command]
pub fn resolve_approval(state: State<'_, AppState>, id: String, decision: ApprovalDecision) {
    state.approvals.resolve(&id, decision);
}

#[tauri::command]
pub async fn start_oauth(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let redirect = format!(
        "http://127.0.0.1:{}/oauth/callback",
        state.server_status().port
    );
    let url = state.hub.start_oauth(&id, &redirect).await.map_err(err)?;
    app.opener().open_url(url, None::<&str>).map_err(err)
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.hub.sign_out(&id).await.map_err(err)
}

#[tauri::command]
pub async fn install_client(
    state: State<'_, AppState>,
    profile_id: String,
    client: plexus_core::clients::Client,
) -> CmdResult<String> {
    let profile = state.hub.profile(&profile_id).map_err(err)?;
    let token = state.hub.profile_token(&profile_id).map_err(err)?;
    let url = state.profile_url(&profile.slug);
    let shell_path = state.hub.shell_path().map(String::from);
    tauri::async_runtime::spawn_blocking(move || {
        plexus_core::clients::install(
            client,
            &plexus_core::clients::Endpoint {
                slug: &profile.slug,
                url: &url,
                token: &token,
            },
            shell_path.as_deref(),
        )
    })
    .await
    .map_err(err)?
    .map_err(err)
}
