pub mod entities;

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use reqwest::Url;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;
use tracing::{debug, error, info, instrument, trace_span, Instrument, Span};

use crate::extractor::Entry;

use self::entities::{Feed, FeedInfo};

pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();

        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(db_path)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Delete)
                    .create_if_missing(true),
            )
            .await
            .with_context(|| anyhow!("could not open a SQLite database `{}`", db_path.display()))?;
        info!("Using an SQLite database `{}`", db_path.display());
        sqlx::migrate!()
            .run(&pool)
            .await
            .with_context(|| anyhow!("could not prepare a database schema"))?;

        // TODO: delete feeds removed from the config.

        Ok(Self { pool })
    }

    pub async fn begin(&self) -> Result<Tx> {
        self.pool
            .begin()
            .await
            .context("could not begin a new DB transaction")
            .map(Tx)
    }
}

pub struct Tx(Transaction<'static, Sqlite>);

impl Tx {
    pub async fn commit(self) -> Result<()> {
        self.0
            .commit()
            .await
            .context("could not commit a DB transaction")
    }

    #[instrument(level = "TRACE", skip(self, entries), fields(entry_count = entries.len()))]
    pub async fn store_entries(&mut self, feed_name: &str, entries: Vec<Entry>) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        let feed_id: i64 = sqlx::query_scalar(
            "INSERT
            INTO feeds (name, last_updated)
            VALUES (?1, ?2)
            ON CONFLICT (name) DO UPDATE SET last_updated = excluded.last_updated
            RETURNING id",
        )
        .bind(feed_name)
        .bind(now)
        .fetch_one(self.0.as_mut())
        .await
        .context("could not retrieve the feed id")?;

        Span::current().record("feed_id", feed_id);

        for (idx, entry) in entries.into_iter().enumerate() {
            async {
                debug!(%entry.id, %entry.title, "Storing entry");
                sqlx::query(
                    "INSERT
                    INTO entries (
                      feed_id,
                      retrieved,
                      entry_id,
                      title,
                      description,
                      url,
                      author,
                      published
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT (feed_id, entry_id) DO UPDATE SET
                      title = excluded.title,
                      description = excluded.description,
                      url = excluded.url,
                      author = excluded.author,
                      published = excluded.published",
                )
                .bind(feed_id)
                .bind(now)
                .bind(entry.id)
                .bind(entry.title)
                .bind(entry.description)
                .bind(entry.url.to_string())
                .bind(entry.author)
                .bind(entry.pub_date)
                .execute(self.0.as_mut())
                .await
                .context("could not insert an entry")
            }
            .instrument(trace_span!("insert_entry", %idx))
            .await?;
        }

        Ok(())
    }

    #[instrument(level = "TRACE", skip(self))]
    pub async fn get_feed_last_updated(
        &mut self,
        feed_name: &str,
    ) -> Result<Option<OffsetDateTime>> {
        sqlx::query_scalar(
            "SELECT last_updated
            FROM feeds
            WHERE name = ?1",
        )
        .bind(feed_name)
        .fetch_optional(self.0.as_mut())
        .await
        .context("could not retrieve the last update date")
    }

    #[instrument(level = "TRACE", skip(self))]
    pub async fn get_feeds(&mut self) -> Result<Vec<FeedInfo>> {
        let feeds: Vec<Feed> = sqlx::query_as(
            "SELECT id, name, last_updated
            FROM feeds
            ORDER BY id ASC",
        )
        .fetch_all(self.0.as_mut())
        .await
        .context("could not retrieve the feed list")?;

        let feed_counts: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT feeds.id AS id, COUNT(*) AS entry_count
            FROM feeds
              LEFT JOIN entries ON (feeds.id = entries.feed_id)
            GROUP BY feeds.id
            ORDER BY feeds.id ASC",
        )
        .fetch_all(self.0.as_mut())
        .await
        .context("could not retrieve entry counts")?;

        let mut feed_counts = feed_counts.into_iter().peekable();
        let mut result = Vec::with_capacity(feeds.len());

        for feed in feeds {
            let entry_count = loop {
                if feed_counts
                    .peek()
                    .filter(|(feed_id, _)| feed.id >= *feed_id)
                    .is_none()
                {
                    break 0;
                }

                let (feed_id, count) = feed_counts.next().unwrap();

                if feed_id == feed.id {
                    break count as usize;
                } else {
                    continue;
                }
            };

            result.push(FeedInfo {
                name: feed.name,
                last_updated: feed.last_updated,
                entry_count,
            });
        }

        Ok(result)
    }

    #[instrument(level = "TRACE", skip(self))]
    pub async fn get_feed_entries(&mut self, feed_name: &str, count: usize) -> Result<Vec<Entry>> {
        let feed_id: Option<i64> = sqlx::query_scalar(
            "SELECT id
            FROM feeds
            WHERE name = ?1",
        )
        .bind(feed_name)
        .fetch_optional(self.0.as_mut())
        .await
        .context("could not retrieve the feed id")?;
        let Some(feed_id) = feed_id else {
            return Ok(vec![]);
        };

        let entries: Vec<entities::Entry> = sqlx::query_as(
            "SELECT
              retrieved,
              entry_id,
              title,
              description,
              url,
              author,
              published
            FROM entries
            WHERE feed_id = ?1
            ORDER BY retrieved DESC
            LIMIT ?2",
        )
        .bind(feed_id)
        .bind(count as i64)
        .fetch_all(self.0.as_mut())
        .await
        .context("could not retrieve feed entries")?;

        let mut result = Vec::with_capacity(entries.len());

        for entry in entries {
            let url = match Url::parse(&entry.url) {
                Ok(url) => url,

                Err(e) => {
                    error!(
                        %feed_name, entry_id = %entry.entry_id,
                        "The value of the column `url` is malformed: {e:#}",
                    );
                    continue;
                }
            };

            result.push(Entry {
                id: entry.entry_id,
                title: entry.title,
                description: entry.description,
                url,
                author: entry.author,
                pub_date: Some(entry.published.unwrap_or(entry.retrieved)),
            });
        }

        Ok(result)
    }
}
