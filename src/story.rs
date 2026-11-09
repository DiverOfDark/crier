use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Hn,
    Lobsters,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Hn => "HN",
            Source::Lobsters => "Lobsters",
        }
    }
}

/// A story as seen on one source's front page.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub source: Source,
    pub source_id: String,
    pub rank: usize,
    pub title: String,
    /// External link; `None` for self posts (Ask HN, Lobsters text posts).
    pub url: Option<String>,
    pub discussion_url: String,
    pub score: i64,
    pub comments: i64,
    pub tags: Vec<String>,
    /// Self-post text, if any.
    pub text: Option<String>,
}

impl Candidate {
    /// Identity used for de-duplication: the same link submitted to both sites
    /// is one story.
    pub fn key(&self) -> String {
        match self.url.as_deref().and_then(normalize_url) {
            Some(u) => u,
            None => format!("{}:{}", serde_plain(self.source), self.source_id),
        }
    }
}

fn serde_plain(s: Source) -> &'static str {
    match s {
        Source::Hn => "hn",
        Source::Lobsters => "lobsters",
    }
}

/// Canonical form of a link: no scheme, no `www.`, no fragment, no tracking
/// parameters, no trailing slash.
pub fn normalize_url(raw: &str) -> Option<String> {
    let mut u = Url::parse(raw).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.set_fragment(None);
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !(k.starts_with("utm_") || k == "ref" || k == "fbclid" || k == "gclid")
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(kept);
    }
    let host = u.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let path = u.path().trim_end_matches('/');
    let mut out = format!("{host}{path}");
    if let Some(q) = u.query() {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_equivalent_links() {
        let a = normalize_url("https://www.Example.com/post/?utm_source=hn#top").unwrap();
        let b = normalize_url("http://example.com/post").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "example.com/post");
    }

    #[test]
    fn keeps_meaningful_query() {
        assert_eq!(
            normalize_url("https://youtube.com/watch?v=abc&utm_medium=x").unwrap(),
            "youtube.com/watch?v=abc"
        );
    }

    #[test]
    fn self_posts_key_by_source_id() {
        let c = Candidate {
            source: Source::Hn,
            source_id: "42".into(),
            rank: 1,
            title: "Ask HN: x".into(),
            url: None,
            discussion_url: "https://news.ycombinator.com/item?id=42".into(),
            score: 1,
            comments: 0,
            tags: vec![],
            text: Some("body".into()),
        };
        assert_eq!(c.key(), "hn:42");
    }
}
