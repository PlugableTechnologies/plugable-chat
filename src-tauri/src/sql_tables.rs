//! Finding and repairing table names in model-written SQL.
//!
//! Small models sometimes emit identifiers with stray spaces (`main.chicago_ crimes`, seen in the
//! rc14 Chicago test). A whitespace split read that as the table `main.chicago_`, the lookup failed
//! with "Table 'main.chicago_' not found", and the model looped on its own typo. This module reads
//! such spans as one name, matches them to the tables that really exist, and rewrites the SQL.

/// A table reference as written in the SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// Cleaned name, e.g. `main.chicago_crimes` (spaces and quotes removed).
    pub name: String,
    /// Exactly the text in the SQL that this reference came from.
    pub raw: String,
}

const STOP_WORDS: [&str; 10] = [
    "SELECT", "WHERE", "ON", "AS", "LEFT", "RIGHT", "INNER", "OUTER", "CROSS", "NATURAL",
];

fn is_glue(t: &str) -> bool {
    t.ends_with('_') || t.ends_with('.')
}

fn starts_glue(t: &str) -> bool {
    t.starts_with('_') || t.starts_with('.')
}

fn clean_identifier(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '`' | '[' | ']' | '\''))
        .collect()
}

/// Table references after FROM / JOIN. A name split by whitespace around `_` or `.` is rejoined;
/// an ordinary alias (`FROM trips t`) is not.
pub fn extract_table_refs(sql: &str) -> Vec<TableRef> {
    // Tokens with byte offsets, so `raw` can be sliced from the original text.
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in sql.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                tokens.push((s, &sql[s..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        tokens.push((s, &sql[s..]));
    }

    let mut refs: Vec<TableRef> = Vec::new();
    for (i, (_, tok)) in tokens.iter().enumerate() {
        let up = tok.to_uppercase();
        if !(up == "FROM" || up == "JOIN") || i + 1 >= tokens.len() {
            continue;
        }
        let first = i + 1;
        let mut last = first;
        while last + 1 < tokens.len() && (is_glue(tokens[last].1) || starts_glue(tokens[last + 1].1)) {
            // Never glue across a clause boundary such as "FROM a. WHERE".
            if STOP_WORDS.contains(&tokens[last + 1].1.to_uppercase().as_str()) {
                break;
            }
            last += 1;
        }
        let (first_off, _) = tokens[first];
        let (last_off, last_tok) = tokens[last];
        let mut raw = sql[first_off..last_off + last_tok.len()].to_string();
        // Wrapping punctuation is not part of the name.
        let lead = raw.len() - raw.trim_start_matches('(').len();
        raw = raw[lead..].trim_end_matches(|c| c == ')' || c == ',' || c == ';').to_string();
        let name = clean_identifier(&raw);
        if name.is_empty() || STOP_WORDS.contains(&name.to_uppercase().as_str()) {
            continue; // a subquery or a keyword, not a table
        }
        refs.push(TableRef { name, raw });
    }
    let mut seen = std::collections::HashSet::new();
    refs.retain(|r| seen.insert(r.name.clone()));
    refs
}

/// Names only, in order (the shape the lookup and error JSON use).
pub fn extract_table_names(sql: &str) -> Vec<String> {
    extract_table_refs(sql).into_iter().map(|r| r.name).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The name is a known table, or the unqualified end of exactly one.
    Exact(String),
    /// Not a table, but one known table is a close spelling; the SQL should be rewritten.
    Corrected(String),
    Ambiguous(Vec<String>),
    NotFound,
}

fn last_part(s: &str) -> &str {
    s.rsplit('.').next().unwrap_or(s)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Matches a name from SQL to the known fully qualified table names.
pub fn resolve_table_name(name: &str, known_fqns: &[String]) -> Resolution {
    let lower = name.to_lowercase();
    let target_last = last_part(&lower).to_string();

    let exact: Vec<&String> = known_fqns.iter().filter(|f| f.to_lowercase() == lower).collect();
    if exact.len() == 1 {
        return Resolution::Exact(exact[0].clone());
    }
    let partial: Vec<&String> = known_fqns
        .iter()
        .filter(|f| last_part(&f.to_lowercase()) == target_last)
        .collect();
    match partial.len() {
        1 => return Resolution::Exact(partial[0].clone()),
        n if n > 1 => return Resolution::Ambiguous(partial.into_iter().cloned().collect()),
        _ => {}
    }
    // A name cut short (`chicago_`) that begins exactly one real table; short names are too easy to confuse.
    if target_last.chars().count() >= 5 {
        let by_prefix: Vec<&String> = known_fqns
            .iter()
            .filter(|f| last_part(&f.to_lowercase()).starts_with(&target_last))
            .collect();
        if by_prefix.len() == 1 {
            return Resolution::Corrected(by_prefix[0].clone());
        }
    }
    // Close spelling of the table part only.
    if target_last.chars().count() >= 5 {
        let mut scored: Vec<(usize, &String)> = known_fqns
            .iter()
            .map(|f| (edit_distance(&target_last, last_part(&f.to_lowercase())), f))
            .filter(|(d, _)| *d <= 2)
            .collect();
        scored.sort_by_key(|(d, _)| *d);
        if let Some((best, f)) = scored.first() {
            let tied = scored.iter().filter(|(d, _)| d == best).count();
            if tied == 1 {
                return Resolution::Corrected((*f).clone());
            }
        }
    }
    Resolution::NotFound
}

/// Rewrites the first occurrence of the written reference to the real table name.
pub fn rewrite_table(sql: &str, table: &TableRef, fq_name: &str) -> String {
    sql.replacen(&table.raw, fq_name, 1)
}

/// The SQL to run once the lookup resolved `table` to `fq_name`: untouched when it was written
/// correctly, tidied when only spaces or quotes were wrong, and rewritten to the real name when the
/// model misspelled it.
pub fn repair_sql(sql: &str, table: &TableRef, fq_name: &str) -> String {
    let written = table.name.to_lowercase();
    let fq = fq_name.to_lowercase();
    let same_table = fq == written || last_part(&fq) == last_part(&written);
    if same_table && table.raw == table.name {
        sql.to_string()
    } else if same_table {
        rewrite_table(sql, table, &table.name)
    } else {
        rewrite_table(sql, table, fq_name)
    }
}

/// The error the model sees: it names the tables that exist so it can correct itself even when
/// the schema is not re-injected (native tool results carry no schema context).
pub fn table_not_found_message(name: &str, known_fqns: &[String]) -> String {
    const SHOWN: usize = 30;
    let mut tables: Vec<&String> = known_fqns.iter().collect();
    tables.sort();
    let list = tables.iter().take(SHOWN).map(|t| t.as_str()).collect::<Vec<_>>().join(", ");
    let more = if tables.len() > SHOWN { format!(" (+{} more)", tables.len() - SHOWN) } else { String::new() };
    if tables.is_empty() {
        format!("Table '{name}' not found: no tables are available in the enabled database sources.")
    } else {
        format!("Table '{name}' not found. Available tables: {list}{more}. Use one of these names exactly.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["main.chicago_crimes".to_string()]
    }

    #[test]
    fn plain_and_qualified_names() {
        assert_eq!(extract_table_names("SELECT * FROM main.chicago_crimes LIMIT 5"), vec!["main.chicago_crimes"]);
        assert_eq!(extract_table_names("select count(*) from chicago_crimes;"), vec!["chicago_crimes"]);
    }

    #[test]
    fn the_rc14_garbled_name_is_read_as_one_table() {
        let refs = extract_table_refs("SELECT type FROM main.chicago_ crimes WHERE year = 2025");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "main.chicago_crimes");
        assert_eq!(refs[0].raw, "main.chicago_ crimes");
    }

    #[test]
    fn spaces_around_the_dot_and_quoting() {
        assert_eq!(extract_table_names("SELECT 1 FROM main. chicago_crimes")[0], "main.chicago_crimes");
        assert_eq!(extract_table_names("SELECT 1 FROM main .chicago_crimes")[0], "main.chicago_crimes");
        assert_eq!(extract_table_names("SELECT 1 FROM \"main\".\"chicago_crimes\"")[0], "main.chicago_crimes");
        assert_eq!(extract_table_names("SELECT 1 FROM `main.chicago_crimes`")[0], "main.chicago_crimes");
    }

    #[test]
    fn an_alias_is_not_glued_on() {
        let refs = extract_table_refs("SELECT t.a FROM chicago_crimes t JOIN other_table o ON t.id = o.id");
        let names: Vec<_> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["chicago_crimes", "other_table"]);
    }

    #[test]
    fn joins_commas_subqueries_and_duplicates() {
        let n = extract_table_names("SELECT * FROM (SELECT 1) x JOIN a ON 1=1 JOIN a ON 1=1");
        assert_eq!(n, vec!["a"]);
        assert!(extract_table_names("SELECT 1").is_empty());
        assert!(extract_table_names("SELECT * FROM").is_empty());
    }

    #[test]
    fn a_trailing_dot_does_not_swallow_the_next_clause() {
        let refs = extract_table_refs("SELECT * FROM main. WHERE x = 1");
        assert!(refs.iter().all(|r| !r.name.to_uppercase().contains("WHERE")), "{refs:?}");
    }

    #[test]
    fn resolve_exact_partial_and_case() {
        assert_eq!(resolve_table_name("main.chicago_crimes", &known()), Resolution::Exact("main.chicago_crimes".into()));
        assert_eq!(resolve_table_name("Chicago_Crimes", &known()), Resolution::Exact("main.chicago_crimes".into()));
    }

    #[test]
    fn resolve_corrects_a_near_miss_but_not_a_stranger() {
        assert_eq!(resolve_table_name("main.chicago_crime", &known()), Resolution::Corrected("main.chicago_crimes".into()));
        assert_eq!(resolve_table_name("main.chicago_", &known()), Resolution::Corrected("main.chicago_crimes".into()));
        assert_eq!(resolve_table_name("customers", &known()), Resolution::NotFound);
    }

    #[test]
    fn ambiguity_and_ties_are_never_guessed() {
        let two = vec!["a.trips".to_string(), "b.trips".to_string()];
        assert!(matches!(resolve_table_name("trips", &two), Resolution::Ambiguous(v) if v.len() == 2));
        let tie = vec!["main.orders_a".to_string(), "main.orders_b".to_string()];
        assert_eq!(resolve_table_name("orders_c", &tie), Resolution::NotFound);
    }

    #[test]
    fn short_names_are_not_fuzzy_matched() {
        assert_eq!(resolve_table_name("tab", &["main.tag".to_string()]), Resolution::NotFound);
    }

    #[test]
    fn rewrite_replaces_the_written_span() {
        let sql = "SELECT type FROM main.chicago_ crimes WHERE year = 2025";
        let r = &extract_table_refs(sql)[0];
        assert_eq!(
            rewrite_table(sql, r, "main.chicago_crimes"),
            "SELECT type FROM main.chicago_crimes WHERE year = 2025"
        );
    }

    #[test]
    fn the_not_found_message_lists_the_real_tables() {
        let m = table_not_found_message("main.chicago_", &known());
        assert!(m.contains("main.chicago_crimes") && m.contains("not found"), "{m}");
        assert!(table_not_found_message("x", &[]).contains("no tables"));
        let many: Vec<String> = (0..40).map(|i| format!("t{i:02}")).collect();
        assert!(table_not_found_message("x", &many).contains("+10 more"));
    }

    #[test]
    fn repair_leaves_correct_sql_alone_and_fixes_the_rest() {
        let ok = "SELECT 1 FROM chicago_crimes";
        assert_eq!(repair_sql(ok, &extract_table_refs(ok)[0], "main.chicago_crimes"), ok);

        let quoted = "SELECT 1 FROM \"chicago_crimes\" WHERE x=1";
        assert_eq!(
            repair_sql(quoted, &extract_table_refs(quoted)[0], "main.chicago_crimes"),
            "SELECT 1 FROM chicago_crimes WHERE x=1",
            "same table, tidied"
        );

        let garbled = "SELECT type, COUNT(*) FROM main.chicago_ crimes WHERE year = 2025 GROUP BY type";
        let fixed = repair_sql(garbled, &extract_table_refs(garbled)[0], "main.chicago_crimes");
        assert_eq!(fixed, "SELECT type, COUNT(*) FROM main.chicago_crimes WHERE year = 2025 GROUP BY type");
        // and what we run now parses back to the real table
        assert_eq!(extract_table_names(&fixed), vec!["main.chicago_crimes"]);
    }

    #[test]
    fn the_whole_rc14_flow_from_garbled_sql_to_runnable_sql() {
        let sql = "SELECT primary_type FROM main.chicago_ crimes LIMIT 5";
        let r = &extract_table_refs(sql)[0];
        let fq = match resolve_table_name(&r.name, &known()) {
            Resolution::Exact(f) | Resolution::Corrected(f) => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(repair_sql(sql, r, &fq), "SELECT primary_type FROM main.chicago_crimes LIMIT 5");
    }
}
