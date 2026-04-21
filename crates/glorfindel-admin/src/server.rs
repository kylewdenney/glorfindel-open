use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    routing::{delete, get},
    Router,
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::api::{agents, campaign, definitions, tasks, tools, ws};
use crate::persist;
use crate::state::AppState;
use crate::zmq_bus;

static FRONTEND: &str = include_str!("admin.html");
static CAMPAIGN_FRONTEND: &str = include_str!("campaign.html");
static AGENTIFIER_FRONTEND: &str = include_str!("agentifier.html");

async fn serve_frontend() -> axum::response::Html<&'static str> {
    axum::response::Html(FRONTEND)
}

async fn serve_campaign_frontend() -> axum::response::Html<&'static str> {
    axum::response::Html(CAMPAIGN_FRONTEND)
}

async fn serve_agentifier_frontend() -> axum::response::Html<&'static str> {
    axum::response::Html(AGENTIFIER_FRONTEND)
}

pub async fn run(addr: &str, data_dir: PathBuf) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(data_dir.join("definitions")).await?;

    let state = AppState::new(data_dir.clone());
    *state.definitions.write().await = persist::load_definitions(&data_dir).await;

    let state = Arc::new(state);

    // Spawn ZMQ campaign event publisher (non-fatal if ZMQ unavailable)
    tokio::spawn(zmq_bus::run_publisher(state.task_events.subscribe()));

    let api = Router::new()
        .route("/definitions", get(definitions::list).post(definitions::create))
        .route(
            "/definitions/:id",
            get(definitions::get)
                .put(definitions::update)
                .delete(definitions::delete),
        )
        .route("/agents", get(agents::list).post(agents::spawn))
        .route("/agents/:id", delete(agents::stop))
        .route("/tasks", get(tasks::list).post(tasks::submit))
        .route("/tasks/:id", get(tasks::get))
        .route("/tools", get(tools::list))
        .route("/campaign", get(campaign::list_campaigns))
        .route("/campaign/:name/files", get(campaign::list_files))
        .route("/campaign/:name/file/*file_path", get(campaign::read_file).put(campaign::write_file))
        .route("/campaign/:name/run", axum::routing::post(campaign::run_task))
        .route("/campaign/:name/notes", axum::routing::post(campaign::append_note))
        .route("/campaign/:name/think", axum::routing::post(campaign::think_and_run))
        .route("/campaign/:name/session", axum::routing::post(campaign::session_turn))
        .route("/campaign/:name/session/:dir/summary", axum::routing::post(campaign::session_summary))
        .route("/campaign/:name/grand-opener", axum::routing::post(campaign::grand_opener))
        .route("/campaign/:name/eucatastrophe", axum::routing::post(campaign::eucatastrophe))
        .route("/campaign/:name/player-turn", axum::routing::post(campaign::player_turn))
        .route("/campaign/:name/session/:dir/open-scene", axum::routing::post(campaign::create_scene_opener))
        .route("/campaign/:name/session/:dir/scene/:scene/player-turn", axum::routing::post(campaign::scene_player_turn))
        .route("/campaign/:name/session/:dir/scene/:scene/summary", axum::routing::post(campaign::scene_summary_handler));

    let app = Router::new()
        .route("/", get(serve_frontend))
        .route("/campaign", get(serve_campaign_frontend))
        .route("/agentifier", get(serve_agentifier_frontend))
        .nest("/api", api)
        .route("/ws/tasks/:id", get(ws::task_stream))
        .route("/ws/pipeline", get(ws::pipeline_stream))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(addr = addr, data_dir = ?data_dir, "Glorfindel Admin listening");
    axum::serve(listener, app).await?;
    Ok(())
}
