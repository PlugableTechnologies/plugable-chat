//! Database Toolbox Actor - manages Google MCP Database Toolbox process lifecycle
//!
//! This actor handles:
//! - Spawning/stopping the Toolbox binary
//! - Connecting via SSE transport to the Toolbox server
//! - Routing schema discovery and SQL execution requests through MCP
//! - Caching discovered schemas for embedding

use crate::protocol::McpHostMsg;
use crate::settings::{
    CachedColumnSchema, CachedTableSchema, DatabaseSourceConfig, DatabaseToolboxConfig,
    McpServerConfig, SupportedDatabaseKind,
};
use crate::actors::mcp_host_actor::McpTool;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use std::sync::Arc;
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
    pub connected_sources: Vec<String>,
    pub error: Option<String>,
}

impl Default for ToolboxStatus {
    fn default() -> Self {
        Self {
            running: false,
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
    mcp_host_tx: mpsc::Sender<McpHostMsg>,
}

impl DatabaseToolboxActor {
    fn sanitize_identifier(&self, value: &str) -> String {
        let trimmed = value.trim();
        trimmed.trim_matches('"').trim_matches('\'').to_string()
    }

    /// Create a new Database Toolbox Actor
    pub fn new(
        rx: mpsc::Receiver<DatabaseToolboxMsg>,
        state: SharedDatabaseToolboxState,
        mcp_host_tx: mpsc::Sender<McpHostMsg>,
    ) -> Self {
        Self {
            rx,
            state,
            mcp_host_tx,
        }
    }

    /// Run the actor's message loop
    pub async fn run(mut self) {

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
                    let result = self.sql_select(&source_id, &sql, &parameters).await;
                    let _ = reply_to.send(result);
                }
                DatabaseToolboxMsg::TestConnection { source, reply_to } => {
                    let result = self.test_connection(&source).await;
                    let _ = reply_to.send(result);
                }
            }
        }

        // Cleanup on shutdown
        let _ = self.stop_toolbox().await;

        println!("[DatabaseToolboxActor] Stopped");
    }

    /// Start (sync) MCP database servers
    async fn start_toolbox(&mut self, config: DatabaseToolboxConfig) -> Result<(), String> {
        let source_labels: std::collections::HashMap<String, String> = config
            .sources
            .iter()
            .map(|src| (src.id.clone(), src.name.clone()))
            .collect();

        // Build the list of sources to sync
        let mut mcp_configs: Vec<McpServerConfig> = config
            .sources
            .iter()
            .cloned()
            .map(|src| self.source_to_mcp_config(&src))
            .collect();

        // Also include any sources that were in the OLD config but are NOT in the NEW config,
        // marking them as disabled so they are correctly disconnected.
        {
            let state = self.state.read().await;
            if let Some(old_config) = &state.config {
                for old_source in &old_config.sources {
                    if !config.sources.iter().any(|s| s.id == old_source.id) {
                        println!("[DatabaseToolboxActor] Source {} removed, marking for disconnection", old_source.id);
                        let mut mcp_config = self.source_to_mcp_config(old_source);
                        mcp_config.enabled = false;
                        mcp_configs.push(mcp_config);
                    }
                }
            }
        }

        let (tx, rx) = oneshot::channel();
        self.mcp_host_tx
            .send(McpHostMsg::SyncEnabledServers {
                configs: mcp_configs,
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Failed to sync MCP database servers: {}", e))?;

        let results = rx
            .await
            .map_err(|_| "MCP host unavailable while syncing database servers".to_string())?;

        // Surface any connection errors immediately so the caller can show them
        let failed: Vec<String> = results
            .into_iter()
            .filter_map(|(id, res)| match res {
                Ok(_) => None,
                Err(err) => {
                    let label = source_labels
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| id.clone());
                    Some(format!("{} ({}): {}", label, id, err))
                }
            })
            .collect();
        if !failed.is_empty() {
            return Err(format!(
                "Failed to connect database MCP servers: {}",
                failed.join("; ")
            ));
        }

        // Update state
        {
            let mut state = self.state.write().await;
            state.config = Some(config.clone());
            state.status = ToolboxStatus {
                running: true,
                connected_sources: config
                    .sources
                    .iter()
                    .filter(|s| s.enabled)
                    .map(|s| s.id.clone())
                    .collect(),
                error: None,
            };
        }

        println!("[DatabaseToolboxActor] Database MCP servers synced");
        Ok(())
    }

    fn source_to_mcp_config(&self, source: &DatabaseSourceConfig) -> McpServerConfig {
        let mut env = source.env.clone();

        // BigQuery requires BIGQUERY_PROJECT; derive it from project_id when not provided
        if source.kind == SupportedDatabaseKind::Bigquery {
            if let Some(project_id) = source.project_id.as_ref() {
                if !project_id.trim().is_empty() && !env.contains_key("BIGQUERY_PROJECT") {
                    env.insert("BIGQUERY_PROJECT".to_string(), project_id.trim().to_string());
                }
            }
        }

        McpServerConfig {
            id: source.id.clone(),
            name: source.name.clone(),
            enabled: source.enabled,
            transport: source.transport.clone(),
            command: source.command.clone(),
            args: source.args.clone(),
            env,
            auto_approve_tools: source.auto_approve_tools,
            defer_tools: source.defer_tools,
            python_name: None,
        }
    }

    async fn call_mcp_tool_value(
        &self,
        server_id: &str,
        tool_name: &str,
        params: Value,
    ) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        self.mcp_host_tx
            .send(McpHostMsg::ExecuteTool {
                server_id: server_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: params,
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Failed to send MCP ExecuteTool: {}", e))?;

        let result = rx
            .await
            .map_err(|_| "MCP host unavailable during ExecuteTool".to_string())??;

        if result.is_error {
            let err_msg = result
                .content
                .first()
                .and_then(|c| c.text.clone())
                .unwrap_or_else(|| "MCP tool returned an error".to_string());
            return Err(err_msg);
        }

        if let Some(first) = result.content.first() {
            // If there are multiple text items, treat them as an array.
            // Try to parse each as JSON if possible, otherwise keep as string.
            if result.content.len() > 1 && result.content.iter().all(|c| c.text.is_some()) {
                let items: Vec<Value> = result
                    .content
                    .iter()
                    .filter_map(|c| {
                        c.text.as_ref().map(|t| {
                            serde_json::from_str::<Value>(t).unwrap_or(Value::String(t.clone()))
                        })
                    })
                    .collect();
                return Ok(Value::Array(items));
            }

            if let Some(data) = &first.data {
                if let Ok(val) = serde_json::from_str::<Value>(data) {
                    return Ok(val);
                }
                return Ok(Value::String(data.clone()));
            }
            if let Some(text) = &first.text {
                if let Ok(val) = serde_json::from_str::<Value>(text) {
                    return Ok(val);
                }
                return Ok(Value::String(text.clone()));
            }
        }

        Err("MCP tool returned no usable content".to_string())
    }

    async fn call_mcp_tool_value_checked(
        &self,
        server_id: &str,
        label: &str,
        candidates: &[&str],
        params: Value,
    ) -> Result<Value, String> {
        let tools = self.get_tool_catalog(server_id).await?;
        let available: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        // Pick the first candidate that exists
        let tool = candidates
            .iter()
            .find_map(|cand| tools.iter().find(|t| t.name == *cand))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{}: required tool not found. Tried [{}]. Available: [{}]",
                    label,
                    candidates.join(", "),
                    available.join(", ")
                )
            })?;

        // Validate parameters against tool schema
        let (required_params, property_keys) = Self::extract_schema_requirements(tool.input_schema.as_ref());
        let args_obj = params
            .as_object()
            .ok_or_else(|| format!("{}: arguments must be an object", label))?;

        let provided_keys: Vec<String> = args_obj.keys().cloned().collect();
        let missing: Vec<String> = required_params
            .iter()
            .filter(|r| !args_obj.contains_key(*r))
            .cloned()
            .collect();

        let extra: Vec<String> = if property_keys.is_empty() {
            Vec::new()
        } else {
            provided_keys
                .iter()
                .filter(|k| !property_keys.contains(k))
                .cloned()
                .collect()
        };

        if !missing.is_empty() {
            return Err(format!(
                "{}: parameter mismatch for tool '{}'. Missing: [{}]. Extra: [{}]. Required: [{}]. Provided: [{}]. Available tools: [{}]",
                label,
                tool.name,
                missing.join(", "),
                extra.join(", "),
                required_params.join(", "),
                provided_keys.join(", "),
                available.join(", ")
            ));
        }

        if !extra.is_empty() {
            println!(
                "[DatabaseToolboxActor] {}: tool '{}' extra params ignored: [{}] (required: [{}])",
                label,
                tool.name,
                extra.join(", "),
                required_params.join(", ")
            );
        }

        self.call_mcp_tool_value(server_id, &tool.name, params).await
    }

    async fn get_tool_catalog(&self, server_id: &str) -> Result<Vec<McpTool>, String> {
        let (tx, rx) = oneshot::channel();
        self.mcp_host_tx
            .send(McpHostMsg::ListTools {
                server_id: server_id.to_string(),
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Failed to request tool list: {}", e))?;

        rx.await
            .map_err(|_| "MCP host unavailable while listing tools".to_string())?
    }

    fn extract_schema_requirements(schema: Option<&Value>) -> (Vec<String>, Vec<String>) {
        let mut required = Vec::new();
        let mut properties = Vec::new();
        if let Some(Value::Object(map)) = schema {
            if let Some(Value::Array(reqs)) = map.get("required") {
                for r in reqs {
                    if let Some(s) = r.as_str() {
                        required.push(s.to_string());
                    }
                }
            }
            if let Some(Value::Object(props)) = map.get("properties") {
                properties.extend(props.keys().cloned());
            }
        }
        (required, properties)
    }

    /// Stop all database MCP servers (disconnect)
    async fn stop_toolbox(&mut self) -> Result<(), String> {
        let mut configs = Vec::new();
        
        // Get known sources from state and mark them as disabled for sync
        {
            let state = self.state.read().await;
            if let Some(config) = &state.config {
                for source in &config.sources {
                    let mut mcp_config = self.source_to_mcp_config(source);
                    mcp_config.enabled = false;
                    configs.push(mcp_config);
                }
            }
        }

        if !configs.is_empty() {
            let (tx, rx) = oneshot::channel();
            self.mcp_host_tx
                .send(McpHostMsg::SyncEnabledServers {
                    configs,
                    respond_to: tx,
                })
                .await
                .map_err(|e| format!("Failed to send stop message: {}", e))?;
            let _ = rx
                .await
                .map_err(|_| "MCP host unavailable while stopping database servers".to_string())?;
        }

        {
            let mut state = self.state.write().await;
            state.status = ToolboxStatus::default();
        }
        println!("[DatabaseToolboxActor] Database MCP servers disconnected");
        Ok(())
    }

    /// Enumerate schemas/datasets for a source
    async fn enumerate_schemas(&self, source_id: &str) -> Result<Vec<String>, String> {
        let source = {
            let state = self.state.read().await;
            let config = state.config.as_ref().ok_or("Toolbox not configured")?;
            config
                .sources
                .iter()
                .find(|s| s.id == source_id)
                .ok_or_else(|| format!("Source not found: {}", source_id))?
                .clone()
        };

        // Call the appropriate enumeration tool based on database kind
        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let project_id = source
                    .project_id
                    .as_ref()
                    .ok_or("BigQuery source requires project_id")?;

                let response = self
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list datasets",
                        &[
                            "list_dataset_ids",
                            "bigquery-list-datasets",
                            "bigquery-list-dataset-ids",
                        ],
                        json!({ "project_id": project_id }),
                    )
                    .await?;

                // Parse response to extract dataset IDs
                let datasets = self.parse_list_response(response)?;
                Ok(datasets
                    .into_iter()
                    .map(|d| self.sanitize_identifier(&d))
                    .collect())
            }
            SupportedDatabaseKind::Postgres | SupportedDatabaseKind::Mysql => {
                // Use INFORMATION_SCHEMA query
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
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list schemas",
                        &[source.kind.execute_tool_name()],
                        json!({ "sql": query }),
                    )
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
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list databases",
                        &["spanner-list-databases"],
                        json!({}),
                    )
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
        let source = {
            let state = self.state.read().await;
            let config = state.config.as_ref().ok_or("Toolbox not configured")?;
            config
                .sources
                .iter()
                .find(|s| s.id == source_id)
                .ok_or_else(|| format!("Source not found: {}", source_id))?
                .clone()
        };

        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let project_id = source
                    .project_id
                    .as_ref()
                    .ok_or("BigQuery source requires project_id")?;
                let dataset_clean = self.sanitize_identifier(dataset_or_schema);

                let response = self
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list tables",
                        &[
                            "list_table_ids",
                            "bigquery-list-table-ids",
                            "bigquery-list-tables",
                        ],
                        json!({
                            // According to MCP docs, list_table_ids expects `dataset` (required) and `project` (optional)
                            "dataset": dataset_clean,
                            "project": project_id,
                        }),
                    )
                    .await?;

                let tables = self.parse_list_response(response)?;
                Ok(tables
                    .into_iter()
                    .map(|t| self.sanitize_identifier(&t))
                    .collect())
            }
            SupportedDatabaseKind::Postgres => {
                let query = format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = '{}'",
                    dataset_or_schema.replace("'", "''")
                );
                let response = self
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list tables",
                        &["postgres-sql"],
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
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "list tables",
                        &["mysql-sql"],
                        json!({ "sql": query }),
                    )
                    .await?;

                self.parse_sql_column_response(response, "table_name")
            }
            SupportedDatabaseKind::Sqlite => {
                let response = self
                    .call_mcp_tool_value(
                        &source.id,
                        "sqlite-sql",
                        json!({ "sql": "SELECT name FROM sqlite_master WHERE type='table'" }),
                    )
                    .await?;

                self.parse_sql_column_response(response, "name")
            }
            SupportedDatabaseKind::Spanner => {
                let response = self
                    .call_mcp_tool_value(&source.id, "spanner-list-tables", json!({}))
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
        drop(state);

        match source.kind {
            SupportedDatabaseKind::Bigquery => {
                let parts: Vec<&str> = fully_qualified_table.split('.').collect();
                if parts.len() < 2 {
                    return Err("BigQuery table must be in format dataset.table or project.dataset.table".to_string());
                }
                
                let (project, dataset, table) = if parts.len() == 3 {
                    (
                        self.sanitize_identifier(parts[0]),
                        self.sanitize_identifier(parts[1]),
                        self.sanitize_identifier(parts[2]),
                    )
                } else {
                    let project = source.project_id.as_ref().ok_or("BigQuery source requires project_id")?.clone();
                    (
                        project,
                        self.sanitize_identifier(parts[0]),
                        self.sanitize_identifier(parts[1]),
                    )
                };

                let response = self
                    .call_mcp_tool_value_checked(
                        &source.id,
                        "get table info",
                        &["get_table_info", "bigquery-get-table-info"],
                        json!({
                            // Per toolbox docs: required dataset/table, optional project
                            "dataset": dataset,
                            "table": table,
                            "project": project,
                        }),
                    )
                    .await?;

                self.parse_bigquery_table_info(&source.id, source.kind, source.get_sql_dialect(), &response)
            }
            _ => {
                // For other databases, use INFORMATION_SCHEMA
                self.get_table_info_via_information_schema(&source, fully_qualified_table)
                    .await
            }
        }
    }

    /// Get table info using INFORMATION_SCHEMA queries
    async fn get_table_info_via_information_schema(
        &self,
        source: &DatabaseSourceConfig,
        fully_qualified_table: &str,
    ) -> Result<CachedTableSchema, String> {
        let (schema_name, table_name) = self.parse_table_name(source.kind, fully_qualified_table)?;

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
            .call_mcp_tool_value_checked(
                &source.id,
                "describe table",
                &[source.kind.execute_tool_name()],
                json!({ "sql": column_query }),
            )
            .await?;

        let columns = self.parse_column_info(source.kind, &response)?;

        Ok(CachedTableSchema {
            fully_qualified_name: fully_qualified_table.to_string(),
            source_id: source.id.clone(),
            kind: source.kind,
            sql_dialect: source.get_sql_dialect().to_string(),
            enabled: true,
            columns,
            primary_keys: Vec::new(), // Would need additional query for PKs
            partition_columns: Vec::new(),
            cluster_columns: Vec::new(),
            description: None,
        })
    }

    /// Execute SQL query
    async fn sql_select(
        &self,
        source_id: &str,
        sql: &str,
        _parameters: &[Value],
    ) -> Result<SqlExecutionResult, String> {
        let (source, config) = {
            let state = self.state.read().await;
            let config = state.config.clone();
            let source = config.as_ref().and_then(|c| {
                c.sources.iter().find(|s| s.id == source_id).cloned()
            });
            (source, config)
        };

        // If not configured, we can't execute. The caller (lib.rs) should ensure_toolbox_running.
        let source = source.ok_or_else(|| {
            if config.is_none() {
                "Toolbox not configured. Please ensure database sources are enabled in settings.".to_string()
            } else {
                format!("Source not found: {}", source_id)
            }
        })?;

        let execute_candidates: Vec<&str> = if source.kind == SupportedDatabaseKind::Bigquery {
            vec!["sql_select", "execute_sql", "bigquery-execute-sql"]
        } else {
            vec![source.kind.execute_tool_name(), "execute_sql"]
        };

        let result = self
            .call_mcp_tool_value_checked(
                &source.id,
                "execute sql",
                &execute_candidates,
                json!({ "sql": sql }),
            )
            .await;

        match result {
            Ok(response) => self.parse_sql_execution_result(&response),
            Err(e) if e.contains("required tool not found") || e.contains("host unavailable") => {
                // Potential connection issue, suggest re-syncing
                println!("[DatabaseToolboxActor] SqlSelect failed with potential connection issue: {}. Suggesting re-initialization.", e);
                Err(format!("Database connection lost for '{}'. Please try refreshing database schemas in settings or check if the database is reachable.", source.name))
            }
            Err(e) => Err(e),
        }
    }

    /// Test connection to a source
    async fn test_connection(&self, source: &DatabaseSourceConfig) -> Result<(), String> {
        // Simple test query
        let test_query = match source.kind {
            SupportedDatabaseKind::Postgres => "SELECT 1 AS test",
            SupportedDatabaseKind::Mysql => "SELECT 1 AS test",
            SupportedDatabaseKind::Sqlite => "SELECT 1 AS test",
            SupportedDatabaseKind::Bigquery => "SELECT 1 AS test",
            SupportedDatabaseKind::Spanner => "SELECT 1 AS test",
        };

        let test_candidates: Vec<&str> = if source.kind == SupportedDatabaseKind::Bigquery {
            vec!["sql_select", "bigquery-execute-sql"]
        } else {
            vec![source.kind.execute_tool_name()]
        };

        self.call_mcp_tool_value_checked(
            &source.id,
            "test connection",
            &test_candidates,
            json!({ "sql": test_query }),
        )
        .await?;

        Ok(())
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
        sql_dialect: &str,
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
            sql_dialect: sql_dialect.to_string(),
            enabled: true,
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
        let mut columns: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Value>> = Vec::new();

        if let Some(arr) = response.as_array() {
            // Case 1: Raw array of objects (records) - common for BigQuery
            if !arr.is_empty() {
                // Collect all unique keys from all objects to ensure we don't miss any columns
                let mut all_keys = std::collections::BTreeSet::new();
                for row_val in arr {
                    if let Some(obj) = row_val.as_object() {
                        for key in obj.keys() {
                            all_keys.insert(key.clone());
                        }
                    }
                }
                columns = all_keys.into_iter().collect();

                for row_val in arr {
                    if let Some(obj) = row_val.as_object() {
                        let mut row = Vec::new();
                        for key in &columns {
                            row.push(obj.get(key).cloned().unwrap_or(Value::Null));
                        }
                        rows.push(row);
                    }
                }
            }
        } else if let Some(obj) = response.as_object() {
            // Case 2: Structured object with explicit "columns" and "rows"/"data"
            let rows_data = obj
                .get("rows")
                .or_else(|| obj.get("data"))
                .and_then(|r| r.as_array());

            if let Some(arr) = rows_data {
                columns = obj
                    .get("columns")
                    .and_then(|c| c.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                // If columns weren't explicit, collect from all rows
                if columns.is_empty() {
                    let mut all_keys = std::collections::BTreeSet::new();
                    for row_val in arr {
                        if let Some(row_obj) = row_val.as_object() {
                            for key in row_obj.keys() {
                                all_keys.insert(key.clone());
                            }
                        }
                    }
                    columns = all_keys.into_iter().collect();
                }

                for row_val in arr {
                    if let Some(row_obj) = row_val.as_object() {
                        let mut row = Vec::new();
                        for key in &columns {
                            row.push(row_obj.get(key).cloned().unwrap_or(Value::Null));
                        }
                        rows.push(row);
                    } else if let Some(row_arr) = row_val.as_array() {
                        rows.push(row_arr.clone());
                    }
                }
            } else {
                // Case 3: Single record object (e.g. from an aggregation query)
                columns = obj.keys().cloned().collect();
                if !columns.is_empty() {
                    let mut row = Vec::new();
                    for key in &columns {
                        row.push(obj.get(key).cloned().unwrap_or(Value::Null));
                    }
                    rows.push(row);
                }
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolbox_status_default() {
        let status = ToolboxStatus::default();
        assert!(!status.running);
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

    #[test]
    fn test_parse_sql_execution_result_array() {
        let actor = DatabaseToolboxActor::new(
            tokio::sync::mpsc::channel(1).1,
            Arc::new(RwLock::new(DatabaseToolboxState::default())),
            tokio::sync::mpsc::channel(1).0,
        );

        let array_response = json!([
            {"id": 1, "name": "Alice"},
            {"id": 2, "name": "Bob"}
        ]);

        let result = actor.parse_sql_execution_result(&array_response).unwrap();
        assert_eq!(result.columns.len(), 2);
        assert!(result.columns.contains(&"id".to_string()));
        assert!(result.columns.contains(&"name".to_string()));
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.row_count, 2);
    }
}
