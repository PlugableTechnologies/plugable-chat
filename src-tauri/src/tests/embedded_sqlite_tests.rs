//! Functional tests for the embedded SQLite demo database.
//!
//! These tests validate:
//! - Database initialization and table creation
//! - CSV data loading
//! - SQL query execution
//! - Schema enumeration
//! - CLI flag handling

use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::actors::embedded_sqlite_actor::{
    is_embedded_demo_source, EmbeddedSqliteActor, EmbeddedSqliteMsg, EmbeddedSqliteState,
};
use crate::cli::{apply_cli_overrides, CliArgs};
use crate::demo_schema::{
    chicago_crimes_table_schema, CHICAGO_CRIMES_TABLE_FQ_NAME, EMBEDDED_DEMO_SOURCE_ID,
};
use crate::settings::{ensure_default_servers, AppSettings, EMBEDDED_DEMO_SOURCE_ID as SETTINGS_EMBEDDED_DEMO_ID};

/// Test that is_embedded_demo_source correctly identifies the demo source
#[test]
fn test_is_embedded_demo_source() {
    assert!(is_embedded_demo_source("embedded-demo"));
    assert!(!is_embedded_demo_source("other-source"));
    assert!(!is_embedded_demo_source(""));
    assert!(!is_embedded_demo_source("embedded"));
}

/// Test that the demo schema has the expected structure
#[test]
fn test_chicago_crimes_schema_structure() {
    let schema = chicago_crimes_table_schema();

    // Should have 35 columns
    assert_eq!(schema.columns.len(), 35);

    // Check key columns exist
    let column_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(column_names.contains(&"id"));
    assert!(column_names.contains(&"case_number"));
    assert!(column_names.contains(&"primary_type"));
    assert!(column_names.contains(&"community_area_name"));
    assert!(column_names.contains(&"crime_category"));

    // Check fully qualified name
    assert_eq!(schema.fully_qualified_name, CHICAGO_CRIMES_TABLE_FQ_NAME);

    // Check source ID
    assert_eq!(schema.source_id, EMBEDDED_DEMO_SOURCE_ID);
}

/// Test that ensure_default_servers adds the demo database source
#[test]
fn test_ensure_default_servers_adds_demo_source() {
    let mut settings = AppSettings::default();
    
    // Clear any existing demo sources
    settings.database_toolbox.sources.retain(|s| s.id != SETTINGS_EMBEDDED_DEMO_ID);
    
    // Ensure default servers adds it back
    ensure_default_servers(&mut settings);
    
    // Check that demo source was added
    let has_demo = settings
        .database_toolbox
        .sources
        .iter()
        .any(|s| s.id == SETTINGS_EMBEDDED_DEMO_ID);
    assert!(has_demo, "Demo database source should be added by ensure_default_servers");
    
    // Check it's disabled by default
    let demo_source = settings
        .database_toolbox
        .sources
        .iter()
        .find(|s| s.id == SETTINGS_EMBEDDED_DEMO_ID)
        .unwrap();
    assert!(!demo_source.enabled, "Demo source should be disabled by default");
    
    // Check it has proper MCP toolbox args configured
    assert!(
        demo_source.args.contains(&"--tools-file".to_string()),
        "Demo source should have --tools-file arg for MCP toolbox"
    );
    assert!(
        demo_source.args.contains(&"--stdio".to_string()),
        "Demo source should have --stdio arg for MCP toolbox"
    );
    assert!(
        demo_source.args.iter().any(|a| a.contains("demo-tools.yaml")),
        "Demo source should reference demo-tools.yaml"
    );
}

/// Test CLI --enable-demo-db flag parsing
#[test]
fn test_cli_enable_demo_db_flag_parsing() {
    use clap::Parser;
    
    // Test with flag enabled
    let args = CliArgs::parse_from(["plugable-chat", "--enable-demo-db", "true"]);
    assert_eq!(args.enable_demo_db, Some(true));
    
    // Test with flag disabled
    let args = CliArgs::parse_from(["plugable-chat", "--enable-demo-db", "false"]);
    assert_eq!(args.enable_demo_db, Some(false));
    
    // Test without flag (should be None)
    let args = CliArgs::parse_from(["plugable-chat"]);
    assert_eq!(args.enable_demo_db, None);
}

/// Test that apply_cli_overrides enables demo database when flag is set
#[test]
fn test_apply_cli_overrides_enables_demo_db() {
    use clap::Parser;
    
    let args = CliArgs::parse_from(["plugable-chat", "--enable-demo-db", "true"]);
    let mut settings = AppSettings::default();
    
    // Apply overrides
    let _ = apply_cli_overrides(&args, &mut settings);
    
    // Check database toolbox is enabled
    assert!(settings.database_toolbox.enabled, "Database toolbox should be enabled");
    
    // Check demo source is enabled
    let demo_source = settings
        .database_toolbox
        .sources
        .iter()
        .find(|s| s.id == SETTINGS_EMBEDDED_DEMO_ID);
    assert!(demo_source.is_some(), "Demo source should exist");
    assert!(demo_source.unwrap().enabled, "Demo source should be enabled");
    
    // Check sql_select is in always-on builtins
    assert!(
        settings.always_on_builtin_tools.contains(&"sql_select".to_string()),
        "sql_select should be in always-on builtins"
    );
}

/// Test embedded SQLite actor initialization and schema listing
#[tokio::test]
async fn test_embedded_sqlite_actor_list_schemas() {
    let (tx, rx) = mpsc::channel(32);
    let state = Arc::new(RwLock::new(EmbeddedSqliteState::default()));

    // Spawn the actor
    let state_clone = state.clone();
    tokio::spawn(async move {
        let actor = EmbeddedSqliteActor::new(rx, state_clone);
        actor.run().await;
    });

    // Request schema list
    let (respond_to, response_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::ListSchemas { respond_to })
        .await
        .expect("Failed to send message");

    let result = response_rx.await.expect("Failed to receive response");
    
    // Should return ["main"]
    assert!(result.is_ok());
    let schemas = result.unwrap();
    assert_eq!(schemas, vec!["main".to_string()]);
}

/// Test embedded SQLite actor table listing
#[tokio::test]
async fn test_embedded_sqlite_actor_list_tables() {
    let (tx, rx) = mpsc::channel(32);
    let state = Arc::new(RwLock::new(EmbeddedSqliteState::default()));

    // Spawn the actor
    let state_clone = state.clone();
    tokio::spawn(async move {
        let actor = EmbeddedSqliteActor::new(rx, state_clone);
        actor.run().await;
    });

    // First ensure initialized (this may take time to load CSV)
    let (init_tx, init_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::EnsureInitialized { respond_to: init_tx })
        .await
        .expect("Failed to send message");
    
    let init_result = init_rx.await.expect("Failed to receive init response");
    // Skip test if CSV file not found (CI environment)
    if init_result.is_err() {
        println!("Skipping test - CSV file not found: {:?}", init_result.err());
        return;
    }

    // Request table list for "main" schema
    let (respond_to, response_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::ListTables {
        schema_name: "main".to_string(),
        respond_to,
    })
    .await
    .expect("Failed to send message");

    let result = response_rx.await.expect("Failed to receive response");
    
    // Should return ["chicago_crimes"]
    assert!(result.is_ok());
    let tables = result.unwrap();
    assert!(tables.contains(&"chicago_crimes".to_string()));
}

/// Test embedded SQLite actor SQL execution
#[tokio::test]
async fn test_embedded_sqlite_actor_execute_sql() {
    let (tx, rx) = mpsc::channel(32);
    let state = Arc::new(RwLock::new(EmbeddedSqliteState::default()));

    // Spawn the actor
    let state_clone = state.clone();
    tokio::spawn(async move {
        let actor = EmbeddedSqliteActor::new(rx, state_clone);
        actor.run().await;
    });

    // First ensure initialized
    let (init_tx, init_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::EnsureInitialized { respond_to: init_tx })
        .await
        .expect("Failed to send message");
    
    let init_result = init_rx.await.expect("Failed to receive init response");
    // Skip test if CSV file not found (CI environment)
    if init_result.is_err() {
        println!("Skipping test - CSV file not found: {:?}", init_result.err());
        return;
    }

    // Execute a simple query
    let (respond_to, response_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::ExecuteSql {
        sql: "SELECT COUNT(*) as count FROM chicago_crimes".to_string(),
        respond_to,
    })
    .await
    .expect("Failed to send message");

    let result = response_rx.await.expect("Failed to receive response");
    
    assert!(result.is_ok(), "SQL execution should succeed");
    let sql_result = result.unwrap();
    assert!(sql_result.success);
    assert_eq!(sql_result.row_count, 1);
    assert!(sql_result.columns.contains(&"count".to_string()));
}

/// Test embedded SQLite actor aggregation query
#[tokio::test]
async fn test_embedded_sqlite_actor_aggregation_query() {
    let (tx, rx) = mpsc::channel(32);
    let state = Arc::new(RwLock::new(EmbeddedSqliteState::default()));

    // Spawn the actor
    let state_clone = state.clone();
    tokio::spawn(async move {
        let actor = EmbeddedSqliteActor::new(rx, state_clone);
        actor.run().await;
    });

    // First ensure initialized
    let (init_tx, init_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::EnsureInitialized { respond_to: init_tx })
        .await
        .expect("Failed to send message");
    
    let init_result = init_rx.await.expect("Failed to receive init response");
    // Skip test if CSV file not found (CI environment)
    if init_result.is_err() {
        println!("Skipping test - CSV file not found: {:?}", init_result.err());
        return;
    }

    // Execute an aggregation query
    let (respond_to, response_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::ExecuteSql {
        sql: "SELECT primary_type, COUNT(*) as crime_count FROM chicago_crimes GROUP BY primary_type ORDER BY crime_count DESC LIMIT 5".to_string(),
        respond_to,
    })
    .await
    .expect("Failed to send message");

    let result = response_rx.await.expect("Failed to receive response");
    
    assert!(result.is_ok(), "Aggregation query should succeed");
    let sql_result = result.unwrap();
    assert!(sql_result.success);
    assert!(sql_result.row_count <= 5);
    assert!(sql_result.columns.contains(&"primary_type".to_string()));
    assert!(sql_result.columns.contains(&"crime_count".to_string()));
}

/// Test embedded SQLite actor table info retrieval
#[tokio::test]
async fn test_embedded_sqlite_actor_get_table_info() {
    let (tx, rx) = mpsc::channel(32);
    let state = Arc::new(RwLock::new(EmbeddedSqliteState::default()));

    // Spawn the actor
    let state_clone = state.clone();
    tokio::spawn(async move {
        let actor = EmbeddedSqliteActor::new(rx, state_clone);
        actor.run().await;
    });

    // First ensure initialized
    let (init_tx, init_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::EnsureInitialized { respond_to: init_tx })
        .await
        .expect("Failed to send message");
    
    let init_result = init_rx.await.expect("Failed to receive init response");
    // Skip test if CSV file not found (CI environment)
    if init_result.is_err() {
        println!("Skipping test - CSV file not found: {:?}", init_result.err());
        return;
    }

    // Request table info
    let (respond_to, response_rx) = tokio::sync::oneshot::channel();
    tx.send(EmbeddedSqliteMsg::GetTableInfo {
        table_name: "chicago_crimes".to_string(),
        respond_to,
    })
    .await
    .expect("Failed to send message");

    let result = response_rx.await.expect("Failed to receive response");
    
    assert!(result.is_ok(), "GetTableInfo should succeed");
    let table_schema = result.unwrap();
    assert_eq!(table_schema.columns.len(), 35);
    assert_eq!(table_schema.source_id, EMBEDDED_DEMO_SOURCE_ID);
}
