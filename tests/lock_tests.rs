//! Integration tests for LFS lock endpoints (BUD-19).
//!
//! Tests the full HTTP lock API against an in-memory Blossom server
//! with a MemoryLockDatabase.

use blossom_rs::auth::{auth_header_value, build_blossom_auth, Signer};
use blossom_rs::locks::MemoryLockDatabase;
use blossom_rs::server::BlobServer;
use blossom_rs::storage::MemoryBackend;
use blossom_rs::BlossomSigner;

fn lock_server() -> BlobServer {
    BlobServer::builder(MemoryBackend::new(), "http://localhost:3000")
        .lock_database(MemoryLockDatabase::new())
        .build()
}

fn no_lock_server() -> BlobServer {
    BlobServer::new(MemoryBackend::new(), "http://localhost:3000")
}

fn lock_auth(signer: &Signer) -> String {
    let event = build_blossom_auth(signer, "lock", None, None, "");
    auth_header_value(&event)
}

async fn spawn_server(server: BlobServer) -> String {
    let app = server.router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    url
}

#[tokio::test]
async fn test_create_lock() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let resp = reqwest::Client::new()
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", auth)
        .json(&serde_json::json!({"path": "assets/big-file.bin"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let lock = &body["lock"];
    assert_eq!(lock["path"], "assets/big-file.bin");
    assert_eq!(lock["owner"]["name"], signer.public_key_hex());
    assert!(!lock["id"].as_str().unwrap().is_empty());
    assert!(!lock["locked_at"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_create_lock_conflict() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let client = reqwest::Client::new();
    let path = format!("{}/lfs/myrepo/locks", url);

    let resp1 = client
        .post(&path)
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 201);

    let resp2 = client
        .post(&path)
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 409);
}

#[tokio::test]
async fn test_create_lock_requires_auth() {
    let server = lock_server();
    let url = spawn_server(server).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/lfs/myrepo/locks", url))
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_create_lock_different_repos_same_path() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("{}/lfs/repo1/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 201);

    let resp2 = client
        .post(format!("{}/lfs/repo2/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 201);
}

#[tokio::test]
async fn test_unlock_by_owner() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let client = reqwest::Client::new();

    let create_resp: serde_json::Value = client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let lock_id = create_resp["lock"]["id"].as_str().unwrap();

    let unlock_resp = client
        .post(format!("{}/lfs/myrepo/locks/{}/unlock", url, lock_id))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"force": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(unlock_resp.status(), 200);
    let body: serde_json::Value = unlock_resp.json().await.unwrap();
    assert_eq!(body["lock"]["id"], lock_id);
}

#[tokio::test]
async fn test_unlock_by_non_owner_forbidden() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let owner = Signer::generate();
    let other = Signer::generate();

    let client = reqwest::Client::new();

    let create_resp: serde_json::Value = client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", lock_auth(&owner))
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let lock_id = create_resp["lock"]["id"].as_str().unwrap();

    let unlock_resp = client
        .post(format!("{}/lfs/myrepo/locks/{}/unlock", url, lock_id))
        .header("Authorization", lock_auth(&other))
        .json(&serde_json::json!({"force": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(unlock_resp.status(), 403);
}

#[tokio::test]
async fn test_unlock_not_found() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let resp = reqwest::Client::new()
        .post(format!("{}/lfs/myrepo/locks/nonexistent-id/unlock", url))
        .header("Authorization", auth)
        .json(&serde_json::json!({"force": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_list_locks() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);
    let client = reqwest::Client::new();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "a.txt"}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "b.txt"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let locks = body["locks"].as_array().unwrap();
    assert_eq!(locks.len(), 2);
}

#[tokio::test]
async fn test_list_locks_with_path_filter() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);
    let client = reqwest::Client::new();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "a.txt"}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "b.txt"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{}/lfs/myrepo/locks?path=a.txt", url))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let locks = body["locks"].as_array().unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0]["path"], "a.txt");
}

#[tokio::test]
async fn test_verify_locks_splits_ours_and_theirs() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let owner = Signer::generate();
    let other = Signer::generate();

    let client = reqwest::Client::new();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", lock_auth(&owner))
        .json(&serde_json::json!({"path": "owner-file.txt"}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", lock_auth(&other))
        .json(&serde_json::json!({"path": "other-file.txt"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(format!("{}/lfs/myrepo/locks/verify", url))
        .header("Authorization", lock_auth(&owner))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    let ours = body["ours"].as_array().unwrap();
    let theirs = body["theirs"].as_array().unwrap();

    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0]["path"], "owner-file.txt");
    assert_eq!(ours[0]["owner"]["name"], owner.public_key_hex());

    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0]["path"], "other-file.txt");
    assert_eq!(theirs[0]["owner"]["name"], other.public_key_hex());
}

#[tokio::test]
async fn test_lock_endpoints_return_404_without_lock_db() {
    let server = no_lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let client = reqwest::Client::new();

    let create_resp = client
        .post(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "file.txt"}))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 404);

    let list_resp = client
        .get(format!("{}/lfs/myrepo/locks", url))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 404);

    let verify_resp = client
        .post(format!("{}/lfs/myrepo/locks/verify", url))
        .header("Authorization", &auth)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 404);
}

#[tokio::test]
async fn test_list_locks_empty_repo() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);

    let resp = reqwest::Client::new()
        .get(format!("{}/lfs/empty-repo/locks", url))
        .header("Authorization", auth)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["locks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_lock_lifecycle() {
    let server = lock_server();
    let url = spawn_server(server).await;
    let signer = Signer::generate();
    let auth = lock_auth(&signer);
    let client = reqwest::Client::new();
    let repo = "lifecycle-test";

    let create_resp = client
        .post(format!("{}/lfs/{}/locks", url, repo))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"path": "big-file.bin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 201);
    let create_body: serde_json::Value = create_resp.json().await.unwrap();
    let lock_id = create_body["lock"]["id"].as_str().unwrap().to_string();

    let verify_resp = client
        .post(format!("{}/lfs/{}/locks/verify", url, repo))
        .header("Authorization", &auth)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 200);
    let verify_body: serde_json::Value = verify_resp.json().await.unwrap();
    assert_eq!(verify_body["ours"].as_array().unwrap().len(), 1);
    assert_eq!(verify_body["theirs"].as_array().unwrap().len(), 0);

    let list_resp = client
        .get(format!("{}/lfs/{}/locks", url, repo))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_body: serde_json::Value = list_resp.json().await.unwrap();
    assert_eq!(list_body["locks"].as_array().unwrap().len(), 1);

    let unlock_resp = client
        .post(format!("{}/lfs/{}/locks/{}/unlock", url, repo, lock_id))
        .header("Authorization", &auth)
        .json(&serde_json::json!({"force": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(unlock_resp.status(), 200);

    let list_after_resp = client
        .get(format!("{}/lfs/{}/locks", url, repo))
        .header("Authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(list_after_resp.status(), 200);
    let list_after_body: serde_json::Value = list_after_resp.json().await.unwrap();
    assert!(list_after_body["locks"].as_array().unwrap().is_empty());
}
