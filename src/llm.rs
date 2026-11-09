//! Turns a story plus its context into a short Telegram post via an
//! OpenAI-compatible chat completions endpoint.

use crate::article::truncate_chars;
use anyhow::Context;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// What the model writes; rendering into Telegram HTML happens in `render`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub headline: String,
    pub body: String,
    pub discussion: Option<String>,
    pub hashtags: Vec<String>,
}

/// Everything the model gets to see about one story.
#[derive(Debug, Clone, Default)]
pub struct StoryContext {
    pub title: String,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub self_text: Option<String>,
    pub article_text: Option<String>,
    pub comments: Vec<String>,
}

#[derive(Clone)]
pub struct Writer {
    http: Client,
    url: String,
    api_key: String,
    model: String,
    reasoning_effort: Option<String>,
    language: String,
}

const SYSTEM_PROMPT: &str = r#"You write posts for a Telegram channel that relays the best stories from Hacker News and Lobsters to busy software engineers.

For each story write:
- headline: the story title, rewritten only if the original is vague or clickbait. Max 110 characters. No emoji.
- body: 2-3 short sentences, at most 60 words. Say concretely what the thing is and what is new or interesting about it: names, numbers, techniques, trade-offs. No hype, no filler like "In this article" or "The author explores". Do not just restate the headline.
- discussion: one sentence, at most 30 words, on the most substantive point, disagreement or correction from the comments. null when no comments are provided or they add nothing.
- hashtags: 1-3 lowercase topic tags without '#', letters/digits/underscores only (e.g. rust, databases, security).

Rules:
- Use only facts present in the provided material. If the article text is missing, write a shorter body from the title, self-text and comments, and do not guess the content.
- Plain text only: no markdown, no HTML, no links.
- Write in {LANGUAGE}. Keep product names, code identifiers and the headline's proper nouns as they are."#;

impl Writer {
    pub fn new(
        http: Client,
        url: String,
        api_key: String,
        model: String,
        reasoning_effort: Option<String>,
        language: String,
    ) -> Self {
        Self {
            http,
            url,
            api_key,
            model,
            reasoning_effort,
            language,
        }
    }

    pub async fn write(&self, ctx: &StoryContext) -> anyhow::Result<Post> {
        let mut body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT.replace("{LANGUAGE}", &self.language)},
                {"role": "user", "content": user_message(ctx)},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "post", "strict": true, "schema": schema()},
            },
        });
        if let Some(effort) = &self.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        anyhow::ensure!(
            status.is_success(),
            "LLM HTTP {status}: {}",
            truncate_chars(&text, 500)
        );
        let v: Value = serde_json::from_str(&text).context("LLM response is not JSON")?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .context("LLM response has no message content")?;
        parse_post(content)
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["headline", "body", "discussion", "hashtags"],
        "properties": {
            "headline": {"type": "string"},
            "body": {"type": "string"},
            "discussion": {"type": ["string", "null"]},
            "hashtags": {"type": "array", "items": {"type": "string"}},
        },
    })
}

fn user_message(ctx: &StoryContext) -> String {
    let mut m = format!("Title: {}\n", ctx.title);
    if let Some(u) = &ctx.url {
        m.push_str(&format!("Link: {u}\n"));
    }
    if !ctx.tags.is_empty() {
        m.push_str(&format!("Lobsters tags: {}\n", ctx.tags.join(", ")));
    }
    if let Some(t) = &ctx.self_text {
        m.push_str(&format!(
            "\nSubmitter's text:\n{}\n",
            truncate_chars(t, 3000)
        ));
    }
    match &ctx.article_text {
        Some(t) => m.push_str(&format!("\nArticle text (may be truncated):\n{t}\n")),
        None => m.push_str("\nArticle text: unavailable\n"),
    }
    if !ctx.comments.is_empty() {
        m.push_str("\nTop comments:\n");
        for (i, c) in ctx.comments.iter().enumerate() {
            m.push_str(&format!("{}. {}\n", i + 1, truncate_chars(c, 1200)));
        }
    }
    m
}

pub fn parse_post(content: &str) -> anyhow::Result<Post> {
    let mut p: Post = serde_json::from_str(content.trim()).context("LLM post JSON invalid")?;
    p.headline = p.headline.trim().to_string();
    p.body = p.body.trim().to_string();
    p.discussion = p
        .discussion
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    p.hashtags = p
        .hashtags
        .iter()
        .map(|h| {
            h.trim_start_matches('#')
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>()
                .trim_matches('_')
                .to_string()
        })
        .filter(|h| !h.is_empty())
        .take(3)
        .collect();
    anyhow::ensure!(
        !p.headline.is_empty() && !p.body.is_empty(),
        "LLM post is empty"
    );
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn cleans_hashtags_and_empty_discussion() {
        let p = parse_post(
            r##"{"headline":" T ","body":"B","discussion":"  ","hashtags":["#Rust","web dev","--","a","b"]}"##,
        )
        .unwrap();
        assert_eq!(p.headline, "T");
        assert_eq!(p.discussion, None);
        assert_eq!(p.hashtags, vec!["rust", "web_dev", "a"]);
    }

    #[tokio::test]
    async fn sends_schema_and_reasoning_effort() {
        let s = MockServer::start_async().await;
        let m = s.mock(|w, t| {
            w.method(POST)
                .path("/v1/chat/completions")
                .header("authorization", "Bearer k")
                .body_contains("\"reasoning_effort\":\"low\"")
                .body_contains("\"json_schema\"")
                .body_contains("Article text: unavailable");
            t.json_body(json!({"choices":[{"message":{"content":
                "{\"headline\":\"H\",\"body\":\"B\",\"discussion\":null,\"hashtags\":[\"x\"]}"}}]}));
        });
        let w = Writer::new(
            Client::new(),
            s.url("/v1/chat/completions"),
            "k".into(),
            "m".into(),
            Some("low".into()),
            "English".into(),
        );
        let p = w
            .write(&StoryContext {
                title: "T".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        m.assert();
        assert_eq!(p.headline, "H");
    }

    #[tokio::test]
    async fn http_errors_surface() {
        let s = MockServer::start_async().await;
        s.mock(|_, t| {
            t.status(429).body("slow down");
        });
        let w = Writer::new(
            Client::new(),
            s.url("/"),
            "k".into(),
            "m".into(),
            None,
            "English".into(),
        );
        let e = w.write(&StoryContext::default()).await.unwrap_err();
        assert!(e.to_string().contains("429"));
    }
}
