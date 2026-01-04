//! Embedded SQLite Actor - manages the built-in demo database.
//!
//! This actor handles:
//! - Creating and managing the demo.db SQLite database in test-data/
//! - Loading the Chicago Crimes dataset from CSV on first access
//! - Executing SQL queries against the embedded database
//! - Providing schema information for the demo tables

use crate::actors::database_toolbox_actor::SqlExecutionResult;
use crate::demo_schema::{
    chicago_crimes_table_schema, CHICAGO_CRIMES_CREATE_TABLE, CHICAGO_CRIMES_TABLE_FQ_NAME,
    DEMO_SCHEMA_VERSION, EMBEDDED_DEMO_SOURCE_ID,
};
use crate::settings::CachedTableSchema;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

/// Messages for the Embedded SQLite Actor
#[derive(Debug)]
pub enum EmbeddedSqliteMsg {
    /// Ensure the database is initialized (creates tables, loads data if needed)
    EnsureInitialized {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Execute a SQL query
    ExecuteSql {
        sql: String,
        respond_to: oneshot::Sender<Result<SqlExecutionResult, String>>,
    },
    /// Get table information
    GetTableInfo {
        table_name: String,
        respond_to: oneshot::Sender<Result<CachedTableSchema, String>>,
    },
    /// List schemas (always returns ["main"] for SQLite)
    ListSchemas {
        respond_to: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// List tables in a schema
    ListTables {
        schema_name: String,
        respond_to: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// Get row count for a table
    GetRowCount {
        respond_to: oneshot::Sender<Result<usize, String>>,
    },
}

/// State for the Embedded SQLite Actor
pub struct EmbeddedSqliteState {
    pub initialized: bool,
    pub db_path: PathBuf,
    pub csv_path: PathBuf,
    pub row_count: usize,
}

impl Default for EmbeddedSqliteState {
    fn default() -> Self {
        Self {
            initialized: false,
            db_path: PathBuf::new(),
            csv_path: PathBuf::new(),
            row_count: 0,
        }
    }
}

/// Shared reference to the Embedded SQLite Actor state
pub type SharedEmbeddedSqliteState = Arc<RwLock<EmbeddedSqliteState>>;

/// Embedded SQLite Actor
pub struct EmbeddedSqliteActor {
    rx: mpsc::Receiver<EmbeddedSqliteMsg>,
    state: SharedEmbeddedSqliteState,
}

impl EmbeddedSqliteActor {
    /// Create a new Embedded SQLite Actor
    pub fn new(rx: mpsc::Receiver<EmbeddedSqliteMsg>, state: SharedEmbeddedSqliteState) -> Self {
        Self { rx, state }
    }

    /// Run the actor's message loop
    pub async fn run(mut self) {
        println!("[EmbeddedSqliteActor] Started");

        while let Some(msg) = self.rx.recv().await {
            match msg {
                EmbeddedSqliteMsg::EnsureInitialized { respond_to } => {
                    let result = self.ensure_initialized().await;
                    let _ = respond_to.send(result);
                }
                EmbeddedSqliteMsg::ExecuteSql { sql, respond_to } => {
                    let result = self.execute_sql(&sql).await;
                    let _ = respond_to.send(result);
                }
                EmbeddedSqliteMsg::GetTableInfo {
                    table_name,
                    respond_to,
                } => {
                    let result = self.get_table_info(&table_name).await;
                    let _ = respond_to.send(result);
                }
                EmbeddedSqliteMsg::ListSchemas { respond_to } => {
                    let _ = respond_to.send(Ok(vec!["main".to_string()]));
                }
                EmbeddedSqliteMsg::ListTables {
                    schema_name,
                    respond_to,
                } => {
                    let result = self.list_tables(&schema_name).await;
                    let _ = respond_to.send(result);
                }
                EmbeddedSqliteMsg::GetRowCount { respond_to } => {
                    let state = self.state.read().await;
                    let _ = respond_to.send(Ok(state.row_count));
                }
            }
        }

        println!("[EmbeddedSqliteActor] Stopped");
    }

    /// Find the test-data directory by probing from current dir and parents
    fn find_test_data_dir() -> Option<PathBuf> {
        // Try current directory first
        let mut dir = std::env::current_dir().ok()?;

        for _ in 0..5 {
            let test_data = dir.join("test-data");
            if test_data.exists() && test_data.is_dir() {
                return Some(test_data);
            }
            if !dir.pop() {
                break;
            }
        }

        // Also check relative to executable
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                // Check exe_dir/../../test-data (typical for development)
                let dev_path = exe_dir.join("../../test-data");
                if dev_path.exists() {
                    return Some(dev_path.canonicalize().ok()?);
                }
                // Check exe_dir/test-data (for bundled apps)
                let bundled_path = exe_dir.join("test-data");
                if bundled_path.exists() {
                    return Some(bundled_path);
                }
            }
        }

        None
    }

    /// Ensure the database is initialized
    async fn ensure_initialized(&self) -> Result<(), String> {
        {
            let state = self.state.read().await;
            if state.initialized {
                return Ok(());
            }
        }

        println!("[EmbeddedSqliteActor] Initializing demo database...");

        // Find test-data directory
        let test_data_dir = Self::find_test_data_dir()
            .ok_or_else(|| "Could not find test-data directory".to_string())?;

        let db_path = test_data_dir.join("demo.db");
        let csv_path = test_data_dir.join("Chicago_Crimes_2025_Enriched.csv");

        println!("[EmbeddedSqliteActor] DB path: {:?}", db_path);
        println!("[EmbeddedSqliteActor] CSV path: {:?}", csv_path);

        // Check if CSV exists
        if !csv_path.exists() {
            return Err(format!(
                "Chicago Crimes CSV not found at {:?}",
                csv_path
            ));
        }

        // Clone paths for the blocking task
        let db_path_clone = db_path.clone();
        let csv_path_clone = csv_path.clone();

        // Run database initialization in a blocking task
        let row_count = tokio::task::spawn_blocking(move || {
            Self::init_database_sync(&db_path_clone, &csv_path_clone)
        })
        .await
        .map_err(|e| format!("Database init task panicked: {}", e))??;

        // Update state
        {
            let mut state = self.state.write().await;
            state.initialized = true;
            state.db_path = db_path;
            state.csv_path = csv_path;
            state.row_count = row_count;
        }

        println!(
            "[EmbeddedSqliteActor] Database initialized with {} rows",
            row_count
        );

        Ok(())
    }

    /// Synchronous database initialization (runs in blocking task)
    fn init_database_sync(db_path: &PathBuf, csv_path: &PathBuf) -> Result<usize, String> {
        // Open or create the database
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open SQLite database: {}", e))?;

        // Check schema version for migration
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);

        if current_version != DEMO_SCHEMA_VERSION {
            println!(
                "[EmbeddedSqliteActor] Schema version mismatch (found v{}, expected v{}), rebuilding...",
                current_version, DEMO_SCHEMA_VERSION
            );
            // Drop existing table to force rebuild
            conn.execute("DROP TABLE IF EXISTS chicago_crimes", [])
                .map_err(|e| format!("Failed to drop old table: {}", e))?;
        }

        // Create the table if it doesn't exist
        conn.execute(CHICAGO_CRIMES_CREATE_TABLE, [])
            .map_err(|e| format!("Failed to create table: {}", e))?;

        // Check if the table is empty
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chicago_crimes", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count rows: {}", e))?;

        if count > 0 {
            println!(
                "[EmbeddedSqliteActor] Table already has {} rows, skipping CSV load",
                count
            );
            return Ok(count as usize);
        }

        // Load data from CSV
        println!("[EmbeddedSqliteActor] Loading data from CSV...");
        let loaded = Self::load_csv_data(&conn, csv_path)?;

        // Set schema version after successful load
        conn.execute(
            &format!("PRAGMA user_version = {}", DEMO_SCHEMA_VERSION),
            [],
        )
        .map_err(|e| format!("Failed to set schema version: {}", e))?;

        println!(
            "[EmbeddedSqliteActor] Schema version set to v{}",
            DEMO_SCHEMA_VERSION
        );

        Ok(loaded)
    }

    /// Load CSV data into the database
    fn load_csv_data(conn: &Connection, csv_path: &PathBuf) -> Result<usize, String> {
        let file = std::fs::File::open(csv_path)
            .map_err(|e| format!("Failed to open CSV file: {}", e))?;

        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(file);

        // Prepare the INSERT statement
        let insert_sql = r#"
            INSERT INTO chicago_crimes (
                id, case_number, date_of_crime, time_of_crime, block, iucr,
                primary_type, description, location_description, arrest, domestic,
                beat, district, ward, community_area, fbi_code, x_coordinate,
                y_coordinate, year, latitude, longitude, location, day_of_week,
                month_name, hour, is_weekend, season, is_business_hours,
                community_area_name, hardship_index, crime_category, location_zone,
                dist_from_center_km, gun_involved, child_involved
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28,
                ?29, ?30, ?31, ?32, ?33, ?34, ?35
            )
        "#;

        let mut stmt = conn
            .prepare(insert_sql)
            .map_err(|e| format!("Failed to prepare INSERT: {}", e))?;

        let mut count = 0;
        let mut errors = 0;

        // Begin transaction for faster inserts
        conn.execute("BEGIN TRANSACTION", [])
            .map_err(|e| format!("Failed to begin transaction: {}", e))?;

        for result in rdr.records() {
            match result {
                Ok(record) => {
                    if record.len() < 35 {
                        errors += 1;
                        continue;
                    }

                    // Parse each field - indices match new CSV column order
                    let params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                        // id (INTEGER) - CSV index 0
                        Box::new(record.get(0).and_then(|s| s.parse::<i64>().ok())),
                        // case_number (TEXT) - CSV index 1
                        Box::new(record.get(1).map(|s| s.to_string())),
                        // date_of_crime (TEXT, YYYY-MM-DD) - CSV index 2
                        Box::new(record.get(2).map(|s| s.to_string())),
                        // time_of_crime (TEXT, HH:MM:SS) - CSV index 3
                        Box::new(record.get(3).map(|s| s.to_string())),
                        // block (TEXT) - CSV index 4
                        Box::new(record.get(4).map(|s| s.to_string())),
                        // iucr (TEXT) - CSV index 5
                        Box::new(record.get(5).map(|s| s.to_string())),
                        // primary_type (TEXT) - CSV index 6
                        Box::new(record.get(6).map(|s| s.to_string())),
                        // description (TEXT) - CSV index 7
                        Box::new(record.get(7).map(|s| s.to_string())),
                        // location_description (TEXT) - CSV index 8
                        Box::new(record.get(8).map(|s| s.to_string())),
                        // arrest (INTEGER - boolean) - CSV index 9
                        Box::new(Self::parse_bool(record.get(9))),
                        // domestic (INTEGER - boolean) - CSV index 10
                        Box::new(Self::parse_bool(record.get(10))),
                        // beat (INTEGER) - CSV index 11
                        Box::new(record.get(11).and_then(|s| s.parse::<i64>().ok())),
                        // district (INTEGER) - CSV index 12
                        Box::new(record.get(12).and_then(|s| s.parse::<i64>().ok())),
                        // ward (REAL) - CSV index 13
                        Box::new(record.get(13).and_then(|s| s.parse::<f64>().ok())),
                        // community_area (REAL) - CSV index 14
                        Box::new(record.get(14).and_then(|s| s.parse::<f64>().ok())),
                        // fbi_code (TEXT) - CSV index 15
                        Box::new(record.get(15).map(|s| s.to_string())),
                        // x_coordinate (REAL) - CSV index 16
                        Box::new(record.get(16).and_then(|s| s.parse::<f64>().ok())),
                        // y_coordinate (REAL) - CSV index 17
                        Box::new(record.get(17).and_then(|s| s.parse::<f64>().ok())),
                        // year (INTEGER) - CSV index 18
                        Box::new(record.get(18).and_then(|s| s.parse::<i64>().ok())),
                        // latitude (REAL) - CSV index 19
                        Box::new(record.get(19).and_then(|s| s.parse::<f64>().ok())),
                        // longitude (REAL) - CSV index 20
                        Box::new(record.get(20).and_then(|s| s.parse::<f64>().ok())),
                        // location (TEXT) - CSV index 21
                        Box::new(record.get(21).map(|s| s.to_string())),
                        // day_of_week (TEXT) - CSV index 22
                        Box::new(record.get(22).map(|s| s.to_string())),
                        // month_name (TEXT) - CSV index 23
                        Box::new(record.get(23).map(|s| s.to_string())),
                        // hour (INTEGER) - CSV index 24
                        Box::new(record.get(24).and_then(|s| s.parse::<i64>().ok())),
                        // is_weekend (INTEGER - boolean) - CSV index 25
                        Box::new(Self::parse_bool(record.get(25))),
                        // season (TEXT) - CSV index 26
                        Box::new(record.get(26).map(|s| s.to_string())),
                        // is_business_hours (INTEGER - boolean) - CSV index 27
                        Box::new(Self::parse_bool(record.get(27))),
                        // community_area_name (TEXT) - CSV index 28
                        Box::new(record.get(28).map(|s| s.to_string())),
                        // hardship_index (REAL) - CSV index 29
                        Box::new(record.get(29).and_then(|s| s.parse::<f64>().ok())),
                        // crime_category (TEXT) - CSV index 30
                        Box::new(record.get(30).map(|s| s.to_string())),
                        // location_zone (TEXT) - CSV index 31
                        Box::new(record.get(31).map(|s| s.to_string())),
                        // dist_from_center_km (REAL) - CSV index 32
                        Box::new(record.get(32).and_then(|s| s.parse::<f64>().ok())),
                        // gun_involved (INTEGER - boolean) - CSV index 33
                        Box::new(Self::parse_bool(record.get(33))),
                        // child_involved (INTEGER - boolean) - CSV index 34
                        Box::new(Self::parse_bool(record.get(34))),
                    ];

                    match stmt.execute(params_from_iter(params.iter().map(|p| p.as_ref()))) {
                        Ok(_) => count += 1,
                        Err(e) => {
                            if errors < 5 {
                                println!("[EmbeddedSqliteActor] Insert error: {}", e);
                            }
                            errors += 1;
                        }
                    }

                    // Log progress every 5000 rows
                    if count % 5000 == 0 {
                        println!("[EmbeddedSqliteActor] Loaded {} rows...", count);
                    }
                }
                Err(e) => {
                    if errors < 5 {
                        println!("[EmbeddedSqliteActor] CSV parse error: {}", e);
                    }
                    errors += 1;
                }
            }
        }

        // Commit transaction
        conn.execute("COMMIT", [])
            .map_err(|e| format!("Failed to commit transaction: {}", e))?;

        if errors > 0 {
            println!(
                "[EmbeddedSqliteActor] Loaded {} rows with {} errors",
                count, errors
            );
        }

        Ok(count)
    }

    /// Parse a boolean value from CSV (True/False or 1/0)
    fn parse_bool(value: Option<&str>) -> Option<i64> {
        match value {
            Some("True") | Some("true") | Some("1") => Some(1),
            Some("False") | Some("false") | Some("0") => Some(0),
            _ => None,
        }
    }

    /// Execute a SQL query
    async fn execute_sql(&self, sql: &str) -> Result<SqlExecutionResult, String> {
        // Ensure initialized first
        self.ensure_initialized().await?;

        let state = self.state.read().await;
        let db_path = state.db_path.clone();
        drop(state);

        let sql_owned = sql.to_string();

        // Run query in a blocking task
        tokio::task::spawn_blocking(move || Self::execute_sql_sync(&db_path, &sql_owned))
            .await
            .map_err(|e| format!("SQL execution task panicked: {}", e))?
    }

    /// Synchronous SQL execution
    fn execute_sql_sync(db_path: &PathBuf, sql: &str) -> Result<SqlExecutionResult, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare SQL: {}", e))?;

        // Get column names
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        // Execute query and collect rows
        let mut rows: Vec<Vec<Value>> = Vec::new();

        let column_count = stmt.column_count();
        let mut rows_iter = stmt
            .query([])
            .map_err(|e| format!("Failed to execute SQL: {}", e))?;

        while let Some(row) = rows_iter
            .next()
            .map_err(|e| format!("Failed to fetch row: {}", e))?
        {
            let mut row_values: Vec<Value> = Vec::with_capacity(column_count);

            for i in 0..column_count {
                let value = Self::rusqlite_to_json(row, i);
                row_values.push(value);
            }

            rows.push(row_values);
        }

        let row_count = rows.len();

        Ok(SqlExecutionResult {
            success: true,
            columns,
            rows,
            row_count,
            error: None,
        })
    }

    /// Convert a rusqlite value to serde_json::Value
    fn rusqlite_to_json(row: &rusqlite::Row, idx: usize) -> Value {
        // Try different types
        if let Ok(v) = row.get::<_, i64>(idx) {
            return Value::Number(v.into());
        }
        if let Ok(v) = row.get::<_, f64>(idx) {
            return serde_json::Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null);
        }
        if let Ok(v) = row.get::<_, String>(idx) {
            return Value::String(v);
        }
        if let Ok(v) = row.get::<_, Option<i64>>(idx) {
            return v.map(|n| Value::Number(n.into())).unwrap_or(Value::Null);
        }
        if let Ok(v) = row.get::<_, Option<f64>>(idx) {
            return v
                .and_then(|n| serde_json::Number::from_f64(n).map(Value::Number))
                .unwrap_or(Value::Null);
        }
        if let Ok(v) = row.get::<_, Option<String>>(idx) {
            return v.map(Value::String).unwrap_or(Value::Null);
        }

        Value::Null
    }

    /// Get table information
    async fn get_table_info(&self, table_name: &str) -> Result<CachedTableSchema, String> {
        // Ensure initialized first
        self.ensure_initialized().await?;

        // For now, only chicago_crimes is supported
        if table_name == CHICAGO_CRIMES_TABLE_FQ_NAME
            || table_name == "chicago_crimes"
            || table_name == "main.chicago_crimes"
        {
            Ok(chicago_crimes_table_schema())
        } else {
            Err(format!("Unknown table: {}", table_name))
        }
    }

    /// List tables in a schema
    async fn list_tables(&self, schema_name: &str) -> Result<Vec<String>, String> {
        // Ensure initialized first
        self.ensure_initialized().await?;

        if schema_name == "main" || schema_name.is_empty() {
            Ok(vec!["chicago_crimes".to_string()])
        } else {
            Ok(Vec::new())
        }
    }
}

/// Check if a source ID is the embedded demo database
pub fn is_embedded_demo_source(source_id: &str) -> bool {
    source_id == EMBEDDED_DEMO_SOURCE_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_embedded_demo_source() {
        assert!(is_embedded_demo_source("embedded-demo"));
        assert!(!is_embedded_demo_source("other-source"));
    }

    #[test]
    fn test_parse_bool() {
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("True")), Some(1));
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("true")), Some(1));
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("1")), Some(1));
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("False")), Some(0));
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("false")), Some(0));
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("0")), Some(0));
        assert_eq!(EmbeddedSqliteActor::parse_bool(None), None);
        assert_eq!(EmbeddedSqliteActor::parse_bool(Some("invalid")), None);
    }
}
