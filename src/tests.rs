//! End-to-end cycles against mocked HN, Lobsters, LLM and Telegram.

use crate::config::{Cli, Command, Config};
use crate::pipeline::{App, CycleStats};
use clap::Parser;
use httpmock::prelude::*;
use serde_json::json;

fn config(server: &MockServer, db: &str, extra: &[&str]) -> Config {
    let base = server.base_url();
    let mut args = vec![
        "crier",
        "once",
        "--database-url",
        db,
        "--openai-api-key",
        "k",
        "--telegram-bot-token",
        "T",
        "--telegram-chat-id",
        "@chan",
        "--hn-top",
        "3",
        "--lobsters-top",
        "3",
        "--initial-posts",
        "1",
    ];
    let openai = format!("{base}/v1/chat/completions");
    let hn = format!("{base}/hn");
    let lob = format!("{base}/lob");
    args.extend([
        "--openai-url",
        &openai,
        "--hn-api-base",
        &hn,
        "--lobsters-base",
        &lob,
    ]);
    args.extend(["--telegram-api-base", &base]);
    args.extend(extra);
    match Cli::try_parse_from(args).unwrap().command {
        Command::Once(c) | Command::Run(c) => c,
    }
}

fn hn_item(s: &MockServer, id: u64, path: &str) {
    // Article links point at the mock server (404 -> no article text), never the internet.
    let url = s.url(path);
    s.mock(|w, t| {
        w.path(format!("/hn/item/{id}.json"));
        t.json_body(json!({"id":id,"type":"story","title":format!("HN {id}"),"url":url,"score":100,"descendants":5}));
    });
}

fn llm_ok(s: &MockServer) -> httpmock::Mock<'_> {
    s.mock(|w, t| {
        w.method(POST).path("/v1/chat/completions");
        t.json_body(json!({"choices":[{"message":{"content":
            "{\"headline\":\"Written\",\"body\":\"Body.\",\"discussion\":null,\"hashtags\":[\"rust\"]}"}}]}));
    })
}

fn telegram_ok(s: &MockServer) -> (httpmock::Mock<'_>, httpmock::Mock<'_>) {
    let send = s.mock(|w, t| {
        w.method(POST).path("/botT/sendMessage");
        t.json_body(json!({"ok":true,"result":{"message_id":7}}));
    });
    let edit = s.mock(|w, t| {
        w.method(POST)
            .path("/botT/editMessageText")
            .body_contains("Lobsters")
            .body_contains("\"message_id\":7");
        t.json_body(json!({"ok":true,"result":{}}));
    });
    (send, edit)
}

fn db_url(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("crier-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    format!("sqlite://{}", dir.join("db.sqlite").display())
}

#[tokio::test(flavor = "multi_thread")]
async fn first_run_posts_few_then_only_new_stories() {
    let s = MockServer::start_async().await;
    let mut top = s.mock(|w, t| {
        w.path("/hn/topstories.json");
        t.json_body(json!([1, 2]));
    });
    hn_item(&s, 1, "/one");
    hn_item(&s, 2, "/two");
    hn_item(&s, 3, "/three");
    let mut lob = s.mock(|w, t| {
        w.path("/lob/hottest.json");
        t.json_body(json!([]));
    });
    let llm = llm_ok(&s);
    let (send, edit) = telegram_ok(&s);
    let app = App::new(config(&s, &db_url("first"), &[])).await.unwrap();

    // First run: two new stories, only one posted.
    let st = app.cycle().await.unwrap();
    assert_eq!(
        st,
        CycleStats {
            seen: 2,
            new: 2,
            posted: 1,
            ..Default::default()
        }
    );
    assert_eq!(send.hits(), 1);

    // Nothing changed: nothing posted.
    let st = app.cycle().await.unwrap();
    assert_eq!(
        st,
        CycleStats {
            seen: 2,
            ..Default::default()
        }
    );

    // Story 3 enters the front page and gets posted.
    top.delete();
    top = s.mock(|w, t| {
        w.path("/hn/topstories.json");
        t.json_body(json!([3, 1, 2]));
    });
    let st = app.cycle().await.unwrap();
    assert_eq!(st.posted, 1);
    assert_eq!(send.hits(), 2);
    assert!(llm.hits() >= 2);

    // The same link shows up on Lobsters: existing post is edited, not reposted.
    lob.delete();
    lob = s.mock(|w, t| {
        w.path("/lob/hottest.json");
        t.json_body(json!([{"short_id":"xyz","title":"L","url":s.url("/three/?utm_source=lobsters"),
            "score":30,"comment_count":4,"comments_url":"https://lobste.rs/s/xyz/l","tags":["rust"]}]));
    });
    let st = app.cycle().await.unwrap();
    assert_eq!(st.posted, 0);
    assert_eq!(st.edited, 1);
    assert_eq!(edit.hits(), 1);
    assert_eq!(send.hits(), 2);
    top.assert_hits(2);
    lob.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread")]
async fn llm_outage_falls_back_to_title_after_retries() {
    let s = MockServer::start_async().await;
    s.mock(|w, t| {
        w.path("/hn/topstories.json");
        t.json_body(json!([1]));
    });
    hn_item(&s, 1, "/one");
    s.mock(|w, t| {
        w.path("/lob/hottest.json");
        t.json_body(json!([]));
    });
    s.mock(|w, t| {
        w.method(POST).path("/v1/chat/completions");
        t.status(500).body("down");
    });
    let fallback = s.mock(|w, t| {
        w.method(POST)
            .path("/botT/sendMessage")
            .body_contains("<b>HN 1</b>");
        t.json_body(json!({"ok":true,"result":{"message_id":1}}));
    });
    let app = App::new(config(&s, &db_url("llm"), &[])).await.unwrap();

    assert_eq!(app.cycle().await.unwrap().failed, 1);
    assert_eq!(app.cycle().await.unwrap().failed, 1);
    assert_eq!(app.cycle().await.unwrap().posted, 1);
    assert_eq!(fallback.hits(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn telegram_failure_keeps_llm_output_for_retry() {
    let s = MockServer::start_async().await;
    s.mock(|w, t| {
        w.path("/hn/topstories.json");
        t.json_body(json!([1]));
    });
    hn_item(&s, 1, "/one");
    s.mock(|w, t| {
        w.path("/lob/hottest.json");
        t.json_body(json!([]));
    });
    let llm = llm_ok(&s);
    let mut tg = s.mock(|w, t| {
        w.path("/botT/sendMessage");
        t.json_body(json!({"ok":false,"description":"Forbidden: bot is not a member"}));
    });
    let app = App::new(config(&s, &db_url("tg"), &[])).await.unwrap();
    assert_eq!(app.cycle().await.unwrap().failed, 1);

    tg.delete();
    tg = s.mock(|w, t| {
        w.path("/botT/sendMessage").body_contains("Written");
        t.json_body(json!({"ok":true,"result":{"message_id":3}}));
    });
    assert_eq!(app.cycle().await.unwrap().posted, 1);
    assert_eq!(llm.hits(), 1, "LLM output is reused on retry");
    assert_eq!(tg.hits(), 1);
}
