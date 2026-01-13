//! Integration tests for the Agentic Loop and State Machine
//!
//! These tests validate that the model triggers appropriate tools (sql_select, python_execution)
//! when given specific queries and context.
//!
//! Requirements:
//! - Foundry Local must be running
//! - A model must be loaded (e.g., phi-4-mini)

use std::collections::{HashMap};
use serde_json::json;

use crate::protocol::{
    ChatMessage, 
    parse_tool_calls
};
use crate::settings::{AppSettings, ToolCallFormatName};
use crate::settings_state_machine::{SettingsStateMachine};
use crate::state_machine::{AgenticStateMachine};
use crate::agentic_state::{PromptContext, McpToolContext, StateEvent, TableInfo, ColumnInfo};
use crate::tool_capability::ToolLaunchFilter;

/// Test harness for agentic integration tests
struct AgenticIntegrationTestHarness;

impl AgenticIntegrationTestHarness {
    /// Create minimal settings for testing
    fn create_test_settings(
        sql_enabled: bool,
        python_enabled: bool,
        primary_format: ToolCallFormatName,
    ) -> AppSettings {
        let mut settings = AppSettings::default();
        if sql_enabled {
            settings.always_on_builtin_tools.push("sql_select".to_string());
            settings.always_on_builtin_tools.push("schema_search".to_string());
            settings.database_toolbox.enabled = true;
        }
        if python_enabled {
            settings.always_on_builtin_tools.push("python_execution".to_string());
        }
        settings.tool_call_formats.primary = primary_format;
        settings.tool_call_formats.enabled = vec![primary_format];
        settings.tool_call_formats.normalize();
        
        settings
    }

    /// Create a state machine from settings
    fn create_state_machine(settings: &AppSettings) -> AgenticStateMachine {
        let filter = ToolLaunchFilter::default();
        let settings_sm = SettingsStateMachine::from_settings(settings, &filter);
        let prompt_context = PromptContext {
            base_prompt: "You are a helpful assistant.".to_string(),
            mcp_context: McpToolContext::default(),
            attached_tables: Vec::new(),
            attached_tools: Vec::new(),
            attached_tabular_files: Vec::new(),
            tabular_column_info: Vec::new(),
            tool_call_format: settings.tool_call_formats.primary,
            model_tool_format: None,
            custom_tool_prompts: HashMap::new(),
            python_primary: settings.tool_call_formats.primary == ToolCallFormatName::CodeMode,
            has_attachments: false,
        };
        AgenticStateMachine::new_from_settings_sm(&settings_sm, prompt_context)
    }

    /// Helper to call Foundry Local directly via REST (simulating FoundryActor)
    async fn call_llm(messages: Vec<ChatMessage>) -> Result<String, String> {
        // Dynamic discovery of Foundry port and model
        let (port, model_id) = Self::discover_foundry().await?;
        
        let client = reqwest::Client::new();
        let body = json!({
            "model": model_id,
            "messages": messages.iter().map(|m| json!({
                "role": m.role,
                "content": m.content
            })).collect::<Vec<_>>(),
            "stream": false,
            "temperature": 0.0, // Deterministic for tests
        });

        println!("\n[TestHarness] === SENDING TO MODEL (Port: {}, Model: {}) ===", port, model_id);
        println!("{}", serde_json::to_string_pretty(&body).unwrap_or_else(|_| "Failed to serialize body".to_string()));
        println!("[TestHarness] =====================================================\n");

        let res = client.post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        let status = res.status();
        if !status.is_success() {
            let err_text = res.text().await.unwrap_or_default();
            return Err(format!("Foundry returned error: {} - {}", status, err_text));
        }

        let json: serde_json::Value = res.json().await.map_err(|e| format!("Failed to parse JSON: {}", e))?;
        
        println!("\n[TestHarness] === RECEIVED FROM MODEL ===");
        println!("{}", serde_json::to_string_pretty(&json).unwrap_or_else(|_| "Failed to serialize response".to_string()));
        println!("[TestHarness] ============================\n");

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "Missing content in response".to_string())
    }

    /// Discover running Foundry service and a suitable model
    async fn discover_foundry() -> Result<(u16, String), String> {
        use std::process::Command;
        
        // 1. Detect port via 'foundry service status'
        let output = Command::new("foundry")
            .args(&["service", "status"])
            .output()
            .map_err(|e| format!("Failed to run 'foundry service status': {}", e))?;
            
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut port = None;
        
        for line in stdout.lines() {
            if let Some(start_idx) = line.find("http://127.0.0.1:") {
                let rest = &line[start_idx + "http://127.0.0.1:".len()..];
                let port_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = port_str.parse::<u16>() {
                    port = Some(p);
                    break;
                }
            }
        }
        
        let port = port.ok_or_else(|| "Could not detect Foundry port from 'foundry service status'".to_string())?;
        
        // 2. Find a suitable model (prefer Phi-4)
        let client = reqwest::Client::new();
        let res = client.get(format!("http://127.0.0.1:{}/v1/models", port))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch models: {}", e))?;
            
        let json: serde_json::Value = res.json().await.map_err(|e| format!("Failed to parse models JSON: {}", e))?;
        let models = json["data"].as_array().ok_or_else(|| "Invalid models response".to_string())?;
        
        let model_id = models.iter()
            .filter_map(|m| m["id"].as_str())
            .find(|id| id.to_lowercase().contains("phi-4"))
            .map(|id| id.to_string())
            .ok_or_else(|| "No Phi-4 model found loaded in Foundry".to_string())?;
            
        Ok((port, model_id))
    }
}

#[tokio::test]
async fn test_sql_select_triggering() {
    // 1. Enable SQL
    let settings = AgenticIntegrationTestHarness::create_test_settings(true, false, ToolCallFormatName::Hermes);
    let mut state_machine = AgenticIntegrationTestHarness::create_state_machine(&settings);

    // 2. Mock schema_search finding a table
    let test_table = TableInfo {
        fully_qualified_name: "customers".to_string(),
        source_id: "test_db".to_string(),
        sql_dialect: "sqlite".to_string(),
        description: Some("Customer records with spend data".to_string()),
        relevancy: 0.9,
        columns: vec![
            ColumnInfo {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                description: None,
                special_attributes: vec!["primary_key".to_string()],
                top_values: Vec::new(),
            },
            ColumnInfo {
                name: "name".to_string(),
                data_type: "TEXT".to_string(),
                nullable: false,
                description: None,
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
            ColumnInfo {
                name: "total_spend".to_string(),
                data_type: "REAL".to_string(),
                nullable: true,
                description: None,
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
        ],
    };

    state_machine.handle_event(StateEvent::SchemaSearched {
        tables: vec![test_table],
        max_relevancy: 0.9,
    });

    // 3. Assemble prompt and call model
    let system_prompt = state_machine.build_system_prompt();
    let user_query = "Who are the top 3 customers by spend?";
    
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_query.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    println!("--- SYSTEM PROMPT ---");
    println!("{}", system_prompt);
    println!("--- USER QUERY ---");
    println!("{}", user_query);

    let response = AgenticIntegrationTestHarness::call_llm(messages).await.unwrap();
    println!("--- MODEL RESPONSE ---");
    println!("{}", response);

    // 4. Validate tool call
    let tool_calls = parse_tool_calls(&response);
    
    if tool_calls.is_empty() {
        panic!("Model failed to trigger sql_select! Response: {}", response);
    }

    let sql_call = tool_calls.iter().find(|c| c.tool == "sql_select").expect("No sql_select call found");
    let sql = sql_call.arguments["sql"].as_str().expect("Missing sql argument");
    
    assert!(sql.to_lowercase().contains("customers"), "SQL should contain 'customers'");
    assert!(sql.to_lowercase().contains("total_spend"), "SQL should contain 'total_spend'");
    assert!(sql.to_lowercase().contains("limit 3"), "SQL should contain 'limit 3'");
}

#[tokio::test]
async fn test_python_execution_triggering() {
    // 1. Enable Python in Code Mode
    let settings = AgenticIntegrationTestHarness::create_test_settings(false, true, ToolCallFormatName::CodeMode);
    let state_machine = AgenticIntegrationTestHarness::create_state_machine(&settings);

    // 3. Assemble prompt and call model
    let system_prompt = state_machine.build_system_prompt();
    let user_query = "Calculate the first 10 numbers in the Fibonacci sequence.";
    
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_query.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    println!("--- SYSTEM PROMPT ---");
    println!("{}", system_prompt);
    println!("--- USER QUERY ---");
    println!("{}", user_query);

    let response = AgenticIntegrationTestHarness::call_llm(messages).await.unwrap();
    println!("--- MODEL RESPONSE ---");
    println!("{}", response);

    // 4. Validate tool call (Code Mode uses ```python blocks)
    assert!(response.contains("```python"), "Model failed to trigger python_execution! Response: {}", response);
    assert!(response.contains("def fib"), "Model output should contain a fibonacci function");
}

/// Test that SQL error recovery with schema injection helps the model fix its query.
/// 
/// This is the "Cursor for SQL" approach: when a SQL error occurs, we re-inject the
/// schema directly into the error response so small models don't have to look back
/// in context.
#[tokio::test]
async fn test_sql_error_recovery_with_schema_injection() {
    use crate::system_prompt::build_sql_error_recovery_prompt;
    
    // 1. Enable SQL
    let settings = AgenticIntegrationTestHarness::create_test_settings(true, false, ToolCallFormatName::Hermes);
    let mut state_machine = AgenticIntegrationTestHarness::create_state_machine(&settings);

    // 2. Mock schema_search finding a table with "product_title" (NOT "product")
    let test_table = TableInfo {
        fully_qualified_name: "analytics.sales_summary".to_string(),
        source_id: "test_db".to_string(),
        sql_dialect: "GoogleSQL".to_string(),
        description: Some("Sales summary with product metrics".to_string()),
        relevancy: 0.9,
        columns: vec![
            ColumnInfo {
                name: "product_title".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: Some("Product name/title".to_string()),
                special_attributes: Vec::new(),
                top_values: vec!["Widget A (25%)".to_string(), "Widget B (20%)".to_string()],
            },
            ColumnInfo {
                name: "country_code".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: None,
                special_attributes: Vec::new(),
                top_values: vec!["US (60%)".to_string(), "UK (20%)".to_string(), "CA (10%)".to_string()],
            },
            ColumnInfo {
                name: "revenue".to_string(),
                data_type: "FLOAT".to_string(),
                nullable: true,
                description: Some("Total revenue in USD".to_string()),
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
            ColumnInfo {
                name: "units_sold".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: true,
                description: None,
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
        ],
    };

    state_machine.handle_event(StateEvent::SchemaSearched {
        tables: vec![test_table],
        max_relevancy: 0.9,
    });

    // 3. Get the system prompt and schema context
    let system_prompt = state_machine.build_system_prompt();
    let schema_context = state_machine.get_compact_schema_context();
    
    println!("--- SCHEMA CONTEXT ---");
    println!("{}", schema_context.as_deref().unwrap_or("None"));

    // 4. Build error recovery prompt (simulating a first failed attempt)
    // The model tried to use "product" but that column doesn't exist
    let failed_sql = "SELECT product, SUM(revenue) AS total_revenue FROM analytics.sales_summary WHERE country_code = 'US' GROUP BY product ORDER BY total_revenue DESC LIMIT 5";
    let error_message = "Unrecognized name: product at [1:8]";
    let user_question = "what are my top products by revenue in the US market?";
    
    let error_recovery_prompt = build_sql_error_recovery_prompt(
        failed_sql,
        error_message,
        schema_context.as_deref(),
        user_question,
    );
    
    println!("--- ERROR RECOVERY PROMPT ---");
    println!("{}", error_recovery_prompt);

    // 5. Send to model with the error context
    // This simulates what happens after a first failed attempt
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_question.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "assistant".to_string(),
            content: format!(
                "<tool_call>{{\"name\": \"sql_select\", \"arguments\": {{\"sql\": \"{}\"}}}}</tool_call>",
                failed_sql
            ),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "<tool_response error=\"true\">\n{{\n  \"success\": false,\n  \"error\": \"{}\",\n  \"sql_executed\": \"{}\"\n}}\n</tool_response>\n\n{}",
                error_message, failed_sql, error_recovery_prompt
            ),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    println!("--- SENDING ERROR RECOVERY CONTEXT TO MODEL ---");
    
    let response = AgenticIntegrationTestHarness::call_llm(messages).await.unwrap();
    println!("--- MODEL RESPONSE AFTER ERROR ---");
    println!("{}", response);

    // 6. Validate: model should now use correct column name "product_title"
    let tool_calls = parse_tool_calls(&response);
    
    if tool_calls.is_empty() {
        // Model might respond with text explaining it will fix the query
        // Check if it at least mentions the correct column
        assert!(
            response.to_lowercase().contains("product_title"),
            "Model should reference 'product_title' in recovery. Response: {}",
            response
        );
        return;
    }

    let sql_call = tool_calls.iter().find(|c| c.tool == "sql_select")
        .expect("Model should retry sql_select after error");
    let sql = sql_call.arguments["sql"].as_str()
        .expect("Missing sql argument");
    
    println!("--- CORRECTED SQL ---");
    println!("{}", sql);

    // The model should now use "product_title" instead of "product"
    assert!(
        sql.to_lowercase().contains("product_title"),
        "Model should use correct column 'product_title' after error recovery. SQL: {}",
        sql
    );
    
    // It should NOT use the invalid "product" column (but may use it as alias)
    // Check that it's not using "product" as a column name without "product_title"
    let sql_lower = sql.to_lowercase();
    if sql_lower.contains("product") && !sql_lower.contains("product_title") {
        panic!(
            "Model should NOT use invalid 'product' column without 'product_title'. SQL: {}",
            sql
        );
    }
}

/// Test SQL error recovery when the model queries the WRONG TABLE.
/// This is the scenario from the user's logs: two tables exist, model picks wrong one.
/// 
/// Uses Chain-of-Thought prompting to force the model to reason about which table
/// has the column it needs.
#[tokio::test]
async fn test_sql_error_recovery_wrong_table() {
    use crate::system_prompt::build_sql_error_recovery_prompt;
    
    let settings = AgenticIntegrationTestHarness::create_test_settings(true, false, ToolCallFormatName::Hermes);
    let mut state_machine = AgenticIntegrationTestHarness::create_state_machine(&settings);

    // Two tables: 
    // - account_summary: NO "product" column (what model wrongly queried)
    // - product_summary: HAS "product" column (what model should query)
    let account_summary_table = TableInfo {
        fully_qualified_name: "analytics.account_summary".to_string(),
        source_id: "test_db".to_string(),
        sql_dialect: "GoogleSQL".to_string(),
        description: Some("Account-level summary metrics".to_string()),
        relevancy: 0.85,
        columns: vec![
            ColumnInfo {
                name: "account_name".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: Some("Account name".to_string()),
                special_attributes: Vec::new(),
                top_values: vec!["Acme Corp (25%)".to_string(), "Beta Inc (20%)".to_string()],
            },
            ColumnInfo {
                name: "country_code".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: None,
                special_attributes: Vec::new(),
                top_values: vec!["US (60%)".to_string(), "UK (20%)".to_string()],
            },
            ColumnInfo {
                name: "total_revenue".to_string(),
                data_type: "FLOAT".to_string(),
                nullable: true,
                description: Some("Total account revenue".to_string()),
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
        ],
    };

    let product_summary_table = TableInfo {
        fully_qualified_name: "analytics.product_summary".to_string(),
        source_id: "test_db".to_string(),
        sql_dialect: "GoogleSQL".to_string(),
        description: Some("Product-level summary metrics".to_string()),
        relevancy: 0.90,
        columns: vec![
            ColumnInfo {
                name: "product".to_string(),  // THIS is the column the model wants
                data_type: "STRING".to_string(),
                nullable: false,
                description: Some("Product identifier".to_string()),
                special_attributes: Vec::new(),
                top_values: vec!["B0779K9DG2 (5%)".to_string(), "B08B6CZ29Q (4%)".to_string()],
            },
            ColumnInfo {
                name: "product_title".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: Some("Product name/title".to_string()),
                special_attributes: Vec::new(),
                top_values: vec!["Widget A (3%)".to_string(), "Widget B (2%)".to_string()],
            },
            ColumnInfo {
                name: "country_code".to_string(),
                data_type: "STRING".to_string(),
                nullable: false,
                description: None,
                special_attributes: Vec::new(),
                top_values: vec!["US (60%)".to_string(), "UK (20%)".to_string()],
            },
            ColumnInfo {
                name: "revenue".to_string(),
                data_type: "FLOAT".to_string(),
                nullable: true,
                description: Some("Product revenue".to_string()),
                special_attributes: Vec::new(),
                top_values: Vec::new(),
            },
        ],
    };

    // Register BOTH tables
    state_machine.handle_event(StateEvent::SchemaSearched {
        tables: vec![account_summary_table, product_summary_table],
        max_relevancy: 0.90,
    });

    let system_prompt = state_machine.build_system_prompt();
    let schema_context = state_machine.get_compact_schema_context();
    
    println!("--- MULTI-TABLE SCHEMA CONTEXT ---");
    println!("{}", schema_context.as_deref().unwrap_or("None"));
    
    // Verify we have both tables in the schema context
    let ctx = schema_context.as_deref().unwrap();
    assert!(ctx.contains("account_summary"), "Should have account_summary table");
    assert!(ctx.contains("product_summary"), "Should have product_summary table");
    assert!(ctx.matches("**Table:").count() >= 2, "Should have at least 2 tables formatted");

    // The model wrongly queried account_summary for "product" column
    let failed_sql = "SELECT product, SUM(total_revenue) AS revenue FROM analytics.account_summary WHERE country_code = 'US' GROUP BY product ORDER BY revenue DESC LIMIT 5";
    let error_message = "Unrecognized name: product at [1:8]";
    let user_question = "what are my top products by revenue in the US?";
    
    let error_recovery_prompt = build_sql_error_recovery_prompt(
        failed_sql,
        error_message,
        schema_context.as_deref(),
        user_question,
    );
    
    println!("--- ERROR RECOVERY PROMPT (CoT) ---");
    println!("{}", error_recovery_prompt);
    
    // Verify CoT elements are present
    assert!(error_recovery_prompt.contains("STOP AND ANALYZE"), "Should have CoT header");
    assert!(error_recovery_prompt.contains("BEFORE you retry"), "Should have CoT instruction");
    assert!(error_recovery_prompt.contains("WRONG table"), "Should warn about wrong table");
    assert!(error_recovery_prompt.contains("product"), "Should mention the missing column");

    // Build conversation with the error
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt.clone(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_question.to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "assistant".to_string(),
            content: format!(
                "<tool_call>{{\"name\": \"sql_select\", \"arguments\": {{\"sql\": \"{}\"}}}}</tool_call>",
                failed_sql
            ),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "<tool_response error=\"true\">\n{{\n  \"success\": false,\n  \"error\": \"{}\",\n  \"sql_executed\": \"{}\"\n}}\n</tool_response>\n\n{}",
                error_message, failed_sql, error_recovery_prompt
            ),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    println!("--- SENDING TO MODEL ---");
    
    let response = AgenticIntegrationTestHarness::call_llm(messages).await.unwrap();
    println!("--- MODEL RESPONSE ---");
    println!("{}", response);

    // The model should now query the CORRECT table: product_summary
    let tool_calls = parse_tool_calls(&response);
    
    if tool_calls.is_empty() {
        // Model explained but didn't call - check it mentions the right table
        assert!(
            response.to_lowercase().contains("product_summary"),
            "Model should reference 'product_summary' as the correct table. Response: {}",
            response
        );
        return;
    }

    let sql_call = tool_calls.iter().find(|c| c.tool == "sql_select")
        .expect("Model should retry sql_select after error");
    let sql = sql_call.arguments["sql"].as_str()
        .expect("Missing sql argument");
    
    println!("--- CORRECTED SQL ---");
    println!("{}", sql);

    // The model should now query product_summary, NOT account_summary
    let sql_lower = sql.to_lowercase();
    assert!(
        sql_lower.contains("product_summary"),
        "Model should query correct table 'product_summary' after error recovery. SQL: {}",
        sql
    );
    assert!(
        !sql_lower.contains("account_summary"),
        "Model should NOT query wrong table 'account_summary' after error recovery. SQL: {}",
        sql
    );
}

/// Test the compact schema context extraction from state machine
#[test]
fn test_get_compact_schema_context() {
    let settings = AgenticIntegrationTestHarness::create_test_settings(true, false, ToolCallFormatName::Hermes);
    let mut state_machine = AgenticIntegrationTestHarness::create_state_machine(&settings);

    // Initially, no schema context should be available
    assert!(state_machine.get_compact_schema_context().is_none());

    // Add a table via state event
    let test_table = TableInfo {
        fully_qualified_name: "test.users".to_string(),
        source_id: "test_db".to_string(),
        sql_dialect: "sqlite".to_string(),
        description: None,
        relevancy: 0.9,
        columns: vec![
            ColumnInfo {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                description: None,
                special_attributes: vec!["primary_key".to_string()],
                top_values: Vec::new(),
            },
            ColumnInfo {
                name: "email".to_string(),
                data_type: "TEXT".to_string(),
                nullable: false,
                description: None,
                special_attributes: Vec::new(),
                top_values: vec!["user@example.com (5%)".to_string()],
            },
        ],
    };

    state_machine.handle_event(StateEvent::SchemaSearched {
        tables: vec![test_table],
        max_relevancy: 0.9,
    });

    // Now schema context should be available
    let schema_context = state_machine.get_compact_schema_context();
    assert!(schema_context.is_some(), "Schema context should be available after SchemaSearched event");
    
    let ctx = schema_context.unwrap();
    println!("Schema context:\n{}", ctx);
    
    // Should contain column names and types
    assert!(ctx.contains("id"), "Schema should contain 'id' column");
    assert!(ctx.contains("INTEGER"), "Schema should contain 'INTEGER' type");
    assert!(ctx.contains("email"), "Schema should contain 'email' column");
    assert!(ctx.contains("TEXT"), "Schema should contain 'TEXT' type");
}
