//! Best-effort plain-text extraction of a linked article, used as LLM context.

use crate::sources::collapse_ws;
use reqwest::Client;
use scraper::{ElementRef, Html, Node, Selector};

const MAX_BYTES: usize = 3 * 1024 * 1024;
pub const MAX_CHARS: usize = 12_000;
const SKIP: &[&str] = &[
    "script", "style", "noscript", "nav", "header", "footer", "aside", "form", "svg", "button",
    "iframe", "template",
];

/// Fetches `url` and returns its readable text, or `None` when the page is not
/// HTML, is unreachable, or has no usable text. Never fails the caller.
pub async fn fetch_text(http: &Client, url: &str) -> Option<String> {
    match fetch(http, url).await {
        Ok(t) => t,
        Err(e) => {
            tracing::info!(%url, error = %e, "article fetch failed; continuing without it");
            None
        }
    }
}

async fn fetch(http: &Client, url: &str) -> anyhow::Result<Option<String>> {
    let mut resp = http
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await?
        .error_for_status()?;
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|ct| ct.contains("html"));
    if !is_html {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_BYTES {
            break;
        }
    }
    Ok(extract(&String::from_utf8_lossy(&body)))
}

pub fn extract(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let root = ["article", "main", "body"]
        .iter()
        .filter_map(|s| doc.select(&Selector::parse(s).ok()?).next())
        .find(|el| collapse_ws(&el.text().collect::<String>()).len() > 200)
        .unwrap_or_else(|| doc.root_element());
    let mut parts = Vec::new();
    collect(root, &mut parts);
    let text = collapse_ws(&parts.join(" "));
    if text.len() < 80 {
        return None;
    }
    Some(truncate_chars(&text, MAX_CHARS))
}

fn collect(el: ElementRef, out: &mut Vec<String>) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push(t.to_string()),
            Node::Element(e) if SKIP.contains(&e.name()) => {}
            Node::Element(_) => {
                if let Some(c) = ElementRef::wrap(child) {
                    collect(c, out);
                }
            }
            _ => {}
        }
    }
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_article_and_skips_chrome() {
        let body = "Real content sentence. ".repeat(20);
        let html = format!(
            "<html><head><style>.x{{}}</style></head><body><nav>Home About</nav>\
             <article><h1>Title</h1><p>{body}</p><script>evil()</script></article>\
             <footer>Copyright</footer></body></html>"
        );
        let t = extract(&html).unwrap();
        assert!(t.starts_with("Title Real content"));
        assert!(!t.contains("evil"));
        assert!(!t.contains("Home About"));
        assert!(!t.contains("Copyright"));
    }

    #[test]
    fn empty_pages_yield_none() {
        assert_eq!(
            extract("<html><body><div id=app></div></body></html>"),
            None
        );
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_chars("héllo", 2), "hé…");
        assert_eq!(truncate_chars("hi", 5), "hi");
    }
}
