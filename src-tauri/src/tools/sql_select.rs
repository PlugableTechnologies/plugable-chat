//! SQL Select Implementation
//!
//! Execute SQL queries against configured database sources via Google MCP Database Toolbox.
//! Returns structured query results.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::actors::database_toolbox_actor::DatabaseToolboxMsg;

/// Input for the sql_select built-in tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlSelectInput {
    /// The database source ID to query (optional if only one source is enabled)
    pub source_id: Option<String>,
    /// SQL query to execute
    #[serde(alias = "query")]
    pub sql: String,
    /// Optional query parameters (for parameterized queries)
    #[serde(default)]
    pub parameters: Vec<Value>,
    /// Maximum number of rows to return (default: 100)
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
}

fn default_max_rows() -> usize {
    100
}

/// Output from sql_select
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlSelectOutput {
    /// Whether the query succeeded
    pub success: bool,
    /// Column names from the result
    pub columns: Vec<String>,
    /// Result rows (each row is an array of values)
    pub rows: Vec<Vec<Value>>,
    /// Number of rows returned
    pub row_count: usize,
    /// Total rows affected (for INSERT/UPDATE/DELETE)
    pub rows_affected: Option<usize>,
    /// Error message if query failed
    pub error: Option<String>,
    /// The SQL that was executed
    pub sql_executed: String,
}

/// Executor for the sql_select built-in tool
pub struct SqlSelectExecutor {
    toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>,
}

impl SqlSelectExecutor {
    /// Create a new SQL execution executor
    pub fn new(toolbox_tx: mpsc::Sender<DatabaseToolboxMsg>) -> Self {
        Self { toolbox_tx }
    }

    /// Execute a SQL query
    pub async fn execute(
        &self,
        input: SqlSelectInput,
        enabled_sources: &[String],
    ) -> Result<SqlSelectOutput, String> {
        let source_id = match input.source_id {
            Some(id) if !id.trim().is_empty() => {
                let trimmed_id = id.trim();
                if !enabled_sources.contains(&trimmed_id.to_string()) {
                    return Err(format!(
                        "Database source '{}' is disabled or not found. Enabled sources: {:?}",
                        trimmed_id, enabled_sources
                    ));
                }
                trimmed_id.to_string()
            }
            _ => {
                if enabled_sources.len() == 1 {
                    enabled_sources[0].clone()
                } else if enabled_sources.is_empty() {
                    return Err("No database sources enabled. Please enable a source in Settings > Databases.".to_string());
                } else {
                    return Err(format!(
                        "Multiple database sources enabled ({:?}). Please specify source_id.",
                        enabled_sources
                    ));
                }
            }
        };

        println!(
            "[SqlSelect] Executing on source '{}': {}",
            source_id,
            truncate_sql(&input.sql, 100)
        );

        if input.sql.trim().is_empty() {
            return Err("SQL query cannot be empty".to_string());
        }

        // Apply row limit to SELECT queries
        let limited_sql = apply_row_limit(&input.sql, input.max_rows);

        // Execute via the Database Toolbox Actor
        let (tx, rx) = oneshot::channel();
        self.toolbox_tx
            .send(DatabaseToolboxMsg::ExecuteSql {
                source_id: source_id.clone(),
                sql: limited_sql.clone(),
                parameters: input.parameters.clone(),
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Failed to send execute request: {}", e))?;

        let result = rx
            .await
            .map_err(|_| "Database toolbox actor died".to_string())?;

        match result {
            Ok(exec_result) => {
                println!(
                    "[SqlSelect] Success: {} rows returned",
                    exec_result.row_count
                );

                Ok(SqlSelectOutput {
                    success: true,
                    columns: exec_result.columns,
                    rows: exec_result.rows,
                    row_count: exec_result.row_count,
                    rows_affected: None, // Would need to parse from result for DML
                    error: None,
                    sql_executed: limited_sql,
                })
            }
            Err(e) => {
                println!("[SqlSelect] Error: {}", e);

                Ok(SqlSelectOutput {
                    success: false,
                    columns: vec![],
                    rows: vec![],
                    row_count: 0,
                    rows_affected: None,
                    error: Some(e),
                    sql_executed: limited_sql,
                })
            }
        }
    }
}

/// Truncate SQL for logging
fn truncate_sql(sql: &str, max_len: usize) -> String {
    let normalized: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() > max_len {
        format!("{}...", &normalized[..max_len])
    } else {
        normalized
    }
}

/// Apply a row limit to SELECT queries if not already present
fn apply_row_limit(sql: &str, max_rows: usize) -> String {
    let upper = sql.to_uppercase();
    
    // Only apply to SELECT queries that don't already have LIMIT
    if upper.trim_start().starts_with("SELECT") && !upper.contains("LIMIT") {
        // Handle different SQL dialects
        if upper.contains("OFFSET") {
            // Already has pagination
            sql.to_string()
        } else {
            format!("{} LIMIT {}", sql.trim_end_matches(';').trim(), max_rows)
        }
    } else {
        sql.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_select_input_defaults() {
        let json = r#"{"source_id": "bq-prod", "sql": "SELECT * FROM orders"}"#;
        let input: SqlSelectInput = serde_json::from_str(json).unwrap();

        assert_eq!(input.source_id, Some("bq-prod".to_string()));
        assert_eq!(input.sql, "SELECT * FROM orders");
        assert!(input.parameters.is_empty());
        assert_eq!(input.max_rows, 100);
    }

    #[test]
    fn test_apply_row_limit() {
        // SELECT without LIMIT gets one added
        let sql = "SELECT * FROM orders";
        assert_eq!(
            apply_row_limit(sql, 50),
            "SELECT * FROM orders LIMIT 50"
        );

        // SELECT with existing LIMIT is unchanged
        let sql2 = "SELECT * FROM orders LIMIT 10";
        assert_eq!(apply_row_limit(sql2, 50), sql2);

        // Non-SELECT queries are unchanged
        let sql3 = "INSERT INTO orders VALUES (1, 2)";
        assert_eq!(apply_row_limit(sql3, 50), sql3);

        // Handles trailing semicolon
        let sql4 = "SELECT * FROM orders;";
        assert_eq!(
            apply_row_limit(sql4, 50),
            "SELECT * FROM orders LIMIT 50"
        );
    }

    #[test]
    fn test_sql_select_output_serde() {
        let output = SqlSelectOutput {
            success: true,
            columns: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec![serde_json::json!(1), serde_json::json!("Alice")],
                vec![serde_json::json!(2), serde_json::json!("Bob")],
            ],
            row_count: 2,
            rows_affected: None,
            error: None,
            sql_executed: "SELECT id, name FROM users LIMIT 100".to_string(),
        };

        let json = serde_json::to_string(&output).unwrap();
        let parsed: SqlSelectOutput = serde_json::from_str(&json).unwrap();

        assert!(parsed.success);
        assert_eq!(parsed.row_count, 2);
        assert_eq!(parsed.columns.len(), 2);
    }

    #[test]
    fn test_truncate_sql() {
        let short = "SELECT * FROM orders";
        assert_eq!(truncate_sql(short, 50), short);

        let long = "SELECT id, name, email, address, phone, created_at, updated_at FROM users WHERE active = true AND deleted_at IS NULL";
        let truncated = truncate_sql(long, 50);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() < long.len());
    }
}
