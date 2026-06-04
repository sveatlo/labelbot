use crate::error::StoreError;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use chrono::Utc;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn connect(db_path: &str) -> Result<Self, StoreError> {
        let pool_opts = SqliteConnectOptions::from_str(db_path)?
            .create_if_missing(true);

        // Resolve filename via SqliteConnectOptions so `sqlite://...?...` URIs
        // don't produce a literal `sqlite:` directory.
        if let Some(parent) = pool_opts.get_filename().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(pool_opts)
            .await?;

        MIGRATOR.run(&pool).await?;

        Ok(Store { pool })
    }

    pub async fn is_processed(&self, message_id: &str) -> Result<bool, StoreError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM processed_messages WHERE message_id = ?")
                .bind(message_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.is_some())
    }

    pub async fn record(&self, message_id: &str, labels: &str) -> Result<(), StoreError> {
        let now = Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO processed_messages (message_id, labels, processed_at) VALUES (?, ?, ?)",
        )
        .bind(message_id)
        .bind(labels)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn count(&self) -> Result<i64, StoreError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM processed_messages")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct TestStore {
        store: Store,
        _dir: TempDir,
    }

    async fn test_store() -> TestStore {
        let dir = TempDir::new().unwrap();
        let path = format!(
            "sqlite:///{}?mode=rwc",
            dir.path().join("test.db").display()
        );
        let store = Store::connect(&path).await.unwrap();
        TestStore { store, _dir: dir }
    }

    #[tokio::test]
    async fn is_processed_false_for_new_id() {
        let ts = test_store().await;
        assert!(!ts.store.is_processed("msg-1").await.unwrap());
    }

    #[tokio::test]
    async fn record_and_then_is_processed_true() {
        let ts = test_store().await;
        ts.store.record("msg-1", "Work,Finance").await.unwrap();
        assert!(ts.store.is_processed("msg-1").await.unwrap());
    }

    #[tokio::test]
    async fn record_empty_labels_accepted() {
        let ts = test_store().await;
        ts.store.record("msg-2", "").await.unwrap();
        assert!(ts.store.is_processed("msg-2").await.unwrap());
    }

    #[tokio::test]
    async fn count_tracks_recorded_messages() {
        let ts = test_store().await;
        assert_eq!(ts.store.count().await.unwrap(), 0);
        ts.store.record("a", "X").await.unwrap();
        ts.store.record("b", "Y").await.unwrap();
        assert_eq!(ts.store.count().await.unwrap(), 2);
    }
}
