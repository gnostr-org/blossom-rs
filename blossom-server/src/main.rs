//! blossom-server — Example Blossom API server.
//!
//! Showcases all blossom-rs features: filesystem storage, SQLite metadata,
//! NIP-96, access control, auth enforcement, and structured tracing.

use std::path::PathBuf;

use blossom_rs::access::Whitelist;
use blossom_rs::db::MemoryDatabase;
use blossom_rs::server::nip96::nip96_router;
use blossom_rs::{BlobServer, FilesystemBackend, MemoryBackend};
use clap::Parser;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "blossom-server", about = "Blossom blob storage API server")]
struct Args {
    /// Listen address.
    #[arg(short, long, default_value = "0.0.0.0:3000")]
    bind: String,

    /// Public base URL for blob URLs in responses.
    #[arg(short = 'u', long, default_value = "http://localhost:3000")]
    base_url: String,

    /// Blob storage directory (ignored with --memory).
    #[arg(short, long, default_value = "./blobs")]
    data_dir: String,

    /// Use in-memory storage instead of filesystem (no persistence).
    #[arg(long)]
    memory: bool,

    /// SQLite database path for metadata (ignored with --memory).
    #[arg(long, default_value = "./blossom.db")]
    db_path: String,

    /// Require BIP-340 Nostr auth for uploads.
    #[arg(long)]
    require_auth: bool,

    /// Maximum upload size in bytes.
    #[arg(long)]
    max_upload_size: Option<u64>,

    /// Path to pubkey whitelist file (one hex pubkey per line).
    #[arg(long)]
    whitelist: Option<PathBuf>,

    /// Log level.
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Init structured tracing (JSON to stdout, OTEL-compatible field names).
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_target(true)
                .with_span_list(true),
        )
        .init();

    // Build the server with the configured backend and database.
    let mut builder = if args.memory {
        info!("using in-memory storage (no persistence)");
        BlobServer::builder(MemoryBackend::new(), &args.base_url).database(MemoryDatabase::new())
    } else {
        let backend = FilesystemBackend::new(&args.data_dir)?;
        info!(data_dir = %args.data_dir, "using filesystem storage");

        let db_url = format!("sqlite:{}?mode=rwc", args.db_path);
        let db = blossom_rs::db::SqliteDatabase::new(&db_url).await?;
        info!(db_path = %args.db_path, "using SQLite metadata database");

        BlobServer::builder(backend, &args.base_url).database(db)
    };

    if args.require_auth {
        builder = builder.require_auth();
        info!("auth required for uploads");
    }

    if let Some(max) = args.max_upload_size {
        builder = builder.max_upload_size(max);
        info!(max_bytes = max, "upload size limit set");
    }

    if let Some(ref wl_path) = args.whitelist {
        let whitelist = Whitelist::from_file(wl_path)?;
        info!(path = %wl_path.display(), "loaded pubkey whitelist");
        builder = builder.access_control(whitelist);
    }

    let server = builder.build();

    // Merge NIP-96 endpoints into the main router.
    let state = server.shared_state();
    let app = server.router().merge(nip96_router(state));

    info!(bind = %args.bind, base_url = %args.base_url, "starting blossom server");

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
