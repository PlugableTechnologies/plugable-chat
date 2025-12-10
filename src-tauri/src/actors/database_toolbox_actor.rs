//! Database Toolbox Actor - manages Google MCP Database Toolbox process lifecycle
//!
//! This actor handles:
//! - Spawning/stopping the Toolbox binary
//! - Connecting via SSE transport to the Toolbox server
//! - Routing schema discovery and SQL execution requests through MCP
//! - Caching discovered schemas for embedding

use crate::settings::{
    CachedColumnSchema, CachedTableSchema, DatabaseSourceConfig, DatabaseToolboxConfig,
    SupportedDatabaseKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, RwLock};

/// Messages for the Database Toolbox Actor
#[derive(Debug)]
pub enum DatabaseToolboxMsg {
    /// Start the Toolbox process
    Start {
        config: DatabaseToolboxConfig,
        reply_to: oneshot::Sender<Result<(), String>>,
    },
    /// Stop the Toolbox process
    Stop {
        reply_to: oneshot::Sender<Result<(), String>>,
    },
    /// Get the current status
    GetStatus {
        reply_to: oneshot::Sender<ToolboxStatus>,
    },
    /// Enumerate datasets/schemas for a source
    EnumerateSchemas {
        source_id: String,
        reply_to: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// Enumerate tables in a dataset/schema
    EnumerateTables {
        source_id: String,
        dataset_or_schema: String,
        reply_to: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// Get detailed table info (columns, keys, etc.)
    GetTableInfo {
        source_id: String,
        fully_qualified_table: String,
        reply_to: oneshot::Sender<Result<CachedTableSchema, String>>,
    },
    /// Execute SQL query
    ExecuteSql {
        source_id: String,
        sql: String,
        parameters: Vec<Value>,
        reply_to: oneshot::Sender<Result<SqlExecutionResult, String>>,
    },
    /// Test connection to a source
    TestConnection {
        source: DatabaseSourceConfig,
        reply_to: oneshot::Sender<Result<(), String>>,
    },
}

/// Status of the Toolbox process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolboxStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub connected_sources: Vec<String>,
    pub error: Option<String>,
}

impl Default for ToolboxStatus {
    fn default() -> Self {
        Self {
            running: false,
            port: None,
            connected_sources: Vec::new(),
            error: None,
        }
    }
}

/// Result from SQL execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlExecutionResult {
    pub success: bool,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: usize,
    pub error: Option<String>,
}

/// Shared reference to the Database Toolbox Actor state
pub type SharedDatabaseToolboxState = Arc<RwLock<DatabaseToolboxState>>;

/// State for the Database Toolbox Actor
pub struct DatabaseToolboxState {
    pub config: Option<DatabaseToolboxConfig>,
    pub status: ToolboxStatus,
}

impl Default for DatabaseToolboxState {
    fn default() -> Self {
        Self {
            config: None,
            status: ToolboxStatus::default(),
        }
    }
}

/// Database Toolbox Actor
pub struct DatabaseToolboxActor {
    rx: mpsc::Receiver<DatabaseToolboxMsg>,
    state: SharedDatabaseToolboxState,
    toolbox_process: Option<Child>,
    http_client: reqwest::Client,
}

impl DatabaseToolboxActor {
    /// Create a new Database Toolbox Actor
    pub fn new(rx: mpsc::Receiver<DatabaseToolboxMsg>, state: SharedDatabaseToolboxState) -> Self {
        Self {
            rx,
            state,
            toolbox_process: None,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Run the actor's message loop
    pub async fn run(mut self) {
        println!("[DatabaseToolboxActor] Starting...");

        while let Some(msg) = self.rx.recv().await {
            match msg {
                DatabaseToolboxMsg::Start { config, reply_to } => {
                    let result = self.start_toolbox(config).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::Stop { reply_to } => {
                    let result = self.stop_toolbox().await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::GetStatus { reply_to } => {
                    let status = self.state.read().await.status.clone();
                    let _ = reply_to.send(status);
                }
                DatabaseToolboxMsg::EnumerateSchemas {
                    source_id,
                    reply_to,
                } => {
                    let result = self.enumerate_schemas(&source_id).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::EnumerateTables {
                    source_id,
                    dataset_or_schema,
                    reply_to,
                } => {
                    let result = self.enumerate_tables(&source_id, &dataset_or_schema).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::GetTableInfo {
                    source_id,
                    fully_qualified_table,
                    reply_to,
                } => {
                    let result = self.get_table_info(&source_id, &fully_qualified_table).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::ExecuteSql {
                    source_id,
                    sql,
                    parameters,
                    reply_to,
                } => {
                    let result = self.execute_sql(&source_id, &sql, &parameters).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::TestConnection { source, reply_to } => {
                    let result = self.test_connection(&source).await;
                    let _ = reply_to.send(result);
                }
            }
        }

        // Cleanup on shutdown
        if self.toolbox_process.is_some() {
            let _ = self.stop_toolbox().await;
        }

        println!("[DatabaseToolboxActor] Stopped");
    }

    /// Start the Toolbox process
    async fn start_toolbox(&mut self, config: DatabaseToolboxConfig) -> Result<(), String> {
        // Check if already running
        if self.toolbox_process.is_some() {
            return Err("Toolbox is already running".to_string());
        }

        // Validate config
        let toolbox_path = config
            .toolbox_path
            .as_ref()
            .ok_or("Toolbox binary path not configured")?;

        let tools_yaml_path = config
            .tools_yaml_path
            .as_ref()
            .ok_or("tools.yaml path not configured")?;

        println!(
            "[DatabaseToolboxActor] Starting Toolbox: {} --tools-file {} --port {}",
            toolbox_path, tools_yaml_path, config.port
        );

        // Spawn the Toolbox process
        let mut cmd = Command::new(toolbox_path);
        cmd.arg("--tools-file")
            .arg(tools_yaml_path)
            .arg("--port")
            .arg(config.port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn Toolbox process: {}", e))?;

        // Wait briefly for startup and check for early failure
        tokio::time::sleep(Duration::from_millis(500)).await;

        match child.try_wait() {
            Ok(Some(status)) => {
                // Process already exited
                let stderr = if let Some(stderr) = child.stderr.take() {
                    let mut reader = BufReader::new(stderr).lines();
                    let mut output = String::new();
                    while let Ok(Some(line)) = reader.next_line().await {
                        output.push_str(&line);
                        output.push('\n');
                    }
                    output
                } else {
                    String::new()
                };
                return Err(format!(
                    "Toolbox exited immediately with status {}: {}",
                    status, stderr
                ));
            }
            Ok(None) => {
                // Process is still running, good
            }
            Err(e) => {
                return Err(format!("Failed to check Toolbox status: {}", e));
            }
        }

        self.toolbox_process = Some(child);

        // Update state
        {
            let mut state = self.state.write().await;
            state.config = Some(config.clone());
            state.status = ToolboxStatus {
                running: true,
                port: Some(config.port),
                connected_sources: config.sources.iter().map(|s| s.id.clone()).collect(),
                error: None,
            };
        }

        println!("[DatabaseToolboxActor] Toolbox started on port {}", config.port);
        Ok(())
    }

    /// Stop the Toolbox process
    async fn stop_toolbox(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.toolbox_process.take() {
            println!("[DatabaseToolboxActor] Stopping Toolbox...");
            child
                .kill()
                .await
                .map_err(|e| format!("Failed to kill Toolbox process: {}", e))?;

            // Update state
            {
                let mut state = self.state.write().await;
                state.status = ToolboxStatus::default();
            }

            println!("[DatabaseToolboxActor] Toolbox stopped");
        }
        Ok(())
    }

    /// Get the base URL for the Toolbox API
    fn get_base_url(&self, state: &DatabaseToolboxState) -> Result<String, String> {
        let port = state
            .status
            .port
            .ok_or("Toolbox not running")?;
        Ok(format!("http://localhost:{}", port))
    }

    /// Enumerate schemas/datasets for a source
    async fn enumerate_schemas(&self, source_id: &str) -> Result<Vec<String>, String> {
        let (source, base_url) = {
            let state = self.state.read().await;
            let config = state.config.as_ref().ok_or("Toolbox not configured")?;
            let source = config
                .sources
                .iter()
                .find(|s| s.id == source_id)
                .ok_or_else(|| format!("Source not found: {}", source_id))?
                .clone();
            let base_url = self.get_base_url(&state)?;
            (source, base_url)
        };

        // Call the appropriate enumeration tool based on database kind
        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let project_id = source
                    .project_id
                    .as_ref()
                    .ok_or("BigQuery source requires project_id")?;

                let response = self
                    .call_mcp_tool(
                        &base_url,
                        "bigquery-list-dataset-ids",
                        json!({ "project_id": project_id }),
                    )
                    .await?;

                // Parse response to extract dataset IDs
                self.parse_list_response(response)
            }
            SupportedDatabaseKind::Postgres | SupportedDatabaseKind::Mysql => {
                // Use INFORMATION_SCHEMA query
                let tool_name = source.kind.execute_tool_name();
                let query = match source.kind {
                    SupportedDatabaseKind::Postgres => {
                        "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog', 'information_schema')"
                    }
                    SupportedDatabaseKind::Mysql => {
                        "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')"
                    }
                    _ => unreachable!(),
                };

                let response = self
                    .call_mcp_tool(&base_url, tool_name, json!({ "sql": query }))
                    .await?;

                self.parse_sql_column_response(response, "schema_name")
            }
            SupportedDatabaseKind::Sqlite => {
                // SQLite has a single database, return empty list (tables are at top level)
                Ok(vec!["main".to_string()])
            }
            SupportedDatabaseKind::Spanner => {
                // Spanner: list databases in instance
                let response = self
                    .call_mcp_tool(&base_url, "spanner-list-databases", json!({}))
                    .await?;

                self.parse_list_response(response)
            }
        }
    }

    /// Enumerate tables in a dataset/schema
    async fn enumerate_tables(
        &self,
        source_id: &str,
        dataset_or_schema: &str,
    ) -> Result<Vec<String>, String> {
        let (source, base_url) = {
            let state = self.state.read().await;
            let config = state.config.as_ref().ok_or("Toolbox not configured")?;
            let source = config
                .sources
                .iter()
                .find(|s| s.id == source_id)
                .ok_or_else(|| format!("Source not found: {}", source_id))?
                .clone();
            let base_url = self.get_base_url(&state)?;
            (source, base_url)
        };

        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let project_id = source
                    .project_id
                    .as_ref()
                    .ok_or("BigQuery source requires project_id")?;

                let response = self
                    .call_mcp_tool(
                        &base_url,
                        "bigquery-list-table-ids",
                        json!({
                            "project_id": project_id,
                            "dataset_id": dataset_or_schema
                        }),
                    )
                    .await?;

                self.parse_list_response(response)
            }
            SupportedDatabaseKind::Postgres => {
                let query = format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = '{}'",
                    dataset_or_schema.replace("'", "''")
                );
                let response = self
                    .call_mcp_tool(
                        &base_url,
                        "postgres-sql",
                        json!({ "sql": query }),
                    )
                    .await?;

                self.parse_sql_column_response(response, "table_name")
            }
            SupportedDatabaseKind::Mysql => {
                let query = format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = '{}'",
                    dataset_or_schema.replace("'", "''")
                );
                let response = self
                    .call_mcp_tool(&base_url, "mysql-sql", json!({ "sql": query }))
                    .await?;

                self.parse_sql_column_response(response, "table_name")
            }
            SupportedDatabaseKind::Sqlite => {
                let response = self
                    .call_mcp_tool(
                        &base_url,
                        "sqlite-sql",
                        json!({ "sql": "SELECT name FROM sqlite_master WHERE type='table'" }),
                    )
                    .await?;

                self.parse_sql_column_response(response, "name")
            }
            SupportedDatabaseKind::Spanner => {
                let response = self
                    .call_mcp_tool(&base_url, "spanner-list-tables", json!({}))
                    .await?;

                self.parse_list_response(response)
            }
        }
    }

    /// Get detailed table information
    async fn get_table_info(
        &self,
        source_id: &str,
        fully_qualified_table: &str,
    ) -> Result<CachedTableSchema, String> {
        let state = self.state.read().await;
        let config = state.config.as_ref().ok_or("Toolbox not configured")?;

        let source = config
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {}", source_id))?
            .clone();

        let base_url = self.get_base_url(&state)?;
        drop(state);

        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let parts: Vec<&str> = fully_qualified_table.split('.').collect();
                if parts.len() < 2 {
                    return Err("BigQuery table must be in format dataset.table or project.dataset.table".to_string());
                }
                
                let (project, dataset, table) = if parts.len() == 3 {
                    (parts[0].to_string(), parts[1].to_string(), parts[2].to_string())
                } else {
                    let project = source.project_id.ok_or("BigQuery source requires project_id")?;
                    (project, parts[0].to_string(), parts[1].to_string())
                };

                let response = self
                    .call_mcp_tool(
                        &base_url,
                        "bigquery-get-table-info",
                        json!({
                            "project_id": project,
                            "dataset_id": dataset,
                            "table_id": table
                        }),
                    )
                    .await?;

                self.parse_bigquery_table_info(&source.id, source.kind, &response)
            }
            _ => {
                // For other databases, use INFORMATION_SCHEMA
                self.get_table_info_via_information_schema(&source, &base_url, fully_qualified_table)
                    .await
            }
        }
    }

    /// Get table info using INFORMATION_SCHEMA queries
    async fn get_table_info_via_information_schema(
        &self,
        source: &DatabaseSourceConfig,
        base_url: &str,
        fully_qualified_table: &str,
    ) -> Result<CachedTableSchema, String> {
        let (schema_name, table_name) = self.parse_table_name(source.kind, fully_qualified_table)?;
        let tool_name = source.kind.execute_tool_name();

        // Query column information
        let column_query = match source.kind {
            SupportedDatabaseKind::Postgres | SupportedDatabaseKind::Mysql => format!(
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = '{}' AND table_name = '{}' ORDER BY ordinal_position",
                schema_name.replace("'", "''"),
                table_name.replace("'", "''")
            ),
            SupportedDatabaseKind::Sqlite => format!(
                "PRAGMA table_info('{}')",
                table_name.replace("'", "''")
            ),
            _ => return Err(format!("Unsupported database kind for INFORMATION_SCHEMA: {:?}", source.kind)),
        };

        let response = self
            .call_mcp_tool(base_url, tool_name, json!({ "sql": column_query }))
            .await?;

        let columns = self.parse_column_info(source.kind, &response)?;

        Ok(CachedTableSchema {
            fully_qualified_name: fully_qualified_table.to_string(),
            source_id: source.id.clone(),
            kind: source.kind,
            columns,
            primary_keys: Vec::new(), // Would need additional query for PKs
            partition_columns: Vec::new(),
            cluster_columns: Vec::new(),
            description: None,
        })
    }

    /// Execute SQL query
    async fn execute_sql(
        &self,
        source_id: &str,
        sql: &str,
        _parameters: &[Value],
    ) -> Result<SqlExecutionResult, String> {
        let state = self.state.read().await;
        let config = state.config.as_ref().ok_or("Toolbox not configured")?;

        let source = config
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {}", source_id))?;

        let base_url = self.get_base_url(&state)?;
        let tool_name = source.kind.execute_tool_name();
        drop(state);

        let response = self
            .call_mcp_tool(&base_url, tool_name, json!({ "sql": sql }))
            .await?;

        self.parse_sql_execution_result(&response)
    }

    /// Test connection to a source
    async fn test_connection(&self, source: &DatabaseSourceConfig) -> Result<(), String> {
        let state = self.state.read().await;
        let base_url = self.get_base_url(&state)?;
        drop(state);

        // Simple test query
        let test_query = match source.kind {
            SupportedDatabaseKind::Postgres => "SELECT 1 AS test",
            SupportedDatabaseKind::Mysql => "SELECT 1 AS test",
            SupportedDatabaseKind::Sqlite => "SELECT 1 AS test",
            SupportedDatabaseKind::Bigquery => "SELECT 1 AS test",
            SupportedDatabaseKind::Spanner => "SELECT 1 AS test",
        };

        let tool_name = source.kind.execute_tool_name();
        self.call_mcp_tool(&base_url, tool_name, json!({ "sql": test_query }))
            .await?;

        Ok(())
    }

    /// Call an MCP tool via the Toolbox HTTP API
    async fn call_mcp_tool(
        &self,
        base_url: &str,
        tool_name: &str,
        params: Value,
    ) -> Result<Value, String> {
        let url = format!("{}/api/tool/{}", base_url, tool_name);

        println!("[DatabaseToolboxActor] Calling tool: {} with params: {}", tool_name, params);

        let response = self
            .http_client
            .post(&url)
            .json(&params)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(format!("Tool call failed ({}): {}", status, body));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        Ok(result)
    }

    // ========== Response Parsing Helpers ==========

    fn parse_list_response(&self, response: Value) -> Result<Vec<String>, String> {
        // Try to extract array from various response formats
        if let Some(arr) = response.as_array() {
            return Ok(arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
        }
        if let Some(result) = response.get("result") {
            if let Some(arr) = result.as_array() {
                return Ok(arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            }
        }
        if let Some(data) = response.get("data") {
            if let Some(arr) = data.as_array() {
                return Ok(arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            }
        }
        Err(format!("Unexpected response format: {}", response))
    }

    fn parse_sql_column_response(&self, response: Value, column_name: &str) -> Result<Vec<String>, String> {
        // Parse SQL result set and extract a single column
        let rows = response
            .get("rows")
            .or_else(|| response.get("result"))
            .or_else(|| response.get("data"))
            .and_then(|r| r.as_array())
            .ok_or("No rows in response")?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                if let Some(obj) = row.as_object() {
                    obj.get(column_name)
                        .or_else(|| obj.get(&column_name.to_uppercase()))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                } else if let Some(arr) = row.as_array() {
                    arr.first().and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect())
    }

    fn parse_bigquery_table_info(
        &self,
        source_id: &str,
        kind: SupportedDatabaseKind,
        response: &Value,
    ) -> Result<CachedTableSchema, String> {
        // Toolbox returns Schema as a top-level array with Pascal-case field names
        let fields = response
            .get("Schema")
            .and_then(|s| s.as_array())
            .ok_or("No Schema array in response")?;

        let columns: Vec<CachedColumnSchema> = fields
            .iter()
            .filter_map(|f| {
                // Pascal-case field names: Name, Type, Required, Description
                let name = f.get("Name")?.as_str()?.to_string();
                let data_type = f.get("Type")?.as_str()?.to_string();
                let required = f.get("Required").and_then(|r| r.as_bool()).unwrap_or(false);
                Some(CachedColumnSchema {
                    name,
                    data_type,
                    nullable: !required,
                    description: f.get("Description")
                        .and_then(|d| d.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                })
            })
            .collect();

        // FullID format: "project:dataset.table"
        let full_id = response
            .get("FullID")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        
        // Parse fully_qualified_name from FullID
        let fully_qualified_name = if full_id.contains(':') {
            // Convert "project:dataset.table" to "project.dataset.table"
            full_id.replace(':', ".")
        } else {
            full_id.to_string()
        };

        // Extract clustering info (Pascal-case)
        let cluster_columns = response
            .get("Clustering")
            .and_then(|c| c.as_object())
            .and_then(|c| c.get("Fields"))
            .and_then(|f| f.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Extract partitioning info (Pascal-case)
        let partition_columns = response
            .get("TimePartitioning")
            .and_then(|tp| tp.as_object())
            .and_then(|tp| tp.get("Field"))
            .and_then(|f| f.as_str())
            .map(|f| vec![f.to_string()])
            .unwrap_or_default();

        Ok(CachedTableSchema {
            fully_qualified_name,
            source_id: source_id.to_string(),
            kind,
            columns,
            primary_keys: Vec::new(), // BigQuery doesn't have traditional PKs
            partition_columns,
            cluster_columns,
            description: response.get("Description")
                .and_then(|d| d.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from),
        })
    }

    fn parse_table_name(
        &self,
        kind: SupportedDatabaseKind,
        fully_qualified: &str,
    ) -> Result<(String, String), String> {
        let parts: Vec<&str> = fully_qualified.split('.').collect();
        match kind {
            SupportedDatabaseKind::Postgres | SupportedDatabaseKind::Mysql => {
                if parts.len() >= 2 {
                    Ok((parts[parts.len() - 2].to_string(), parts[parts.len() - 1].to_string()))
                } else {
                    Ok(("public".to_string(), parts[0].to_string()))
                }
            }
            SupportedDatabaseKind::Sqlite => {
                Ok(("main".to_string(), parts.last().unwrap_or(&"").to_string()))
            }
            _ => Err(format!("Unsupported kind for parse_table_name: {:?}", kind)),
        }
    }

    fn parse_column_info(
        &self,
        kind: SupportedDatabaseKind,
        response: &Value,
    ) -> Result<Vec<CachedColumnSchema>, String> {
        let rows = response
            .get("rows")
            .or_else(|| response.get("result"))
            .and_then(|r| r.as_array())
            .ok_or("No rows in column info response")?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                if kind == SupportedDatabaseKind::Sqlite {
                    // SQLite PRAGMA table_info format
                    let arr = row.as_array()?;
                    Some(CachedColumnSchema {
                        name: arr.get(1)?.as_str()?.to_string(),
                        data_type: arr.get(2)?.as_str()?.to_string(),
                        nullable: arr.get(3)?.as_i64()? == 0,
                        description: None,
                    })
                } else {
                    // Standard INFORMATION_SCHEMA format
                    let obj = row.as_object()?;
                    Some(CachedColumnSchema {
                        name: obj.get("column_name")?.as_str()?.to_string(),
                        data_type: obj.get("data_type")?.as_str()?.to_string(),
                        nullable: obj.get("is_nullable")?.as_str()? == "YES",
                        description: None,
                    })
                }
            })
            .collect())
    }

    fn parse_sql_execution_result(&self, response: &Value) -> Result<SqlExecutionResult, String> {
        // Extract columns and rows from response
        let columns = response
            .get("columns")
            .and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let rows: Vec<Vec<Value>> = response
            .get("rows")
            .or_else(|| response.get("data"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|row| {
                        if let Some(obj) = row.as_object() {
                            Some(obj.values().cloned().collect())
                        } else if let Some(arr) = row.as_array() {
                            Some(arr.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let row_count = rows.len();

        Ok(SqlExecutionResult {
            success: true,
            columns,
            rows,
            row_count,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolbox_status_default() {
        let status = ToolboxStatus::default();
        assert!(!status.running);
        assert!(status.port.is_none());
        assert!(status.connected_sources.is_empty());
        assert!(status.error.is_none());
    }

    #[test]
    fn test_sql_execution_result_serde() {
        let result = SqlExecutionResult {
            success: true,
            columns: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![json!(1), json!("test")]],
            row_count: 1,
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: SqlExecutionResult = serde_json::from_str(&json).unwrap();

        assert!(parsed.success);
        assert_eq!(parsed.columns.len(), 2);
        assert_eq!(parsed.row_count, 1);
    }
}
