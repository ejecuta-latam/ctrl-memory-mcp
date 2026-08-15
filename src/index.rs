use std::ffi::{c_char, c_int};
use std::path::Path;
use std::sync::{Arc, Mutex};

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use regex::Regex;
use rusqlite::ffi::{sqlite3, sqlite3_api_routines, sqlite3_auto_extension};
use rusqlite::{Connection, params};

pub const EMBEDDING_DIM: usize = 384;
const MAX_CHUNK_WORDS: usize = 400;

#[link(name = "sqlite_vec0")]
unsafe extern "C" {
    fn sqlite3_vec_init(
        db: *mut sqlite3,
        err: *mut *mut c_char,
        api: *const sqlite3_api_routines,
    ) -> c_int;
}

fn load_vec_extension() {
    unsafe {
        sqlite3_auto_extension(Some(sqlite3_vec_init));
    }
}

#[derive(Debug)]
pub struct Chunk {
    pub heading: String,
    pub content: String,
}

#[derive(Debug)]
pub struct ChunkDetail {
    pub chunk_id: i64,
    pub path: String,
    pub title: String,
    pub heading: String,
    pub content: String,
}

#[derive(Debug)]
pub struct NoteRow {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub mtime: i64,
    pub size: i64,
    pub hash: String,
}

#[derive(Debug, Clone)]
pub struct Index {
    conn: Arc<Mutex<Connection>>,
}

impl Index {
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        load_vec_extension();
        let conn = Connection::open(db_path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        load_vec_extension();
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                size INTEGER NOT NULL,
                hash TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                links TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY,
                note_id INTEGER NOT NULL,
                heading TEXT NOT NULL,
                content TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(embedding float[384]);
            CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
                chunk_id UNINDEXED, title, heading, content, tokenize='unicode61'
            );",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn get_note(&self, path: &str) -> anyhow::Result<Option<NoteRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, path, title, mtime, size, hash FROM notes WHERE path = ?1",
        )?;
        let mut rows = stmt.query_map([path], |r| {
            Ok(NoteRow {
                id: r.get(0)?,
                path: r.get(1)?,
                title: r.get(2)?,
                mtime: r.get(3)?,
                size: r.get(4)?,
                hash: r.get(5)?,
            })
        })?;
        rows.next().transpose().map_err(Into::into)
    }

    pub fn upsert_note(
        &self,
        path: &str,
        title: &str,
        mtime: i64,
        size: i64,
        hash: &str,
        tags: &[String],
        links: &[String],
    ) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let id = conn.query_row(
            "INSERT INTO notes (path, title, mtime, size, hash, tags, links)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                title = excluded.title, mtime = excluded.mtime, size = excluded.size,
                hash = excluded.hash, tags = excluded.tags, links = excluded.links
             RETURNING id",
            params![path, title, mtime, size, hash, tags.join(","), links.join(",")],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn delete_note(&self, path: &str) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let chunk_ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT c.id FROM chunks c JOIN notes n ON n.id = c.note_id WHERE n.path = ?1",
            )?;
            let rows = stmt.query_map([path], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for id in &chunk_ids {
            tx.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [id])?;
            tx.execute("DELETE FROM fts_chunks WHERE chunk_id = ?1", [id])?;
        }
        tx.execute("DELETE FROM chunks WHERE note_id IN (SELECT id FROM notes WHERE path = ?1)", [path])?;
        tx.execute("DELETE FROM notes WHERE path = ?1", [path])?;
        tx.commit()?;
        Ok(())
    }

    pub fn replace_note_chunks(
        &self,
        note_id: i64,
        title: &str,
        chunks: &[Chunk],
        embeddings: &[Vec<f32>],
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let old_ids: Vec<i64> = {
                let mut stmt = tx.prepare("SELECT id FROM chunks WHERE note_id = ?1")?;
                let rows = stmt.query_map([note_id], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            for id in &old_ids {
                tx.execute("DELETE FROM vec_chunks WHERE rowid = ?1", [id])?;
                tx.execute("DELETE FROM fts_chunks WHERE chunk_id = ?1", [id])?;
            }
            tx.execute("DELETE FROM chunks WHERE note_id = ?1", [note_id])?;
        }
        for (chunk, embedding) in chunks.iter().zip(embeddings) {
            tx.execute(
                "INSERT INTO chunks (note_id, heading, content) VALUES (?1, ?2, ?3)",
                params![note_id, chunk.heading, chunk.content],
            )?;
            let chunk_id = tx.last_insert_rowid();
            let vector = serde_json::to_string(embedding)?;
            tx.execute(
                "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, vector],
            )?;
            tx.execute(
                "INSERT INTO fts_chunks (chunk_id, title, heading, content) VALUES (?1, ?2, ?3, ?4)",
                params![chunk_id, title, chunk.heading, chunk.content],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn wipe(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM vec_chunks; DELETE FROM fts_chunks; DELETE FROM chunks; DELETE FROM notes;",
        )?;
        Ok(())
    }

    pub fn note_count(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?)
    }

    pub fn keyword_search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<(i64, f64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT chunk_id, bm25(fts_chunks) FROM fts_chunks
             WHERE fts_chunks MATCH ?1 ORDER BY bm25(fts_chunks) LIMIT ?2",
        )?;
        let mut results = Vec::new();
        let mut rows = stmt.query_map(params![fts_query(query), limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        while let Some(row) = rows.next() {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn vector_search(
        &self,
        vector: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<(i64, f64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT rowid, distance FROM vec_chunks
             WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
        )?;
        let query = serde_json::to_string(vector)?;
        let mut results = Vec::new();
        let mut rows = stmt.query_map(params![query, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        while let Some(row) = rows.next() {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn chunk_details(&self, chunk_ids: &[i64]) -> anyhow::Result<Vec<ChunkDetail>> {
        let conn = self.conn.lock().unwrap();
        let placeholders = vec!["?"; chunk_ids.len()].join(",");
        let sql = format!(
            "SELECT c.id, n.path, n.title, c.heading, c.content
             FROM chunks c JOIN notes n ON n.id = c.note_id
             WHERE c.id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<rusqlite::types::Value> = chunk_ids
            .iter()
            .map(|id| rusqlite::types::Value::Integer(*id))
            .collect();
        let mut details = Vec::new();
        let mut rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok(ChunkDetail {
                chunk_id: r.get(0)?,
                path: r.get(1)?,
                title: r.get(2)?,
                heading: r.get(3)?,
                content: r.get(4)?,
            })
        })?;
        while let Some(row) = rows.next() {
            details.push(row?);
        }
        Ok(details)
    }
}

pub fn fts_query(query: &str) -> String {
    let mut terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.trim_matches('"'))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if let Some(last) = terms.last_mut() {
        last.insert(last.len() - 1, '*');
    }
    terms.join(" ")
}

pub fn chunk_note(body: &str) -> Vec<Chunk> {    let mut chunks = Vec::new();
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut buffer = String::new();
    let mut in_heading = false;

    for event in Parser::new(body) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let rank = heading_level_rank(level);
                if rank > 1 {
                    flush_buffer(&mut chunks, &headings, &mut buffer);
                    while headings.last().is_some_and(|(l, _)| *l >= rank) {
                        headings.pop();
                    }
                    headings.push((rank, String::new()));
                    in_heading = true;
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
            }
            Event::Text(text) => {
                let text = clean_wikilinks(&text);
                if in_heading {
                    if let Some((_, last)) = headings.last_mut() {
                        last.push_str(&text);
                    }
                } else {
                    buffer.push_str(&text);
                }
            }
            Event::Code(code) => {
                if !in_heading {
                    buffer.push_str(&code);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if !in_heading {
                    buffer.push(' ');
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !in_heading {
                    buffer.push_str("\n\n");
                }
            }
            _ => {}
        }
    }
    flush_buffer(&mut chunks, &headings, &mut buffer);
    chunks
}

fn flush_buffer(chunks: &mut Vec<Chunk>, headings: &[(usize, String)], buffer: &mut String) {
    let content = buffer.trim().to_string();
    let heading = headings
        .iter()
        .map(|(_, h)| h.trim())
        .filter(|h| !h.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    let heading = if heading.is_empty() {
        "Overview"
    } else {
        &heading
    };
    if !content.is_empty() {
        if content.split_whitespace().count() > MAX_CHUNK_WORDS {
            for piece in split_paragraphs(&content, MAX_CHUNK_WORDS) {
                chunks.push(Chunk {
                    heading: heading.to_string(),
                    content: piece,
                });
            }
        } else {
            chunks.push(Chunk {
                heading: heading.to_string(),
                content,
            });
        }
    }
    buffer.clear();
}

fn split_paragraphs(content: &str, max_words: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for paragraph in content.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if !current.is_empty() && current.split_whitespace().count() + paragraph.split_whitespace().count() > max_words
        {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

fn clean_wikilinks(text: &str) -> String {
    let re = Regex::new(r"!?\[\[([^\[\]|#]+)(?:#[^\]|]*)?(?:\|([^\[\]]+))?\]\]").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        caps.get(2)
            .or_else(|| caps.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default()
    })
    .into_owned()
}

fn heading_level_rank(level: pulldown_cmark::HeadingLevel) -> usize {
    match level {
        pulldown_cmark::HeadingLevel::H1 => 1,
        pulldown_cmark::HeadingLevel::H2 => 2,
        pulldown_cmark::HeadingLevel::H3 => 3,
        pulldown_cmark::HeadingLevel::H4 => 4,
        pulldown_cmark::HeadingLevel::H5 => 5,
        pulldown_cmark::HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_note() -> String {
        r#"# Rust Notes

Intro paragraph about rust and memory safety.

## Borrow Checker

The borrow checker enforces ownership rules at compile time.

## Lifetimes

Lifetimes describe scopes for references. They help the borrow checker.

### Explicit Annotations

Sometimes the compiler needs help with lifetime annotations.
"#
        .to_string()
    }

    #[test]
    fn chunks_by_heading() {
        let chunks = chunk_note(&sample_note());
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].heading, "Overview");
        assert!(chunks[0].content.contains("Intro paragraph"));
        assert_eq!(chunks[1].heading, "Borrow Checker");
        assert_eq!(chunks[3].heading, "Lifetimes / Explicit Annotations");
    }

    #[test]
    fn splits_oversized_chunks_by_paragraph() {
        let mut body = String::from("## Long\n\n");
        for i in 0..120 {
            body.push_str(&format!("Paragraph {i} with some words. "));
            body.push_str("\n\n");
        }
        let chunks = chunk_note(&body);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.content.split_whitespace().count() <= MAX_CHUNK_WORDS));
    }

    #[test]
    fn cleans_wikilinks_for_embedding() {
        assert_eq!(clean_wikilinks("see [[Target]] now"), "see Target now");
        assert_eq!(clean_wikilinks("see [[Target|alias]] now"), "see alias now");
        assert_eq!(clean_wikilinks("see ![[Target]] now"), "see Target now");
    }

    #[test]
    fn fts_query_quotes_terms_and_wildcards_last() {
        assert_eq!(fts_query("borrow checker"), "\"borrow\" \"checker*\"");
        assert_eq!(fts_query("  messy   input "), "\"messy\" \"input*\"");
    }

    fn dummy_vec(text: &str) -> Vec<f32> {
        text.chars()
            .cycle()
            .take(EMBEDDING_DIM)
            .map(|ch| ch as u32 as f32 / 1000.0)
            .collect()
    }

    #[test]
    fn crud_and_search_flow() {
        let index = Index::open_in_memory().unwrap();
        let id = index
            .upsert_note("a.md", "A", 1, 10, "h1", &["rust".to_string()], &["B".to_string()])
            .unwrap();
        assert_eq!(index.note_count().unwrap(), 1);

        let chunks = chunk_note(&sample_note());
        let embeddings: Vec<Vec<f32>> = chunks.iter().map(|c| dummy_vec(&c.content)).collect();
        index
            .replace_note_chunks(id, "Rust Notes", &chunks, &embeddings)
            .unwrap();

        let hits = index.keyword_search("borrow checker", 5).unwrap();
        assert!(!hits.is_empty());
        let (chunk_id, _) = hits[0];
        let details = index.chunk_details(&[chunk_id]).unwrap();
        assert_eq!(details[0].path, "a.md");
        assert_eq!(details[0].heading, "Borrow Checker");

        let query_vec = dummy_vec("borrow checker lifetimes");
        let hits = index.vector_search(&query_vec, 5).unwrap();
        assert_eq!(hits.len(), 4);

        index.delete_note("a.md").unwrap();
        assert_eq!(index.note_count().unwrap(), 0);
        assert!(index.keyword_search("borrow", 5).unwrap().is_empty());
    }
}
