use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use ::time::OffsetDateTime;
use anyhow::{anyhow, Context, Result};
use http_cache_reqwest::{CACacheManager, Cache, HttpCache, MokaCache, MokaManager};
use rand::rngs::SmallRng;
use rand::{thread_rng, Rng, SeedableRng};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use tokio::time::Instant;
use tokio::{select, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, info_span, trace, Instrument};

use crate::extractor::Context as ExtractorContext;
use crate::state::Feed;
use crate::storage::Storage;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

pub struct Fetcher {
    feeds: Arc<HashMap<String, Feed>>,
    cache_dir: Option<PathBuf>,
    storage: Arc<Storage>,
    max_initial_sleep: Duration,
}

impl Fetcher {
    pub fn new(
        feeds: Arc<HashMap<String, Feed>>,
        cache_dir: Option<PathBuf>,
        storage: Arc<Storage>,
        max_initial_sleep: Duration,
    ) -> Self {
        Self {
            feeds,
            cache_dir,
            storage,
            max_initial_sleep,
        }
    }

    pub async fn run(self, cancel: CancellationToken) -> Result<()> {
        async move {
            let http_client = {
                let builder = ClientBuilder::new(
                    reqwest::Client::builder()
                        .connect_timeout(CONNECT_TIMEOUT)
                        .read_timeout(READ_TIMEOUT)
                        .timeout(TOTAL_TIMEOUT)
                        .build()
                        .context("could not create an HTTP client")?,
                );

                let builder = if let Some(path) = self.cache_dir {
                    debug!("Using a file cache at {}", path.display());
                    builder.with(Cache(HttpCache {
                        mode: Default::default(),
                        manager: CACacheManager { path },
                        options: Default::default(),
                    }))
                } else {
                    debug!("Using an in-memory cache");
                    builder.with(Cache(HttpCache {
                        mode: Default::default(),
                        manager: MokaManager::new(MokaCache::builder().max_capacity(8192).build()),
                        options: Default::default(),
                    }))
                };

                builder.build()
            };

            {
                let mut thread_rng = thread_rng();

                for (name, feed) in &*self.feeds {
                    if !feed.enabled {
                        info!("Skipping the feed `{name}`: disabled in the config");

                        continue;
                    }

                    let rng = SmallRng::from_rng(&mut thread_rng).unwrap();
                    let task = Task {
                        feeds: self.feeds.clone(),
                        storage: self.storage.clone(),
                        name: name.into(),
                        rng,
                        cancel: cancel.clone(),
                        http_client: http_client.clone(),
                        max_initial_sleep: self.max_initial_sleep,
                    };

                    tokio::spawn(task.run().instrument(info_span!("run", feed_name = %name)));
                }
            }

            cancel.cancelled_owned().await;

            Ok(())
        }
        .instrument(info_span!("fetcher"))
        .await
    }
}

struct Task {
    feeds: Arc<HashMap<String, Feed>>,
    storage: Arc<Storage>,
    name: String,
    rng: SmallRng,
    cancel: CancellationToken,
    http_client: ClientWithMiddleware,
    max_initial_sleep: Duration,
}

impl Task {
    async fn run(mut self) {
        let offset = self.rng.gen_range(Duration::ZERO..self.max_initial_sleep);

        let initial_sleep = if let Ok(Some(last_update)) = self.last_update().await {
            trace!(%last_update, "Found the last update time");
            let next_update = last_update + self.feed().fetch_interval;
            let remaining = (next_update - OffsetDateTime::now_utc()).max(::time::Duration::ZERO);

            (remaining + offset).try_into().unwrap_or(offset)
        } else {
            offset
        };

        debug!("Scheduling the next update in {}s", initial_sleep.as_secs());
        let mut next_fetch = pin!(time::sleep(initial_sleep));
        let force_update_notify = self.feed().force_update.clone().unwrap();
        let mut force_update = pin!(force_update_notify.notified());

        loop {
            select! {
                _ = self.cancel.cancelled() => {
                    debug!("Received a cancellation signal; exiting");
                    break;
                }

                _ = &mut force_update => {
                    force_update.set(force_update_notify.notified());

                    let deadline = next_fetch.deadline();
                    let now = Instant::now();
                    let preempted_by = deadline.saturating_duration_since(now).as_secs();
                    info!(
                        "Received a forced feed update request \
                            (preempted the next scheduled update by {preempted_by}s)"
                    );
                }

                _ = &mut next_fetch => {}
            }

            if let Err(e) = self.update().await {
                error!(
                    "Encountered a failure while updating the feed `{}`: {e:#}",
                    self.name
                );
            }

            let fetch_interval = self.feed().fetch_interval;
            debug!(
                "Scheduling the next update in {}s",
                fetch_interval.as_secs()
            );
            next_fetch
                .as_mut()
                .reset(Instant::now() + self.feed().fetch_interval);
        }
    }

    fn feed(&self) -> &Feed {
        &self.feeds[&self.name]
    }

    async fn last_update(&self) -> Result<Option<OffsetDateTime>> {
        let mut tx = self.storage.begin().await?;
        let last_update = tx.get_feed_last_updated(&self.name).await?;
        tx.commit().await?;

        Ok(last_update)
    }

    async fn update(&mut self) -> Result<()> {
        let url = self.feed().request_url.clone();

        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(Into::into)
            .and_then(|r| r.error_for_status().context("server returned an error"))
            .with_context(|| anyhow!("could not fetch `{}`", self.feed().request_url))?;
        let body = response.text().await.with_context(|| {
            anyhow!(
                "could not read the response when fetching `{}`",
                self.feed().request_url
            )
        })?;

        let entries = self
            .feed()
            .extractor
            .lock()
            .unwrap()
            .extract(ExtractorContext::new(&self.feed().request_url), &body)
            .context("could not extract feed entries")?;
        let count = entries.len();

        let mut tx = self.storage.begin().await?;
        tx.store_entries(&self.name, entries)
            .await
            .context("could not store entries to the DB")?;
        tx.commit().await?;

        info!("Retrieved {count} entries");

        Ok(())
    }
}
