use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
    response::Html,
    routing::get,
};
use moth_core::data::structs::Data;
use rosu_v2::Osu;
use tower_http::cors::CorsLayer;

#[derive(serde::Deserialize)]
struct Params {
    state: u8,
    code: String,
}

pub async fn run(data: Arc<Data>) {
    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/", get(auth_osu))
        .with_state(data)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    axum::serve(listener, app).await.unwrap();
}

#[derive(serde::Serialize)]
struct IndexContext<'a> {
    success: bool,
    user: Option<&'a str>,
}

async fn auth_osu(
    query: Result<Query<Params>, QueryRejection>,
    State(state): State<Arc<Data>>,
) -> Result<(StatusCode, Html<String>), StatusCode> {
    if let Ok(page) = auth(query, &state).await {
        return Ok((StatusCode::OK, Html(page)));
    }

    let context = IndexContext {
        success: false,
        user: None,
    };

    let page = state
        .web
        .handlebars
        .render("index", &context)
        .expect("Failed to render template");

    Ok((StatusCode::OK, Html(page)))
}

async fn auth(query: Result<Query<Params>, QueryRejection>, data: &Data) -> Result<String, ()> {
    let Query(params) = query.map_err(|_| ())?;

    let osu = Osu::builder()
        .client_id(data.web.osu_client_id)
        .client_secret(data.web.osu_client_secret.expose_secret())
        .with_authorization(
            params.code,
            "https://verify.osucord.moe",
            rosu_v2::prelude::Scopes::Identify,
        )
        .build()
        .await
        .map_err(|_| ())?;

    let user = osu.own_data().await.map_err(|_| ())?;

    let context = IndexContext {
        success: true,
        user: Some(&user.username),
    };

    let page = data
        .web
        .handlebars
        .render("index", &context)
        .map_err(|_| ())?;

    Ok(page)
}
