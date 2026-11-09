use crate::llm::Post;
use crate::render::{Discussion, Links};
use crate::story::{Candidate, Source};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;

pub async fn connect(url: &str) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Posted,
    Skipped,
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Posted => "posted",
            Status::Skipped => "skipped",
            Status::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Status {
        match s {
            "posted" => Status::Posted,
            "skipped" => Status::Skipped,
            "failed" => Status::Failed,
            _ => Status::Pending,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Upsert {
    New,
    /// Already known. `linked` is true when this source was not attached
    /// before (e.g. the HN story now also shows up on Lobsters).
    Existing {
        status: Status,
        linked: bool,
    },
}

#[derive(Debug, Clone)]
pub struct StoryRow {
    pub key: String,
    pub title: String,
    pub url: Option<String>,
    pub self_text: Option<String>,
    pub tags: Vec<String>,
    pub first_source: Source,
    pub hn_id: Option<String>,
    pub lobsters_id: Option<String>,
    pub links: Links,
    pub attempts: i64,
    pub post: Option<Post>,
}

pub async fn is_empty(pool: &SqlitePool) -> anyhow::Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stories")
        .fetch_one(pool)
        .await?;
    Ok(n == 0)
}

/// Records `c`, refreshing its score and comment count, and attaches its
/// source if the story was first seen elsewhere. New rows get `new_status`.
pub async fn upsert(
    pool: &SqlitePool,
    c: &Candidate,
    new_status: Status,
) -> anyhow::Result<Upsert> {
    let key = c.key();
    let existing = sqlx::query("SELECT status, hn_id, lobsters_id FROM stories WHERE key = ?")
        .bind(&key)
        .fetch_optional(pool)
        .await?;
    let tags = serde_json::to_string(&c.tags)?;
    let Some(row) = existing else {
        let (hn_id, hn_score, hn_comments) = match c.source {
            Source::Hn => (Some(&c.source_id), Some(c.score), Some(c.comments)),
            Source::Lobsters => (None, None, None),
        };
        let (lob_id, lob_url, lob_score, lob_comments) = match c.source {
            Source::Lobsters => (
                Some(&c.source_id),
                Some(&c.discussion_url),
                Some(c.score),
                Some(c.comments),
            ),
            Source::Hn => (None, None, None, None),
        };
        sqlx::query(
            "INSERT INTO stories (key, title, url, self_text, tags, first_source, hn_id, hn_score, \
             hn_comments, lobsters_id, lobsters_url, lobsters_score, lobsters_comments, status, \
             first_seen_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&key)
        .bind(&c.title)
        .bind(&c.url)
        .bind(&c.text)
        .bind(&tags)
        .bind(source_str(c.source))
        .bind(hn_id)
        .bind(hn_score)
        .bind(hn_comments)
        .bind(lob_id)
        .bind(lob_url)
        .bind(lob_score)
        .bind(lob_comments)
        .bind(new_status.as_str())
        .bind(now())
        .bind(now())
        .execute(pool)
        .await?;
        return Ok(Upsert::New);
    };
    let status = Status::parse(row.get::<String, _>("status").as_str());
    let linked = match c.source {
        Source::Hn => row.get::<Option<String>, _>("hn_id").is_none(),
        Source::Lobsters => row.get::<Option<String>, _>("lobsters_id").is_none(),
    };
    match c.source {
        Source::Hn => {
            sqlx::query(
                "UPDATE stories SET hn_id = ?, hn_score = ?, hn_comments = ?, updated_at = ? WHERE key = ?",
            )
            .bind(&c.source_id)
            .bind(c.score)
            .bind(c.comments)
            .bind(now())
            .bind(&key)
            .execute(pool)
            .await?;
        }
        Source::Lobsters => {
            sqlx::query(
                "UPDATE stories SET lobsters_id = ?, lobsters_url = ?, lobsters_score = ?, \
                 lobsters_comments = ?, tags = CASE WHEN tags = '[]' THEN ? ELSE tags END, \
                 updated_at = ? WHERE key = ?",
            )
            .bind(&c.source_id)
            .bind(&c.discussion_url)
            .bind(c.score)
            .bind(c.comments)
            .bind(&tags)
            .bind(now())
            .bind(&key)
            .execute(pool)
            .await?;
        }
    }
    Ok(Upsert::Existing { status, linked })
}

fn source_str(s: Source) -> &'static str {
    match s {
        Source::Hn => "hn",
        Source::Lobsters => "lobsters",
    }
}

fn row_to_story(r: &sqlx::sqlite::SqliteRow) -> anyhow::Result<StoryRow> {
    let hn_id: Option<String> = r.get("hn_id");
    let lobsters_url: Option<String> = r.get("lobsters_url");
    let post_json: Option<String> = r.get("post_json");
    Ok(StoryRow {
        key: r.get("key"),
        title: r.get("title"),
        url: r.get("url"),
        self_text: r.get("self_text"),
        tags: serde_json::from_str(&r.get::<String, _>("tags")).unwrap_or_default(),
        first_source: if r.get::<String, _>("first_source") == "lobsters" {
            Source::Lobsters
        } else {
            Source::Hn
        },
        links: Links {
            url: r.get("url"),
            hn: hn_id.as_ref().map(|id| Discussion {
                url: format!("https://news.ycombinator.com/item?id={id}"),
                score: r.get::<Option<i64>, _>("hn_score").unwrap_or(0),
                comments: r.get::<Option<i64>, _>("hn_comments").unwrap_or(0),
            }),
            lobsters: lobsters_url.map(|url| Discussion {
                url,
                score: r.get::<Option<i64>, _>("lobsters_score").unwrap_or(0),
                comments: r.get::<Option<i64>, _>("lobsters_comments").unwrap_or(0),
            }),
        },
        hn_id,
        lobsters_id: r.get("lobsters_id"),
        attempts: r.get("attempts"),
        post: post_json.and_then(|j| serde_json::from_str(&j).ok()),
    })
}

/// Pending stories in the order they were first seen.
pub async fn pending(pool: &SqlitePool, limit: usize) -> anyhow::Result<Vec<StoryRow>> {
    let rows = sqlx::query("SELECT * FROM stories WHERE status = 'pending' ORDER BY rowid LIMIT ?")
        .bind(limit as i64)
        .fetch_all(pool)
        .await?;
    rows.iter().map(row_to_story).collect()
}

pub async fn story(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<StoryRow>> {
    let row = sqlx::query("SELECT * FROM stories WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(row_to_story).transpose()
}

pub async fn save_post(pool: &SqlitePool, key: &str, post: &Post) -> anyhow::Result<()> {
    sqlx::query("UPDATE stories SET post_json = ?, updated_at = ? WHERE key = ?")
        .bind(serde_json::to_string(post)?)
        .bind(now())
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Counts a failed attempt; past `give_up_after` the story is marked failed.
pub async fn record_failure(
    pool: &SqlitePool,
    key: &str,
    error: &str,
    give_up_after: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE stories SET attempts = attempts + 1, last_error = ?, updated_at = ?, \
         status = CASE WHEN attempts + 1 >= ? THEN 'failed' ELSE status END WHERE key = ?",
    )
    .bind(error)
    .bind(now())
    .bind(give_up_after)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_posted(
    pool: &SqlitePool,
    key: &str,
    chat_id: &str,
    message_id: i64,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT OR REPLACE INTO posts (story_key, chat_id, message_id, posted_at) VALUES (?,?,?,?)",
    )
    .bind(key)
    .bind(chat_id)
    .bind(message_id)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE stories SET status = 'posted', last_error = NULL, updated_at = ? WHERE key = ?",
    )
    .bind(now())
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Messages already sent for `key`, as (chat_id, message_id).
pub async fn posts_for(pool: &SqlitePool, key: &str) -> anyhow::Result<Vec<(String, i64)>> {
    let rows = sqlx::query("SELECT chat_id, message_id FROM posts WHERE story_key = ?")
        .bind(key)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get("chat_id"), r.get("message_id")))
        .collect())
}
