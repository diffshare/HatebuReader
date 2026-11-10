use anyhow::{Context, Result, bail};
use std::time::Duration;
use url::Url;

pub const CATEGORIES: &[(&str, &str)] = &[
    ("総合", ""),
    ("世の中", "social"),
    ("政治と経済", "economics"),
    ("暮らし", "life"),
    ("学び", "knowledge"),
    ("テクノロジー", "it"),
    ("おもしろ", "fun"),
    ("エンタメ", "entertainment"),
    ("アニメとゲーム", "game"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Popular,
    Recent,
}
impl FeedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Popular => "人気",
            Self::Recent => "新着",
        }
    }
    pub fn url(self, category: usize) -> String {
        let section = match self {
            Self::Popular => "hotentry",
            Self::Recent => "entrylist",
        };
        let slug = CATEGORIES[category].1;
        if slug.is_empty() {
            format!("https://b.hatena.ne.jp/{section}.rss")
        } else {
            format!("https://b.hatena.ne.jp/{section}/{slug}.rss")
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub title: String,
    pub url: String,
    pub count: u32,
    pub date: String,
    pub domain: String,
    pub comments_url: String,
}

fn web_url(value: &str) -> Option<Url> {
    Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

pub fn parse_feed(xml: &str) -> Result<Vec<Entry>> {
    let doc = roxmltree::Document::parse(xml).context("RSS の XML を解析できません")?;
    let rss = "http://purl.org/rss/1.0/";
    let hatena = "http://www.hatena.ne.jp/info/xmlns#";
    let dc = "http://purl.org/dc/elements/1.1/";
    if !doc
        .root_element()
        .has_tag_name(("http://www.w3.org/1999/02/22-rdf-syntax-ns#", "RDF"))
    {
        bail!("はてなブックマークの RSS 1.0 形式ではありません");
    }
    let mut entries = Vec::new();
    for item in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((rss, "item")))
    {
        let value = |ns, name| {
            item.children()
                .find(|n| n.has_tag_name((ns, name)))
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let title = value(rss, "title");
        let Some(url) = web_url(&value(rss, "link")) else {
            continue;
        };
        if title.is_empty() {
            continue;
        }
        let comments_url = web_url(&value(hatena, "bookmarkCommentListPageUrl"))
            .filter(|u| u.scheme() == "https" && u.host_str() == Some("b.hatena.ne.jp"))
            .map(|u| u.to_string())
            .unwrap_or_else(|| {
                let mut target = Url::parse("https://b.hatena.ne.jp/entry/").unwrap();
                target.query_pairs_mut().append_pair("url", url.as_str());
                target.to_string()
            });
        entries.push(Entry {
            title,
            domain: url.host_str().unwrap_or_default().to_owned(),
            url: url.to_string(),
            count: value(hatena, "bookmarkcount").parse().unwrap_or(0),
            date: value(dc, "date"),
            comments_url,
        });
    }
    Ok(entries)
}

pub fn fetch_feed(url: &str) -> Result<Vec<Entry>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("hatebu-reader/0.1.0 (RSS reader)")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()?;
    let response = client
        .get(url)
        .send()
        .context("RSS を取得できません。接続を確認してください")?
        .error_for_status()
        .context("RSS サーバーがエラーを返しました")?;
    // 応答サイズを制限し、想定外の巨大な本文をメモリへ読み込まない。
    use std::io::Read;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        bail!("RSS のサイズが上限を超えました");
    }
    parse_feed(std::str::from_utf8(&bytes).context("RSS が UTF-8 ではありません")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(items: &str) -> String {
        format!(
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:h="http://www.hatena.ne.jp/info/xmlns#" xmlns:dc="http://purl.org/dc/elements/1.1/">{items}</rdf:RDF>"#
        )
    }
    #[test]
    fn parses_namespaces_entities_and_comment_link() {
        let entries = parse_feed(&feed(r#"<item><title>日本語 &amp; Rust</title><link>https://example.com/a?x=1&amp;y=2</link><description>概要</description><h:bookmarkcount>42</h:bookmarkcount><dc:date>2026-11-09T00:00:00Z</dc:date><h:bookmarkCommentListPageUrl>https://b.hatena.ne.jp/entry/s/example.com/a</h:bookmarkCommentListPageUrl></item>"#)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "日本語 & Rust");
        assert_eq!(entries[0].count, 42);
        assert_eq!(entries[0].domain, "example.com");
        assert_eq!(
            entries[0].comments_url,
            "https://b.hatena.ne.jp/entry/s/example.com/a"
        );
    }
    #[test]
    fn rejects_non_web_links_and_encodes_comment_fallback() {
        let entries = parse_feed(&feed(r#"<item><title>unsafe</title><link>file:///C:/test</link></item><item><title>ok</title><link>https://example.com/?a=1&amp;b=2#fragment</link><h:bookmarkCommentListPageUrl>javascript:alert(1)</h:bookmarkCommentListPageUrl></item>"#)).unwrap();
        assert_eq!(entries.len(), 1);
        let comments = Url::parse(&entries[0].comments_url).unwrap();
        assert_eq!(comments.query_pairs().next().unwrap().1, entries[0].url);
    }
    #[test]
    fn distinguishes_empty_feed_from_invalid_response() {
        assert!(parse_feed(&feed("")).unwrap().is_empty());
        assert!(parse_feed("<html>error</html>").is_err());
        assert!(parse_feed("<invalid").is_err());
    }
    #[test]
    fn builds_feed_urls() {
        assert_eq!(
            FeedKind::Popular.url(0),
            "https://b.hatena.ne.jp/hotentry.rss"
        );
        assert_eq!(
            FeedKind::Recent.url(5),
            "https://b.hatena.ne.jp/entrylist/it.rss"
        );
    }
}
