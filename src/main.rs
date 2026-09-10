use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderName, Method};
use axum::middleware;
use axum::routing::{any, get, post};
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};

use steve_assist::handlers;
use steve_assist::services;
use steve_assist::state::AppState;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState::new().await);
    let port = state.server_port;

    // Spawn background scheduler for outbound call assignments
    let scheduler_state = state.clone();
    tokio::spawn(async move {
        services::scheduler::run(scheduler_state).await;
    });

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(
            "https://dashboard.steve.creativecaptains.com"
                .parse()
                .unwrap(),
        ))
        .allow_methods([Method::GET, Method::PUT, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
        ]);

    let api_routes = Router::new()
        .route("/api/sessions", get(handlers::api::list_sessions))
        .route("/api/profiles", get(handlers::api::list_profiles))
        .route(
            "/api/profiles/:profile_id",
            get(handlers::api::get_profile).put(handlers::api::update_profile),
        )
        .route(
            "/api/assignments",
            get(handlers::api::list_assignments).post(handlers::api::create_assignment),
        )
        .route(
            "/api/assignments/:id",
            get(handlers::api::get_assignment)
                .put(handlers::api::update_assignment)
                .delete(handlers::api::delete_assignment),
        )
        .route("/api/calendar/status", get(handlers::api::list_calendar_status))
        .route("/api/calendar/reauth", post(handlers::api::calendar_reauth_url))
        .route("/api/bot-groups", get(handlers::api::list_bot_groups))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            handlers::auth::require_auth,
        ));

    let app = Router::new()
        .route("/incoming-call", post(handlers::incoming_call::incoming_call))
        .route("/media-stream", any(handlers::media_stream::media_stream))
        .route("/outbound-call", post(handlers::outbound_call::outbound_call))
        .route("/outbound-call-status", post(handlers::outbound_call::outbound_call_status))
        .route("/auth/google/start", get(handlers::google_auth::start))
        .route("/auth/google/callback", get(handlers::google_auth::callback))
        .merge(api_routes)
        .with_state(state)
        .layer(cors);

    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind to port");

    tracing::info!("Server running on port {port}");
    axum::serve(listener, app).await.expect("Server error");
}
