//! GRASP git HTTP smart protocol server.
//!
//! Serves git repositories over HTTP for clone/push operations.
//! Repositories are organized as `{npub}/{repo_name}.git` on the filesystem.

pub mod command;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;

use crate::Nip34State;

/// Build the git HTTP router.
///
/// Routes:
/// - `GET /:npub/:repo/info/refs` — advertise refs
/// - `POST /:npub/:repo/git-upload-pack` — fetch objects
/// - `POST /:npub/:repo/git-receive-pack` — push objects
/// - `GET /:npub/:repo/HEAD` — HEAD ref
/// - `GET /:npub/:repo/objects/*rest` — loose objects and packs
pub fn git_router() -> axum::Router<Arc<Nip34State>> {
    axum::Router::new()
        .route("/{npub}/{repo}/info/refs", axum::routing::get(info_refs))
        .route(
            "/{npub}/{repo}/git-upload-pack",
            axum::routing::post(upload_pack),
        )
        .route(
            "/{npub}/{repo}/git-receive-pack",
            axum::routing::post(receive_pack),
        )
}

/// GET /{npub}/{repo}/info/refs?service=git-upload-pack
async fn info_refs(
    State(state): State<Arc<Nip34State>>,
    Path((npub, repo)): Path<(String, String)>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let repo_name = repo.trim_end_matches(".git");
    let repo_path = match state.repo_path(&npub, repo_name) {
        Some(p) if p.join("HEAD").exists() => p,
        _ => return (StatusCode::NOT_FOUND, "repository not found").into_response(),
    };

    let service = params
        .get("service")
        .map(String::as_str)
        .unwrap_or("git-upload-pack");

    let git_cmd = command::GitCommand::new(&state.config.git_path, &repo_path);
    let is_v2 = false; // TODO: detect git protocol version from headers

    match git_cmd.refs(service, is_v2).await {
        Ok(body) => {
            let content_type = format!("application/x-{}-advertisement", service);
            // Pkt-line header required by git smart HTTP
            let pkt_header = format!("# service={}\n", service);
            let pkt_len = pkt_header.len() + 4;
            let pkt_line = format!("{:04x}{}", pkt_len, pkt_header);

            let mut output = pkt_line.into_bytes();
            output.extend_from_slice(b"0000");
            output.extend_from_slice(&body);

            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type)],
                output,
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// POST /{npub}/{repo}/git-upload-pack
async fn upload_pack(
    State(state): State<Arc<Nip34State>>,
    Path((npub, repo)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let repo_name = repo.trim_end_matches(".git");
    let repo_path = match state.repo_path(&npub, repo_name) {
        Some(p) if p.join("HEAD").exists() => p,
        _ => return (StatusCode::NOT_FOUND, "repository not found").into_response(),
    };

    let git_cmd = command::GitCommand::new(&state.config.git_path, &repo_path);

    match git_cmd.upload_pack(&body, false).await {
        Ok(output) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/x-git-upload-pack-result".to_string(),
            )],
            output,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// POST /{npub}/{repo}/git-receive-pack
async fn receive_pack(
    State(state): State<Arc<Nip34State>>,
    Path((npub, repo)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // TODO: Phase 2 — validate Nostr auth for push
    let repo_name = repo.trim_end_matches(".git");
    let repo_path = match state.repo_path(&npub, repo_name) {
        Some(p) if p.join("HEAD").exists() => p,
        _ => return (StatusCode::NOT_FOUND, "repository not found").into_response(),
    };

    let git_cmd = command::GitCommand::new(&state.config.git_path, &repo_path);

    match git_cmd.receive_pack(&body).await {
        Ok(output) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/x-git-receive-pack-result".to_string(),
            )],
            output,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}
