use std::cmp::Reverse;
use std::collections::HashMap;
use std::mem;

use anyhow::{anyhow, Context};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Result};
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder};
use serde::Serialize;
use time::format_description::well_known::Rfc2822;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::OffsetDateTime;
use tracing::error;

use crate::server::convert_errors;
use crate::state::State as AppState;
use crate::template::Template;

use super::responses::FeedCannotBeUpdated;

const MAX_FEED_ENTRY_COUNT: usize = 100;

pub async fn index(State(state): State<AppState>) -> Result<Html<String>> {
    static DATE_FORMAT: &[BorrowedFormatItem<'_>] = format_description!(
        "[year]-[month]-[day] \
            [hour]:[minute]:[second].[subsecond digits:3] \
            [offset_hour sign:mandatory]:[offset_minute]"
    );

    #[derive(Serialize, Debug, Clone)]
    struct FeedDescription {
        name: String,
        last_updated: String,
        entry_count: usize,
        rss_url: String,
        fetch_url: String,
    }

    #[derive(Serialize, Debug, Clone)]
    struct Context {
        feeds: Vec<FeedDescription>,
    }

    convert_errors(async move {
        let mut tx = state.storage.begin().await?;
        let stored_feeds = tx.get_feeds().await?;
        tx.commit().await?;

        let stored_feeds = stored_feeds
            .into_iter()
            .map(|mut feed| (mem::take(&mut feed.name), feed))
            .collect::<HashMap<_, _>>();

        let mut feeds = Vec::with_capacity(state.feeds.len());

        for (name, feed) in &*state.feeds {
            let feed_info = stored_feeds.get(name);

            let last_updated = if let Some(feed_info) = feed_info {
                let last_updated = feed_info.last_updated;

                last_updated
                    .format(DATE_FORMAT)
                    .with_context(|| anyhow!("could not format the date {last_updated}"))?
            } else {
                "never".into()
            };

            let entry_count = feed_info
                .map(|feed_info| feed_info.entry_count)
                .unwrap_or(0);
            let rss_url = format!("/feeds/{}", urlencoding::encode(name));

            feeds.push(FeedDescription {
                name: name.into(),
                last_updated,
                entry_count,
                rss_url,
                fetch_url: feed.request_url.to_string(),
            });
        }

        feeds.sort_unstable_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
        let ctx = Context { feeds };
        let html = state
            .template
            .render(Template::Index.as_str(), &ctx)
            .context("could not render the HTML template")?;

        Ok(Html(html))
    })
    .await
}

pub async fn get_feed(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let feed = state.feeds.get(&name).ok_or(StatusCode::NOT_FOUND)?;

    let mut entries = convert_errors(async {
        let mut tx = state.storage.begin().await?;
        let entries = tx.get_feed_entries(&name, MAX_FEED_ENTRY_COUNT).await?;
        tx.commit().await?;

        Ok(entries)
    })
    .await?;
    entries.sort_by_key(|entry| Reverse(entry.pub_date.unwrap()));

    let now = OffsetDateTime::now_utc();
    let mut channel = ChannelBuilder::default();
    channel
        .title(name.clone())
        .link(feed.request_url.as_str())
        .last_build_date(
            now.format(&Rfc2822)
                .inspect_err(|e| error!("could not format the last build date ({now}): {e:#}"))
                .ok(),
        )
        .generator(Some(format!("Feedgen {}", env!("CARGO_PKG_VERSION"))));

    for entry in entries {
        channel.item(
            ItemBuilder::default()
                .title(Some(entry.title))
                .link(Some(entry.url.into()))
                .description(Some(entry.description))
                .author(entry.author)
                .guid(Some(
                    GuidBuilder::default()
                        .value(format!("feedgen/{}/{}", name, entry.id))
                        .permalink(false)
                        .build(),
                ))
                .pub_date(entry.pub_date.and_then(|pub_date| {
                    pub_date
                        .format(&Rfc2822)
                        .inspect_err(|e| {
                            error!("could not format the publication date ({pub_date}): {e:#}")
                        })
                        .ok()
                }))
                .build(),
        );
    }

    let channel = channel.build();

    Ok((
        [(header::CONTENT_TYPE, "application/rss+xml")],
        channel.to_string(),
    ))
}

pub async fn update_feed(State(state): State<AppState>, Path(name): Path<String>) -> Result<()> {
    let feed = state.feeds.get(&name).ok_or(StatusCode::NOT_FOUND)?;
    let notify = feed.force_update.as_ref().ok_or(FeedCannotBeUpdated { name })?;
    notify.notify_waiters();

    Ok(())
}
