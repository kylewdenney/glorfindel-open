use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use reqwest::Client;
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

// ---------------------------------------------------------------------------
// Shared Jellyfin client state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct JellyfinClient {
    pub base_url: String,
    pub api_key: String,
    pub user_id: String,
    client: Client,
}

impl JellyfinClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        user_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            user_id: user_id.into(),
            client: Client::new(),
        }
    }

    fn auth_header(&self) -> String {
        format!(
            r#"MediaBrowser Client="Glorfindel", Device="glorfindel-agent", DeviceId="glorfindel", Version="0.1", Token="{}""#,
            self.api_key
        )
    }

    pub async fn post_empty(&self, path: &str) -> Result<(), ToolError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Length", "0")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ToolError::ExecutionFailed(format!("Jellyfin returned {}", resp.status())));
        }
        Ok(())
    }

    pub async fn get(&self, path: &str) -> Result<serde_json::Value, ToolError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Jellyfin returned {}",
                resp.status()
            )));
        }

        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// media.search
// ---------------------------------------------------------------------------

/// Search the Jellyfin library for movies, shows, or episodes by title.
pub struct MediaSearchTool {
    jellyfin: JellyfinClient,
}

impl MediaSearchTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaSearchTool {
    fn name(&self) -> &str {
        "media.search"
    }

    fn description(&self) -> &str {
        "Search the media library by title. Parameters: \
         'query' (string, required), \
         'limit' (number, optional, default 10), \
         'type' (string, optional: 'Movie', 'Series', 'Episode', default all). \
         Returns matching items with name, year, type, and overview."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let query = parameters
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("query".into()))?;

        let limit = parameters
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10);

        let item_types = parameters
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut path = format!(
            "/Users/{}/Items?SearchTerm={}&Limit={}&Recursive=true\
             &Fields=Overview,ProductionYear,RunTimeTicks\
             &SortBy=SortName&SortOrder=Ascending",
            self.jellyfin.user_id,
            urlencoding::encode(query),
            limit
        );

        if !item_types.is_empty() {
            path.push_str(&format!("&IncludeItemTypes={item_types}"));
        }

        let data = self.jellyfin.get(&path).await?;

        let items: Vec<serde_json::Value> = data
            .get("Items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.get("Id").and_then(|v| v.as_str()).unwrap_or(""),
                    "name": item.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                    "type": item.get("Type").and_then(|v| v.as_str()).unwrap_or(""),
                    "year": item.get("ProductionYear"),
                    "overview": item.get("Overview").and_then(|v| v.as_str()).unwrap_or(""),
                    "runtime_minutes": item.get("RunTimeTicks")
                        .and_then(|v| v.as_i64())
                        .map(|t| t / 600_000_000),
                })
            })
            .collect();

        let total = data.get("TotalRecordCount").and_then(|v| v.as_u64()).unwrap_or(0);

        Ok(ToolResult::success(
            task_id,
            "media.search",
            serde_json::json!({ "total": total, "results": items }),
        ))
    }
}

// ---------------------------------------------------------------------------
// media.library
// ---------------------------------------------------------------------------

/// Get library statistics and virtual folder names.
pub struct MediaLibraryTool {
    jellyfin: JellyfinClient,
}

impl MediaLibraryTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaLibraryTool {
    fn name(&self) -> &str {
        "media.library"
    }

    fn description(&self) -> &str {
        "Get an overview of the media library: virtual folder names and item counts \
         (movies, series, episodes). No parameters required."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, _parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let folders = self.jellyfin.get("/Library/VirtualFolders").await?;
        let counts = self.jellyfin.get("/Items/Counts").await?;

        let libraries: Vec<serde_json::Value> = folders
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                    "type": f.get("CollectionType").and_then(|v| v.as_str()).unwrap_or("mixed"),
                })
            })
            .collect();

        Ok(ToolResult::success(
            task_id,
            "media.library",
            serde_json::json!({
                "libraries": libraries,
                "counts": {
                    "movies": counts.get("MovieCount"),
                    "series": counts.get("SeriesCount"),
                    "episodes": counts.get("EpisodeCount"),
                    "songs": counts.get("SongCount"),
                    "books": counts.get("BookCount"),
                }
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// media.recent
// ---------------------------------------------------------------------------

/// Get recently added items from the library.
pub struct MediaRecentTool {
    jellyfin: JellyfinClient,
}

impl MediaRecentTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaRecentTool {
    fn name(&self) -> &str {
        "media.recent"
    }

    fn description(&self) -> &str {
        "Get recently added media items. Parameters: \
         'limit' (number, optional, default 10), \
         'type' (string, optional: 'Movie', 'Series', 'Episode'). \
         Returns name, type, year, and date added."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let limit = parameters
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10);

        let item_types = parameters
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut path = format!(
            "/Users/{}/Items/Latest?Limit={}&Fields=Overview,ProductionYear,DateCreated",
            self.jellyfin.user_id, limit
        );

        if !item_types.is_empty() {
            path.push_str(&format!("&IncludeItemTypes={item_types}"));
        }

        let data = self.jellyfin.get(&path).await?;

        let items: Vec<serde_json::Value> = data
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                    "type": item.get("Type").and_then(|v| v.as_str()).unwrap_or(""),
                    "year": item.get("ProductionYear"),
                    "added": item.get("DateCreated").and_then(|v| v.as_str()).unwrap_or(""),
                    "overview": item.get("Overview").and_then(|v| v.as_str()).unwrap_or(""),
                })
            })
            .collect();

        Ok(ToolResult::success(
            task_id,
            "media.recent",
            serde_json::json!({ "items": items }),
        ))
    }
}

// ---------------------------------------------------------------------------
// media.sessions
// ---------------------------------------------------------------------------

/// Get currently active playback sessions.
pub struct MediaSessionsTool {
    jellyfin: JellyfinClient,
}

impl MediaSessionsTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaSessionsTool {
    fn name(&self) -> &str {
        "media.sessions"
    }

    fn description(&self) -> &str {
        "Get currently active playback sessions. No parameters required. \
         Returns who is watching what, playback position, and client info."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, _parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let data = self.jellyfin.get("/Sessions?ActiveWithinSeconds=300").await?;

        let sessions: Vec<serde_json::Value> = data
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                // Only include sessions with active playback
                let now_playing = s.get("NowPlayingItem")?;
                Some(serde_json::json!({
                    "user": s.get("UserName").and_then(|v| v.as_str()).unwrap_or("Unknown"),
                    "client": s.get("Client").and_then(|v| v.as_str()).unwrap_or(""),
                    "device": s.get("DeviceName").and_then(|v| v.as_str()).unwrap_or(""),
                    "playing": now_playing.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                    "type": now_playing.get("Type").and_then(|v| v.as_str()).unwrap_or(""),
                    "position_minutes": s.get("PlayState")
                        .and_then(|ps| ps.get("PositionTicks"))
                        .and_then(|v| v.as_i64())
                        .map(|t| t / 600_000_000),
                    "paused": s.get("PlayState")
                        .and_then(|ps| ps.get("IsPaused"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }))
            })
            .collect();

        Ok(ToolResult::success(
            task_id,
            "media.sessions",
            serde_json::json!({ "active_sessions": sessions }),
        ))
    }
}

// ---------------------------------------------------------------------------
// media.scan
// ---------------------------------------------------------------------------

/// Trigger a library scan on all (or a specific) library.
pub struct MediaScanTool {
    jellyfin: JellyfinClient,
}

impl MediaScanTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaScanTool {
    fn name(&self) -> &str { "media.scan" }

    fn description(&self) -> &str {
        "Trigger a library scan to pick up new or changed files. No parameters required. \
         Jellyfin will scan all libraries in the background."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.write".into())]
    }

    async fn execute(&self, task_id: Uuid, _parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        self.jellyfin.post_empty("/Library/Refresh").await?;
        Ok(ToolResult::success(task_id, "media.scan",
            serde_json::json!({ "status": "scan started" })))
    }
}

// ---------------------------------------------------------------------------
// media.refresh
// ---------------------------------------------------------------------------

/// Refresh metadata for a specific item by ID.
pub struct MediaRefreshTool {
    jellyfin: JellyfinClient,
}

impl MediaRefreshTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaRefreshTool {
    fn name(&self) -> &str { "media.refresh" }

    fn description(&self) -> &str {
        "Refresh metadata for a specific media item. Parameter: 'item_id' (string, required). \
         Get item_id from media.search results."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.write".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let item_id = parameters
            .get("item_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("item_id".into()))?;

        self.jellyfin.post_empty(&format!("/Items/{item_id}/Refresh?ReplaceAllMetadata=false")).await?;
        Ok(ToolResult::success(task_id, "media.refresh",
            serde_json::json!({ "item_id": item_id, "status": "refresh started" })))
    }
}

// ---------------------------------------------------------------------------
// media.item
// ---------------------------------------------------------------------------

/// Get detailed info about a single item by ID.
pub struct MediaItemTool {
    jellyfin: JellyfinClient,
}

impl MediaItemTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaItemTool {
    fn name(&self) -> &str { "media.item" }

    fn description(&self) -> &str {
        "Get detailed info about a media item by ID. Parameter: 'item_id' (string, required). \
         Returns full metadata including genres, studios, cast, ratings, and file info."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let item_id = parameters
            .get("item_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("item_id".into()))?;

        let data = self.jellyfin.get(&format!(
            "/Users/{}/Items/{item_id}?Fields=Genres,Studios,People,MediaSources,Overview,CriticRating,CommunityRating",
            self.jellyfin.user_id
        )).await?;

        let detail = serde_json::json!({
            "id": data.get("Id").and_then(|v| v.as_str()).unwrap_or(""),
            "name": data.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
            "type": data.get("Type").and_then(|v| v.as_str()).unwrap_or(""),
            "year": data.get("ProductionYear"),
            "overview": data.get("Overview").and_then(|v| v.as_str()).unwrap_or(""),
            "genres": data.get("Genres"),
            "studios": data.get("Studios").and_then(|v| v.as_array()).map(|arr|
                arr.iter().filter_map(|s| s.get("Name").and_then(|n| n.as_str())).collect::<Vec<_>>()
            ),
            "community_rating": data.get("CommunityRating"),
            "critic_rating": data.get("CriticRating"),
            "runtime_minutes": data.get("RunTimeTicks")
                .and_then(|v| v.as_i64())
                .map(|t| t / 600_000_000),
            "path": data.get("MediaSources")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|s| s.get("Path"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        });

        Ok(ToolResult::success(task_id, "media.item", detail))
    }
}

// ---------------------------------------------------------------------------
// media.users
// ---------------------------------------------------------------------------

/// List Jellyfin users.
pub struct MediaUsersTool {
    jellyfin: JellyfinClient,
}

impl MediaUsersTool {
    pub fn new(jellyfin: JellyfinClient) -> Self {
        Self { jellyfin }
    }
}

#[async_trait]
impl Tool for MediaUsersTool {
    fn name(&self) -> &str { "media.users" }

    fn description(&self) -> &str {
        "List all Jellyfin users. No parameters required. \
         Returns username, last activity, and whether the account is disabled."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("media.read".into())]
    }

    async fn execute(&self, task_id: Uuid, _parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let data = self.jellyfin.get("/Users").await?;

        let users: Vec<serde_json::Value> = data
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|u| serde_json::json!({
                "id": u.get("Id").and_then(|v| v.as_str()).unwrap_or(""),
                "name": u.get("Name").and_then(|v| v.as_str()).unwrap_or(""),
                "last_login": u.get("LastLoginDate").and_then(|v| v.as_str()).unwrap_or(""),
                "last_activity": u.get("LastActivityDate").and_then(|v| v.as_str()).unwrap_or(""),
                "is_admin": u.get("Policy")
                    .and_then(|p| p.get("IsAdministrator"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                "is_disabled": u.get("Policy")
                    .and_then(|p| p.get("IsDisabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            }))
            .collect();

        Ok(ToolResult::success(task_id, "media.users",
            serde_json::json!({ "users": users })))
    }
}
