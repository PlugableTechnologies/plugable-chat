//! Keyword (lexical) search used when the embedding model is not ready.
//!
//! Chat search and schema/column search normally embed the query with the CPU model. When that
//! model is missing (first download, a blocked network, a failed start-up) those searches used
//! to return nothing useful. This module ranks the same rows by words instead, so the user
//! still gets answers, and the UI says it is "Keyword search only".
//!
//! Ranking uses SQLite FTS5 (`bm25`) over an in-memory table built per search. The bundled
//! SQLite (rusqlite `bundled`) compiles FTS5 in; if a build ever lacks it, `rank_by_keywords`
//! falls back to a LIKE-style word-count score so search never errors out.

use rusqlite::{params, Connection};

/// Text the UI shows next to results that came from the keyword fallback.
pub const KEYWORD_ONLY_LABEL: &str = "Keyword search only (search model unavailable)";

/// Lowercase alphanumeric words of `text`; `_`, `-`, `.` and spaces all separate words, so
/// `chicago_crimes` matches a search for "crimes".
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Rank `docs` against `query`. Returns `(index into docs, score in (0, 1])`, best first, only
/// documents that match at least one query word, at most `limit` of them.
pub fn rank_by_keywords(docs: &[String], query: &str, limit: usize) -> Vec<(usize, f32)> {
    let words = tokenize(query);
    if words.is_empty() || docs.is_empty() || limit == 0 {
        return Vec::new();
    }
    match rank_with_fts5(docs, &words, limit) {
        Ok(ranked) => ranked,
        Err(e) => {
            println!("[KeywordSearch] FTS5 unavailable ({e}); using word-count ranking");
            rank_by_word_count(docs, &words, limit)
        }
    }
}

fn rank_with_fts5(docs: &[String], words: &[String], limit: usize) -> rusqlite::Result<Vec<(usize, f32)>> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("CREATE VIRTUAL TABLE docs USING fts5(body, tokenize = 'unicode61');")?;
    {
        let mut insert = conn.prepare("INSERT INTO docs(rowid, body) VALUES (?1, ?2)")?;
        for (i, doc) in docs.iter().enumerate() {
            insert.execute(params![i as i64, doc])?;
        }
    }
    // Words are alphanumeric only (see `tokenize`), so quoting them cannot break the MATCH syntax.
    let match_expr = words
        .iter()
        .map(|w| format!("\"{w}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut stmt = conn.prepare("SELECT rowid, bm25(docs) FROM docs WHERE docs MATCH ?1 ORDER BY bm25(docs) LIMIT ?2")?;
    let rows = stmt.query_map(params![match_expr, limit as i64], |row| {
        let rowid: i64 = row.get(0)?;
        let bm25: f64 = row.get(1)?;
        Ok((rowid as usize, bm25_to_score(bm25)))
    })?;
    rows.collect()
}

/// bm25 is negative, more negative = better; map to (0, 1] where higher is better.
fn bm25_to_score(bm25: f64) -> f32 {
    let strength = (-bm25).max(0.0);
    ((strength + 0.01) / (strength + 1.0)) as f32
}

/// LIKE-style fallback: score = share of query words found as a prefix of some document word.
fn rank_by_word_count(docs: &[String], words: &[String], limit: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = docs
        .iter()
        .enumerate()
        .filter_map(|(i, doc)| {
            let doc_words = tokenize(doc);
            let hits = words.iter().filter(|w| doc_words.iter().any(|d| d.starts_with(w.as_str()))).count();
            (hits > 0).then(|| (i, hits as f32 / words.len() as f32))
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fts5_is_compiled_into_the_bundled_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(body);").expect("FTS5 must be available");
    }

    #[test]
    fn tokenize_splits_on_underscores_and_lowercases() {
        assert_eq!(tokenize("Chicago_Crimes by-year"), vec!["chicago", "crimes", "by", "year"]);
    }

    #[test]
    fn ranks_the_matching_document_first() {
        let d = docs(&["pasta recipes for dinner", "arrests by district in chicago", "weather today"]);
        let r = rank_by_keywords(&d, "chicago arrests", 10);
        assert_eq!(r[0].0, 1);
        assert!(r[0].1 > 0.0 && r[0].1 <= 1.0);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn prefix_and_case_insensitive() {
        let d = docs(&["Arrests per District", "unrelated"]);
        assert_eq!(rank_by_keywords(&d, "arre", 5)[0].0, 0);
    }

    #[test]
    fn empty_or_symbol_only_query_returns_nothing() {
        let d = docs(&["anything"]);
        assert!(rank_by_keywords(&d, "   ", 5).is_empty());
        assert!(rank_by_keywords(&d, "\"*() -", 5).is_empty());
    }

    #[test]
    fn query_with_fts_syntax_characters_does_not_error() {
        let d = docs(&["it's a test: of NEAR AND OR"]);
        let r = rank_by_keywords(&d, "test\" OR NEAR(", 5);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn limit_is_respected() {
        let d = docs(&["a cat", "a cat sat", "cat cat cat"]);
        assert_eq!(rank_by_keywords(&d, "cat", 2).len(), 2);
    }

    #[test]
    fn word_count_fallback_ranks_by_share_of_words() {
        let d = docs(&["red car", "red", "blue"]);
        let words = tokenize("red car");
        let r = rank_by_word_count(&d, &words, 10);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(r[0].1, 1.0);
    }
}
