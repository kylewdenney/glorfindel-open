use axum::Json;

use crate::tools::{catalog, ToolCatalogEntry};

pub async fn list() -> Json<Vec<ToolCatalogEntry>> {
    Json(catalog())
}
