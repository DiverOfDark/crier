//! Front-page fetchers for Hacker News and Lobsters.

use crate::story::{Candidate, Source};
use anyhow::Context;
use futures::{stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;

/// How many top-level comments are handed to the LLM as discussion context.
pub const MAX_COMMENTS: usize = 5;

#[derive(Clone)]
pub struct Sources {
    http: Client,
    hn_base: String,
    lobsters_base: String,
}

#[derive(Deserialize)]
struct HnItem {
    id: u64,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    title: String,
    url: Option<String>,
    text: Option<String>,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    descendants: i64,
    #[serde(default)]
    kids: Vec<u64>,
    #[serde(default)]
    dead: bool,
    #[serde(default)]
    deleted: bool,
}

#[derive(Deserialize)]
struct LobstersStory {
    short_id: String,
    title: String,
    #[serde(default)]
    url: String,
    score: i64,
    comment_count: i64,
    #[serde(default)]
    description_plain: String,
    comments_url: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct LobstersThread {
    #[serde(default)]
    comments: Vec<LobstersComment>,
}

#[derive(Deserialize)]
struct LobstersComment {
    #[serde(default)]
    comment_plain: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    depth: i64,
    #[serde(default)]
    is_deleted: bool,
    #[serde(default)]
    is_moderated: bool,
}

impl Sources {
    pub fn new(http: Client, hn_base: String, lobsters_base: String) -> Self {
        Self {
            http,
            hn_base: hn_base.trim_end_matches('/').to_string(),
            lobsters_base: lobsters_base.trim_end_matches('/').to_string(),
        }
    }

    async fn hn_item(&self, id: u64) -> anyhow::Result<Option<HnItem>> {
        let url = format!("{}/item/{id}.json", self.hn_base);
        let item: Option<HnItem> = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(item)
    }

    pub async fn hn_top(&self, n: usize) -> anyhow::Result<Vec<Candidate>> {
        let ids: Vec<u64> = self
            .http
            .get(format!("{}/topstories.json", self.hn_base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("HN topstories")?;
        let items: Vec<(usize, anyhow::Result<Option<HnItem>>)> =
            stream::iter(ids.into_iter().take(n).enumerate())
                .map(|(i, id)| async move { (i, self.hn_item(id).await) })
                .buffered(8)
                .collect()
                .await;
        let mut out = Vec::new();
        for (i, item) in items {
            let item = match item {
                Ok(Some(item)) => item,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "HN item fetch failed");
                    continue;
                }
            };
            if item.kind != "story" || item.dead || item.deleted || item.title.is_empty() {
                continue;
            }
            out.push(Candidate {
                source: Source::Hn,
                source_id: item.id.to_string(),
                rank: i + 1,
                title: item.title,
                url: item.url.filter(|u| !u.is_empty()),
                discussion_url: format!("https://news.ycombinator.com/item?id={}", item.id),
                score: item.score,
                comments: item.descendants,
                tags: vec![],
                text: item
                    .text
                    .map(|t| html_to_text(&t))
                    .filter(|t| !t.is_empty()),
            });
        }
        Ok(out)
    }

    pub async fn lobsters_top(&self, n: usize) -> anyhow::Result<Vec<Candidate>> {
        let stories: Vec<LobstersStory> = self
            .http
            .get(format!("{}/hottest.json", self.lobsters_base))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("Lobsters hottest")?;
        Ok(stories
            .into_iter()
            .take(n)
            .enumerate()
            .map(|(i, s)| Candidate {
                source: Source::Lobsters,
                source_id: s.short_id,
                rank: i + 1,
                title: s.title,
                url: Some(s.url).filter(|u| !u.is_empty()),
                discussion_url: s.comments_url,
                score: s.score,
                comments: s.comment_count,
                tags: s.tags,
                text: Some(s.description_plain).filter(|t| !t.trim().is_empty()),
            })
            .collect())
    }

    /// Top-level comments from the story's discussion, best first.
    pub async fn comments(&self, source: Source, source_id: &str) -> anyhow::Result<Vec<String>> {
        match source {
            Source::Hn => {
                let id: u64 = source_id.parse()?;
                let Some(story) = self.hn_item(id).await? else {
                    return Ok(vec![]);
                };
                // `kids` is already in HN's display order.
                let kids: Vec<anyhow::Result<Option<HnItem>>> =
                    stream::iter(story.kids.into_iter().take(MAX_COMMENTS * 2))
                        .map(|id| self.hn_item(id))
                        .buffered(8)
                        .collect()
                        .await;
                Ok(kids
                    .into_iter()
                    .filter_map(|k| k.ok().flatten())
                    .filter(|k| !k.dead && !k.deleted)
                    .filter_map(|k| k.text.map(|t| html_to_text(&t)))
                    .filter(|t| !t.is_empty())
                    .take(MAX_COMMENTS)
                    .collect())
            }
            Source::Lobsters => {
                let thread: LobstersThread = self
                    .http
                    .get(format!("{}/s/{source_id}.json", self.lobsters_base))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let mut top: Vec<LobstersComment> = thread
                    .comments
                    .into_iter()
                    .filter(|c| c.depth == 0 && !c.is_deleted && !c.is_moderated)
                    .filter(|c| !c.comment_plain.trim().is_empty())
                    .collect();
                top.sort_by_key(|c| std::cmp::Reverse(c.score));
                Ok(top
                    .into_iter()
                    .take(MAX_COMMENTS)
                    .map(|c| c.comment_plain)
                    .collect())
            }
        }
    }
}

/// Visible text of an HTML fragment, whitespace collapsed.
pub fn html_to_text(html: &str) -> String {
    let frag = scraper::Html::parse_fragment(html);
    collapse_ws(&frag.root_element().text().collect::<Vec<_>>().join(" "))
}

pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    #[test]
    fn strips_hn_comment_html() {
        assert_eq!(
            html_to_text("It&#x27;s <i>fast</i>.<p>Second &amp; last"),
            "It's fast . Second & last"
        );
    }

    #[tokio::test]
    async fn hn_top_skips_jobs_and_dead() {
        let s = MockServer::start_async().await;
        s.mock(|w, t| {
            w.path("/topstories.json");
            t.json_body(json!([1, 2, 3, 4]));
        });
        s.mock(|w, t| {
            w.path("/item/1.json");
            t.json_body(json!({"id":1,"type":"story","title":"A","url":"https://a.dev","score":10,"descendants":3}));
        });
        s.mock(|w, t| {
            w.path("/item/2.json");
            t.json_body(json!({"id":2,"type":"job","title":"Hiring"}));
        });
        s.mock(|w, t| {
            w.path("/item/3.json");
            t.json_body(
                json!({"id":3,"type":"story","title":"Ask HN: B","text":"<p>hi</p>","score":5}),
            );
        });
        let src = Sources::new(Client::new(), s.base_url(), s.base_url());
        let top = src.hn_top(3).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].url.as_deref(), Some("https://a.dev"));
        assert_eq!(top[1].rank, 3);
        assert_eq!(top[1].url, None);
        assert_eq!(top[1].text.as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn lobsters_comments_best_top_level_first() {
        let s = MockServer::start_async().await;
        s.mock(|w, t| {
            w.path("/s/abc.json");
            t.json_body(json!({"comments":[
                {"comment_plain":"meh","score":1,"depth":0},
                {"comment_plain":"reply","score":50,"depth":1},
                {"comment_plain":"great","score":9,"depth":0}
            ]}));
        });
        let src = Sources::new(Client::new(), s.base_url(), s.base_url());
        let c = src.comments(Source::Lobsters, "abc").await.unwrap();
        assert_eq!(c, vec!["great", "meh"]);
    }
}
