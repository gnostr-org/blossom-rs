//! Relay policy persistence — SQLite or PostgreSQL.
//!
//! Stores admin/whitelist/blacklist pubkeys and kind filters in a
//! `relay_policy` table.

use std::sync::{Arc, Mutex};

use super::policies::RelayPolicy;

/// Policy database — SQLite, PostgreSQL, or in-memory (for testing).
pub enum PolicyDb {
    Sqlite(sqlx::sqlite::SqlitePool),
    Postgres(sqlx::postgres::PgPool),
    Memory(Mutex<Vec<(String, String)>>),
}

impl PolicyDb {
    /// Open a SQLite policy database.
    pub async fn open_sqlite(path: &str) -> Result<Self, String> {
        let url = format!("sqlite:{path}?mode=rwc");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .map_err(|e| format!("policy db sqlite: {e}"))?;

        let db = Self::Sqlite(pool);
        db.migrate().await?;
        Ok(db)
    }

    /// Open a PostgreSQL policy database.
    pub async fn open_postgres(url: &str) -> Result<Self, String> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(url)
            .await
            .map_err(|e| format!("policy db postgres: {e}"))?;

        let db = Self::Postgres(pool);
        db.migrate().await?;
        Ok(db)
    }

    /// Create an in-memory policy database (for testing).
    pub fn memory() -> Self {
        Self::Memory(Mutex::new(Vec::new()))
    }

    async fn migrate(&self) -> Result<(), String> {
        match self {
            Self::Memory(_) => return Ok(()),
            Self::Sqlite(pool) => {
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS relay_policy (
                        category TEXT NOT NULL,
                        value    TEXT NOT NULL,
                        PRIMARY KEY (category, value)
                    )",
                )
                .execute(pool)
                .await
                .map_err(|e| format!("policy migration: {e}"))?;
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS relay_policy (
                        category TEXT NOT NULL,
                        value    TEXT NOT NULL,
                        PRIMARY KEY (category, value)
                    )",
                )
                .execute(pool)
                .await
                .map_err(|e| format!("policy migration: {e}"))?;
            }
        }
        Ok(())
    }

    /// Load all policy entries into a RelayPolicy.
    pub async fn load_into(&self, policy: &Arc<RelayPolicy>) -> Result<(), String> {
        let rows: Vec<(String, String)> = match self {
            Self::Sqlite(pool) => sqlx::query_as("SELECT category, value FROM relay_policy")
                .fetch_all(pool)
                .await
                .map_err(|e| format!("load policy: {e}"))?,
            Self::Postgres(pool) => sqlx::query_as("SELECT category, value FROM relay_policy")
                .fetch_all(pool)
                .await
                .map_err(|e| format!("load policy: {e}"))?,
            Self::Memory(data) => data.lock().unwrap().clone(),
        };

        let count = rows.len();
        for (category, value) in rows {
            match category.as_str() {
                "admin" => policy.add_admin(&value),
                "whitelist" => policy.add_whitelist(&value),
                "blacklist" => policy.add_blacklist(&value),
                "allowed_kind" => {
                    if let Ok(k) = value.parse::<u16>() {
                        policy.add_allowed_kind(nostr::Kind::Custom(k));
                    }
                }
                "disallowed_kind" => {
                    if let Ok(k) = value.parse::<u16>() {
                        policy.add_disallowed_kind(nostr::Kind::Custom(k));
                    }
                }
                _ => {}
            }
        }

        if count > 0 {
            tracing::info!(entries = count, "loaded relay policy from database");
        }

        Ok(())
    }

    /// Add a policy entry.
    pub async fn add(&self, category: &str, value: &str) -> Result<(), String> {
        match self {
            Self::Sqlite(pool) => {
                sqlx::query("INSERT OR IGNORE INTO relay_policy (category, value) VALUES (?, ?)")
                    .bind(category)
                    .bind(value)
                    .execute(pool)
                    .await
                    .map_err(|e| format!("add policy: {e}"))?;
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO relay_policy (category, value) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(category)
                .bind(value)
                .execute(pool)
                .await
                .map_err(|e| format!("add policy: {e}"))?;
            }
            Self::Memory(data) => {
                let mut d = data.lock().unwrap();
                let entry = (category.to_string(), value.to_string());
                if !d.contains(&entry) {
                    d.push(entry);
                }
            }
        }
        Ok(())
    }

    /// Remove a policy entry.
    pub async fn remove(&self, category: &str, value: &str) -> Result<(), String> {
        match self {
            Self::Sqlite(pool) => {
                sqlx::query("DELETE FROM relay_policy WHERE category = ? AND value = ?")
                    .bind(category)
                    .bind(value)
                    .execute(pool)
                    .await
                    .map_err(|e| format!("remove policy: {e}"))?;
            }
            Self::Postgres(pool) => {
                sqlx::query("DELETE FROM relay_policy WHERE category = $1 AND value = $2")
                    .bind(category)
                    .bind(value)
                    .execute(pool)
                    .await
                    .map_err(|e| format!("remove policy: {e}"))?;
            }
            Self::Memory(data) => {
                let mut d = data.lock().unwrap();
                d.retain(|(c, v)| c != category || v != value);
            }
        }
        Ok(())
    }
}
