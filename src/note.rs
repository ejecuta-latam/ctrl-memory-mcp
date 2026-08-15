use std::path::Path;

use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Frontmatter {
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub path: String,
    pub title: String,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub raw: String,
}

impl Note {
    pub fn parse(path: &str, raw: &str) -> Self {
        let (frontmatter, body) = split_frontmatter(raw);
        let title = frontmatter
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_stem(path).to_string());
        Note {
            path: path.to_string(),
            title,
            frontmatter,
            body: body.to_string(),
            raw: raw.to_string(),
        }
    }

    pub fn tags(&self) -> Vec<String> {
        let mut tags = self.frontmatter.tags.clone();
        tags.extend(inline_tags(&self.body));
        tags.sort();
        tags.dedup();
        tags
    }

    pub fn wikilinks(&self) -> Vec<String> {
        let mut links = Vec::new();
        let re = Regex::new(r"!?\[\[([^\[\]|#]+)(?:#[^\]|]*)?(?:\|[^\[\]]+)?\]\]").unwrap();
        for cap in re.captures_iter(&self.body) {
            links.push(cap[1].trim().to_string());
        }
        links.sort();
        links.dedup();
        links
    }
}

fn split_frontmatter(raw: &str) -> (Frontmatter, &str) {
    let Some(rest) = raw.strip_prefix("---\n").or_else(|| raw.strip_prefix("---\r\n")) else {
        return (Frontmatter::default(), raw);
    };
    let Some(end) = rest.find("\n---") else {
        return (Frontmatter::default(), raw);
    };
    let yaml = &rest[..end];
    let body = &rest[end + 4..];
    let frontmatter = serde_yaml::from_str(yaml).unwrap_or_default();
    (frontmatter, body.trim_start())
}

fn file_stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

fn inline_tags(body: &str) -> Vec<String> {
    let re = Regex::new(r"#[\p{L}\p{N}_\-/]+").unwrap();
    let mut tags = Vec::new();
    for m in re.find_iter(body) {
        let preceded_by_word = body[..m.start()]
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '#' || c == '/');
        if !preceded_by_word {
            tags.push(m.as_str().trim_start_matches('#').to_string());
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let note = Note::parse(
            "daily/2026-01-01.md",
            "---\ntitle: New Year\ntags: [daily, personal]\naliases: [NYD]\n---\n# New Year\n\nFirst body line.\n",
        );
        assert_eq!(note.title, "New Year");
        assert_eq!(note.body, "# New Year\n\nFirst body line.\n");
        assert_eq!(note.frontmatter.aliases, vec!["NYD"]);
    }

    #[test]
    fn title_falls_back_to_filename() {
        let note = Note::parse("ideas/plan.md", "# Plan\n\ncontent");
        assert_eq!(note.title, "plan");
    }

    #[test]
    fn extracts_inline_tags_but_not_headings() {
        let note = Note::parse("x.md", "# Title\n\nA note about #rust and #dev/async.\n## Another #section heading\n");
        assert_eq!(note.tags(), vec!["dev/async", "rust", "section"]);
    }

    #[test]
    fn extracts_wikilinks_with_aliases() {
        let note = Note::parse("x.md", "See [[Other Note]] and [[Third|alias here]] and ![[Embedded]].");
        assert_eq!(note.wikilinks(), vec!["Embedded", "Other Note", "Third"]);
    }
}
