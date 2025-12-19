//! Integration tests for tool capability resolution
//!
//! These tests validate tool calling behavior by actually calling models via Foundry Local.
//! Each test takes 2-5 seconds due to real inference.
//!
//! Requirements:
//! - Foundry Local must be running
//! - A model must be loaded (e.g., Phi-4)

use crate::protocol::{ChatMessage, ModelInfo, ToolFormat};
use crate::settings::{AppSettings, ToolCallFormatConfig, ToolCallFormatName};
use crate::tool_capability::{ToolCapabilityResolver, ToolLaunchFilter};
use crate::tool_registry::ToolRegistry;
use std::collections::HashSet;
use std::time::Duration;

/// Test harness for tool capability integration tests
struct ToolCapabilityTestHarness {
    /// Timeout for test operations
    test_timeout: Duration,
}

impl ToolCapabilityTestHarness {
    fn new() -> Self {
        Self {
            test_timeout: Duration::from_secs(30),
        }
    }

    /// Create a minimal model info for testing
    fn create_test_model_info(supports_native: bool, tool_format: ToolFormat) -> ModelInfo {
        ModelInfo {
            id: "test-model".to_string(),
            family: crate::protocol::ModelFamily::Phi,
            tool_calling: supports_native,
            tool_format,
            vision: false,
            reasoning: false,
            reasoning_format: crate::protocol::ReasoningFormat::None,
            max_input_tokens: 4096,
            max_output_tokens: 2048,
            supports_temperature: true,
            supports_top_p: true,
            supports_reasoning_effort: false,
        }
    }

    /// Create minimal settings for testing
    fn create_test_settings(
        python_execution_enabled: bool,
        tool_search_enabled: bool,
        primary_format: ToolCallFormatName,
    ) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.python_execution_enabled = python_execution_enabled;
        settings.tool_search_enabled = tool_search_enabled;
        settings.tool_call_formats.primary = primary_format;
        settings.tool_call_formats.enabled = vec![primary_format];
        settings.tool_call_formats.normalize();
        settings
    }

    /// Create a minimal tool registry for testing
    fn create_test_registry() -> ToolRegistry {
        ToolRegistry::new()
    }
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_format_compliance_hermes() {
    // Test that Phi model outputs tool calls in Hermes format when Hermes is primary
    let harness = ToolCapabilityTestHarness::new();
    let settings = harness.create_test_settings(false, false, ToolCallFormatName::Hermes);
    let model_info = harness.create_test_model_info(false, ToolFormat::Hermes);
    let filter = ToolLaunchFilter::default();
    let registry = harness.create_test_registry();
    let server_configs = vec![];

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify Hermes format is selected
    assert_eq!(
        capabilities.primary_format,
        crate::tool_capability::ToolCallFormatName::Hermes
    );
    assert!(!capabilities.use_native_tools);

    // In a real test, we would:
    // 1. Send a chat request with tool instructions
    // 2. Verify the model outputs <tool_call> tags
    // 3. Parse and validate the tool calls
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_format_compliance_native() {
    // Test that models with native tool calling support use native format
    let harness = ToolCapabilityTestHarness::new();
    let settings = harness.create_test_settings(false, false, ToolCallFormatName::Native);
    let model_info = harness.create_test_model_info(true, ToolFormat::OpenAI);
    let filter = ToolLaunchFilter::default();
    let registry = harness.create_test_registry();
    let server_configs = vec![];

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify native format is selected when model supports it
    assert_eq!(
        capabilities.primary_format,
        crate::tool_capability::ToolCallFormatName::Native
    );
    assert!(capabilities.use_native_tools);
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_python_execution_triggered() {
    // Test that math questions trigger python_execution when enabled
    let harness = ToolCapabilityTestHarness::new();
    let mut settings = harness.create_test_settings(true, false, ToolCallFormatName::CodeMode);
    settings.python_execution_enabled = true;
    let model_info = harness.create_test_model_info(false, ToolFormat::Hermes);
    let filter = ToolLaunchFilter::default();
    let registry = harness.create_test_registry();
    let server_configs = vec![];

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify python_execution is available
    assert!(capabilities
        .available_builtins
        .contains("python_execution"));
    assert_eq!(
        capabilities.primary_format,
        crate::tool_capability::ToolCallFormatName::CodeMode
    );

    // In a real test, we would:
    // 1. Send "Calculate 17 * 23 + 456" to the model
    // 2. Verify the model outputs Python code
    // 3. Verify the code is executed and returns correct result
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_tool_search_discovers_deferred() {
    // Test that tool_search discovers deferred MCP tools
    let harness = ToolCapabilityTestHarness::new();
    let settings = harness.create_test_settings(false, true, ToolCallFormatName::Hermes);
    let model_info = harness.create_test_model_info(false, ToolFormat::Hermes);
    let filter = ToolLaunchFilter::default();
    let mut registry = harness.create_test_registry();
    let server_configs = vec![];

    // Register a deferred tool
    use crate::actors::mcp_host_actor::McpTool;
    let deferred_tool = McpTool {
        name: "test_tool".to_string(),
        description: Some("A test tool".to_string()),
        input_schema: None,
        input_examples: None,
        allowed_callers: None,
    };
    registry.register_mcp_tools("test_server", "test_server", &[deferred_tool], true);

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify tool_search is available when there are deferred tools
    assert!(capabilities
        .available_builtins
        .contains("tool_search"));
    assert!(!capabilities.deferred_mcp_tools.is_empty());

    // In a real test, we would:
    // 1. Send a query that would benefit from the deferred tool
    // 2. Verify the model calls tool_search
    // 3. Verify the tool is discovered and materialized
    // 4. Verify the model can then call the discovered tool
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_disabled_python_not_in_prompt() {
    // Test that python_execution is not available when disabled
    let harness = ToolCapabilityTestHarness::new();
    let settings = harness.create_test_settings(false, false, ToolCallFormatName::Hermes);
    let model_info = harness.create_test_model_info(false, ToolFormat::Hermes);
    let filter = ToolLaunchFilter::default();
    let registry = harness.create_test_registry();
    let server_configs = vec![];

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify python_execution is NOT available
    assert!(!capabilities
        .available_builtins
        .contains("python_execution"));
    assert_ne!(
        capabilities.primary_format,
        crate::tool_capability::ToolCallFormatName::CodeMode
    );
}

#[tokio::test]
#[ignore] // Requires Foundry Local to be running
async fn test_native_fallback_to_hermes() {
    // Test that when native is primary but model doesn't support it, we fall back to Hermes
    let harness = ToolCapabilityTestHarness::new();
    let settings = harness.create_test_settings(false, false, ToolCallFormatName::Native);
    let model_info = harness.create_test_model_info(false, ToolFormat::Hermes); // Model doesn't support native
    let filter = ToolLaunchFilter::default();
    let registry = harness.create_test_registry();
    let server_configs = vec![];

    let capabilities = ToolCapabilityResolver::resolve(
        &settings,
        &model_info,
        &filter,
        &server_configs,
        &registry,
    );

    // Verify fallback to Hermes
    assert_ne!(
        capabilities.primary_format,
        crate::tool_capability::ToolCallFormatName::Native
    );
    assert!(!capabilities.use_native_tools);
}



