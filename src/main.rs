mod auth;
mod db;
mod handlers;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use tokio::fs;
use tower_http::services::ServeDir;

use axum::http::{header, Method};
use tower_http::cors::{Any, CorsLayer};
use dotenvy;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub http_client: reqwest::Client,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    fs::create_dir_all("uploads").await.unwrap();

    let pool = db::init_db().await;
    let http_client = reqwest::Client::new();
    let state = AppState { db: pool, http_client, };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT]);

    let app = Router::new()
        .route("/upload", post(handlers::upload_image))
        .route("/register", post(auth::register_user))
        .route("/login", post(auth::login_user))
        .route("/all_images", get(handlers::list_all_images))
        .route("/my_images", get(handlers::list_user_images))
        .route("/admin/users", get(handlers::list_pending_users))
        .route("/admin/approve/:user_id", post(handlers::approve_user))
        .route("/logout", post(handlers::logout_user))
        .route("/send", get(handlers::send_image))
        .route("/delete/:image_id", post(handlers::delete_image))
        .route("/api/discord_webhook", post(handlers::send_to_discord))
        .route("/favorites", get(handlers::list_favorite_images))
        .route("/favorite/:image_id", post(handlers::toggle_favorite))
        .route("/tags", get(handlers::list_tags))
        .route("/api/images/:image_id/tags", post(handlers::add_tag))
        .route("/api/images/:image_id/tags/:tag_name", axum::routing::delete(handlers::remove_tag))
        .nest_service("/", ServeDir::new("public"))
        .nest_service("/images", ServeDir::new("uploads"))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Server running on http://{addr}"); // Let cloudflare or similar figure https out

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}