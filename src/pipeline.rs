//! One poll cycle: fetch front pages, record what is new, write and send posts.

use crate::article;
use crate::config::Config;
use crate::db::{self, Status, StoryRow, Upsert};
use crate::llm::{Post, StoryContext, Writer};
use crate::render::{fallback_post, render};
use crate::sources::Sources;
use crate::story::{Candidate, Source};
use crate::telegram::Telegram;
use reqwest::Client;
use sqlx::SqlitePool;
use std::time::Duration;

/// LLM failures (in separate cycles) before posting the title-only fallback.
const LLM_ATTEMPTS: i64 = 3;
/// Failed cycles before a story is given up on.
const GIVE_UP_AFTER: i64 = 6;
/// Spacing between messages; Telegram allows ~20/minute per channel.
const SEND_SPACING: Duration = Duration::from_secs(3);

pub struct App {
    pub cfg: Config,
    pub pool: SqlitePool,
    pub http: Client,
    pub sources: Sources,
    pub writer: Writer,
    /// `None` in dry-run mode: posts go to stdout.
    pub telegram: Option<Telegram>,
}

#[derive(Debug, Default, PartialEq)]
pub struct CycleStats {
    pub seen: usize,
    pub new: usize,
    pub posted: usize,
    pub edited: usize,
    pub failed: usize,
}

impl App {
    pub async fn new(cfg: Config) -> anyhow::Result<Self> {
        let http = Client::builder()
            .user_agent(concat!(
                "crier/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/DiverOfDark/crier)"
            ))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        let pool = db::connect(&cfg.database_url).await?;
        let sources = Sources::new(
            http.clone(),
            cfg.hn_api_base.clone(),
            cfg.lobsters_base.clone(),
        );
        let writer = Writer::new(
            http.clone(),
            cfg.openai_url.clone(),
            cfg.openai_api_key.clone(),
            cfg.openai_model.clone(),
            cfg.openai_reasoning_effort.clone(),
            cfg.post_language.clone(),
        );
        let telegram = (!cfg.dry_run).then(|| {
            Telegram::new(
                http.clone(),
                &cfg.telegram_api_base,
                &cfg.telegram_bot_token,
                cfg.telegram_chat_id.clone(),
            )
        });
        Ok(Self {
            cfg,
            pool,
            http,
            sources,
            writer,
            telegram,
        })
    }

    fn chat_id(&self) -> &str {
        if self.telegram.is_some() {
            &self.cfg.telegram_chat_id
        } else {
            "dry-run"
        }
    }

    pub async fn cycle(&self) -> anyhow::Result<CycleStats> {
        let mut stats = CycleStats::default();
        let first_run = db::is_empty(&self.pool).await?;

        let (hn, lobsters) = tokio::join!(
            self.sources.hn_top(self.cfg.hn_top),
            self.sources.lobsters_top(self.cfg.lobsters_top)
        );
        let mut candidates: Vec<Candidate> = Vec::new();
        for (name, res) in [("hn", hn), ("lobsters", lobsters)] {
            match res {
                Ok(c) => candidates.extend(c),
                Err(e) => tracing::warn!(source = name, error = %e, "front page fetch failed"),
            }
        }
        // Interleave by rank so neither site crowds out the other.
        candidates.sort_by_key(|c| (c.rank, c.source == Source::Lobsters));
        stats.seen = candidates.len();

        let mut relink = Vec::new();
        for c in &candidates {
            let status = if first_run && stats.new >= self.cfg.initial_posts {
                Status::Skipped
            } else {
                Status::Pending
            };
            match db::upsert(&self.pool, c, status).await? {
                Upsert::New => {
                    stats.new += 1;
                    tracing::info!(key = %c.key(), title = %c.title, source = c.source.label(), ?status, "new story");
                }
                Upsert::Existing {
                    status: Status::Posted,
                    linked: true,
                } => relink.push(c.key()),
                Upsert::Existing { .. } => {}
            }
        }
        if first_run && stats.new > self.cfg.initial_posts {
            tracing::info!(
                skipped = stats.new - self.cfg.initial_posts,
                "first run: marked the rest of the current front pages as seen"
            );
        }

        for key in relink {
            match self.relink(&key).await {
                Ok(true) => stats.edited += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(%key, error = %e, "could not add second source to post"),
            }
        }

        for story in db::pending(&self.pool, self.cfg.max_posts_per_cycle).await? {
            match self.publish(&story).await {
                Ok(()) => stats.posted += 1,
                Err(e) => {
                    stats.failed += 1;
                    tracing::warn!(key = %story.key, attempts = story.attempts + 1, error = %format!("{e:#}"), "post failed");
                    db::record_failure(&self.pool, &story.key, &format!("{e:#}"), GIVE_UP_AFTER)
                        .await?;
                }
            }
        }
        Ok(stats)
    }

    async fn publish(&self, story: &StoryRow) -> anyhow::Result<()> {
        let post = match &story.post {
            Some(p) => p.clone(),
            None => {
                let post = match self.write(story).await {
                    Ok(p) => p,
                    Err(e) if story.attempts + 1 >= LLM_ATTEMPTS => {
                        tracing::warn!(key = %story.key, error = %e, "LLM keeps failing; posting title only");
                        fallback_post(&story.title)
                    }
                    Err(e) => return Err(e.context("LLM")),
                };
                db::save_post(&self.pool, &story.key, &post).await?;
                post
            }
        };
        let html = render(&post, &story.links);
        let message_id = match &self.telegram {
            Some(tg) => {
                let id = tg.send(&html, story.url.as_deref()).await?;
                tokio::time::sleep(SEND_SPACING).await;
                id
            }
            None => {
                println!("----- {}\n{html}\n", story.key);
                0
            }
        };
        db::mark_posted(&self.pool, &story.key, self.chat_id(), message_id).await?;
        tracing::info!(key = %story.key, message_id, "posted");
        Ok(())
    }

    async fn write(&self, story: &StoryRow) -> anyhow::Result<Post> {
        let source_id = match story.first_source {
            Source::Hn => story.hn_id.as_deref(),
            Source::Lobsters => story.lobsters_id.as_deref(),
        };
        let (article_text, comments) = tokio::join!(
            async {
                match &story.url {
                    Some(u) => article::fetch_text(&self.http, u).await,
                    None => None,
                }
            },
            async {
                let Some(id) = source_id else { return vec![] };
                self.sources
                    .comments(story.first_source, id)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::info!(key = %story.key, error = %e, "comments fetch failed");
                        vec![]
                    })
            }
        );
        tracing::debug!(
            key = %story.key,
            article_chars = article_text.as_ref().map_or(0, |t| t.chars().count()),
            comments = comments.len(),
            "LLM context gathered"
        );
        self.writer
            .write(&StoryContext {
                title: story.title.clone(),
                url: story.url.clone(),
                tags: story.tags.clone(),
                self_text: story.self_text.clone(),
                article_text,
                comments,
            })
            .await
    }

    /// Re-renders an already-posted story after it appeared on the other site,
    /// so the message links both discussions. Returns whether a message changed.
    async fn relink(&self, key: &str) -> anyhow::Result<bool> {
        let Some(story) = db::story(&self.pool, key).await? else {
            return Ok(false);
        };
        let post = story
            .post
            .clone()
            .unwrap_or_else(|| fallback_post(&story.title));
        let html = render(&post, &story.links);
        let mut edited = false;
        for (chat_id, message_id) in db::posts_for(&self.pool, key).await? {
            if chat_id != self.chat_id() {
                continue;
            }
            match &self.telegram {
                Some(tg) => tg.edit(message_id, &html, story.url.as_deref()).await?,
                None => println!("----- edit {key}\n{html}\n"),
            }
            edited = true;
        }
        if edited {
            tracing::info!(%key, "linked second discussion into existing post");
        }
        Ok(edited)
    }
}
