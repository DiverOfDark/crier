//! Telegram HTML rendering. Everything model- or user-supplied is escaped;
//! only tags produced here reach Telegram.

use crate::article::truncate_chars;
use crate::llm::Post;

/// Telegram's limit for a text message.
const MAX_MESSAGE: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct Discussion {
    pub url: String,
    pub score: i64,
    pub comments: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Links {
    pub url: Option<String>,
    pub hn: Option<Discussion>,
    pub lobsters: Option<Discussion>,
}

/// Post used when the LLM keeps failing: the title and the links still go out.
pub fn fallback_post(title: &str) -> Post {
    Post {
        headline: title.to_string(),
        body: String::new(),
        discussion: None,
        hashtags: vec![],
    }
}

pub fn render(post: &Post, links: &Links) -> String {
    let mut lines = vec![format!("<b>{}</b>", esc(&post.headline))];
    let mut text = String::new();
    if !post.body.is_empty() {
        text.push_str(&esc(&post.body));
    }
    if let Some(d) = &post.discussion {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("💬 {}", esc(d)));
    }
    let mut footer = Vec::new();
    if let Some(u) = &links.url {
        footer.push(format!("<a href=\"{}\">{}</a>", esc(u), esc(&domain(u))));
    }
    for (name, d) in [("HN", &links.hn), ("Lobsters", &links.lobsters)] {
        if let Some(d) = d {
            footer.push(format!(
                "<a href=\"{}\">{name}</a> {}▲ {}💬",
                esc(&d.url),
                d.score,
                d.comments
            ));
        }
    }
    let mut tail = footer.join(" · ");
    if !post.hashtags.is_empty() {
        let tags: Vec<String> = post
            .hashtags
            .iter()
            .map(|h| format!("#{}", esc(h)))
            .collect();
        tail.push('\n');
        tail.push_str(&tags.join(" "));
    }

    // Trim the prose, never the markup, if the message would be too long.
    let fixed = lines[0].chars().count() + tail.chars().count() + 8;
    if fixed + text.chars().count() > MAX_MESSAGE {
        let budget = MAX_MESSAGE.saturating_sub(fixed + 1);
        text = truncate_chars(&text, budget);
        // An entity cut in half (`&am…`) would break Telegram's HTML parser.
        if let Some(amp) = text.rfind('&') {
            if !text[amp..].contains(';') {
                text.truncate(amp);
                text.push('…');
            }
        }
    }
    if !text.is_empty() {
        lines.push(text);
    }
    lines.push(tail);
    lines.join("\n\n")
}

fn domain(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| "link".to_string())
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links() -> Links {
        Links {
            url: Some("https://www.example.com/a?b=1&c=2".into()),
            hn: Some(Discussion {
                url: "https://news.ycombinator.com/item?id=1".into(),
                score: 512,
                comments: 200,
            }),
            lobsters: None,
        }
    }

    #[test]
    fn renders_full_post() {
        let p = Post {
            headline: "Rust <3 & you".into(),
            body: "It is fast.".into(),
            discussion: Some("People disagree.".into()),
            hashtags: vec!["rust".into(), "perf".into()],
        };
        assert_eq!(
            render(&p, &links()),
            "<b>Rust &lt;3 &amp; you</b>\n\nIt is fast.\n\n💬 People disagree.\n\n\
             <a href=\"https://www.example.com/a?b=1&amp;c=2\">example.com</a> · \
             <a href=\"https://news.ycombinator.com/item?id=1\">HN</a> 512▲ 200💬\n#rust #perf"
        );
    }

    #[test]
    fn fallback_has_title_and_links_only() {
        let out = render(
            &fallback_post("Title"),
            &Links {
                url: None,
                ..links()
            },
        );
        assert_eq!(
            out,
            "<b>Title</b>\n\n<a href=\"https://news.ycombinator.com/item?id=1\">HN</a> 512▲ 200💬"
        );
    }

    #[test]
    fn long_bodies_fit_and_keep_entities_whole() {
        let p = Post {
            headline: "H".into(),
            body: "a&".repeat(5000),
            discussion: None,
            hashtags: vec![],
        };
        let out = render(&p, &links());
        assert!(out.chars().count() <= MAX_MESSAGE);
        let body = out.split("\n\n").nth(1).unwrap();
        assert!(
            body.ends_with("&amp;…") || body.ends_with("a…"),
            "{}",
            &body[body.len() - 20..]
        );
    }
}
