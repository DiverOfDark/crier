//! Minimal Telegram Bot API client: send and edit HTML messages.

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone)]
pub struct Telegram {
    http: Client,
    base: String,
    chat_id: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    ok: bool,
    result: Option<Value>,
    description: Option<String>,
    parameters: Option<ResponseParameters>,
}

#[derive(Deserialize)]
struct ResponseParameters {
    retry_after: Option<u64>,
}

impl Telegram {
    pub fn new(http: Client, api_base: &str, token: &str, chat_id: String) -> Self {
        Self {
            http,
            base: format!("{}/bot{token}", api_base.trim_end_matches('/')),
            chat_id,
        }
    }

    /// Sends `html` and returns the message id.
    pub async fn send(&self, html: &str, preview_url: Option<&str>) -> anyhow::Result<i64> {
        let mut body = json!({
            "chat_id": self.chat_id,
            "text": html,
            "parse_mode": "HTML",
        });
        body["link_preview_options"] = match preview_url {
            Some(u) => json!({"url": u, "prefer_small_media": true}),
            None => json!({"is_disabled": true}),
        };
        let result = self.call("sendMessage", &body).await?;
        result["message_id"]
            .as_i64()
            .context("sendMessage result has no message_id")
    }

    pub async fn edit(
        &self,
        message_id: i64,
        html: &str,
        preview_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = json!({
            "chat_id": self.chat_id,
            "message_id": message_id,
            "text": html,
            "parse_mode": "HTML",
        });
        body["link_preview_options"] = match preview_url {
            Some(u) => json!({"url": u, "prefer_small_media": true}),
            None => json!({"is_disabled": true}),
        };
        match self.call("editMessageText", &body).await {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("message is not modified") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Calls `method`, waiting out one flood-control response.
    async fn call(&self, method: &str, body: &Value) -> anyhow::Result<Value> {
        for attempt in 0..2 {
            let resp: ApiResponse = self
                .http
                .post(format!("{}/{method}", self.base))
                .json(body)
                .send()
                .await
                // The URL contains the bot token; keep it out of logs.
                .map_err(|e| anyhow::anyhow!("telegram {method}: {}", e.without_url()))?
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("telegram {method}: {}", e.without_url()))?;
            if resp.ok {
                return Ok(resp.result.unwrap_or(Value::Null));
            }
            let retry = resp.parameters.and_then(|p| p.retry_after);
            if let (0, Some(secs)) = (attempt, retry) {
                tracing::warn!(method, secs, "telegram rate limit; waiting");
                tokio::time::sleep(Duration::from_secs(secs.min(120))).await;
                continue;
            }
            anyhow::bail!(
                "telegram {method}: {}",
                resp.description.unwrap_or_else(|| "unknown error".into())
            );
        }
        unreachable!("loop returns or bails")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[tokio::test]
    async fn send_returns_message_id() {
        let s = MockServer::start_async().await;
        let m = s.mock(|w, t| {
            w.method(POST)
                .path("/botTOKEN/sendMessage")
                .json_body_partial(r#"{"chat_id":"@chan","parse_mode":"HTML","link_preview_options":{"url":"https://a.dev"}}"#);
            t.json_body(json!({"ok":true,"result":{"message_id":77}}));
        });
        let tg = Telegram::new(Client::new(), &s.base_url(), "TOKEN", "@chan".into());
        assert_eq!(
            tg.send("<b>x</b>", Some("https://a.dev")).await.unwrap(),
            77
        );
        m.assert();
    }

    #[tokio::test]
    async fn errors_carry_description_not_token() {
        let s = MockServer::start_async().await;
        s.mock(|_, t| {
            t.json_body(json!({"ok":false,"description":"Bad Request: chat not found"}));
        });
        let tg = Telegram::new(Client::new(), &s.base_url(), "SECRET", "@chan".into());
        let e = tg.send("x", None).await.unwrap_err().to_string();
        assert!(e.contains("chat not found"));
        assert!(!e.contains("SECRET"));
    }

    #[tokio::test]
    async fn unmodified_edit_is_ok() {
        let s = MockServer::start_async().await;
        s.mock(|_, t| {
            t.json_body(json!({"ok":false,"description":"Bad Request: message is not modified"}));
        });
        let tg = Telegram::new(Client::new(), &s.base_url(), "T", "@chan".into());
        tg.edit(1, "x", None).await.unwrap();
    }
}
