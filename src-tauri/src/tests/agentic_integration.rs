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
        settings.sql_select_enabled = sql_enabled;
        settings.python_execution_enabled = python_enabled;
        settings.tool_call_formats.primary = primary_format;
        settings.tool_call_formats.enabled = vec![primary_format];
        settings.tool_call_formats.normalize();
        
        // Ensure database toolbox is "enabled" if SQL is enabled, so state machine resolves correctly
        if sql_enabled {
            settings.database_toolbox.enabled = true;
            // Also need at least one "enabled" source for determine_available_builtins to work if called
            // though here we are seeding the state machine directly.
        }
        
        settings
    }

    /// Create a state machine from settings
    fn create_state_machine(settings: &AppSettings) -> AgenticStateMachine {
        let filter = ToolLaunchFilter::default();
        let settings_sm = SettingsStateMachine::from_settings(settings, &filter);
        let prompt_context = PromptContext {
            base_prompt: "You are a helpful assistant.".to_string(),
            mcp_context: McpToolContext::default(),
            tool_call_format: settings.tool_call_formats.primary,
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
            },
            ColumnInfo {
                name: "name".to_string(),
                data_type: "TEXT".to_string(),
                nullable: false,
                description: None,
            },
            ColumnInfo {
                name: "total_spend".to_string(),
                data_type: "REAL".to_string(),
                nullable: true,
                description: None,
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
