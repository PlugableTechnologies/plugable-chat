//! Model Profiles - Regex-based model matching with per-profile prompt building and parsing
//!
//! This module provides a `ModelProfile` abstraction that allows plugable-chat to handle
//! different model families without hardcoding model IDs in the core logic.
//!
//! Each profile provides:
//! - A regex pattern to match model names
//! - A `build_prompt` function to construct model-appropriate prompts
//! - A `parse_tool_calls` function to extract tool calls from model output

use regex::Regex;
use serde_json::{json, Value};
use crate::protocol::{
    ModelFamily, ToolFormat, ChatMessage, OpenAITool, ParsedToolCall,
    ToolSchema, PromptOptions, ModelInput, ReasoningStyle,
};
use crate::tool_adapters::{
    parse_hermes_tool_calls, parse_granite_tool_calls, parse_gemini_tool_calls,
};

/// A model profile that defines how to interact with a specific model family
pub struct ModelProfile {
    /// Unique identifier for this profile
    pub id: &'static str,
    /// Regex pattern to match model names (case-insensitive)
    pattern: Regex,
    /// Model family for format-specific handling
    pub family: ModelFamily,
    /// Tool calling format
    pub tool_format: ToolFormat,
}

impl ModelProfile {
    /// Create a new model profile
    fn new(id: &'static str, pattern: &str, family: ModelFamily, tool_format: ToolFormat) -> Self {
        Self {
            id,
            pattern: Regex::new(&format!("(?i){}", pattern)).expect("Invalid regex pattern"),
            family,
            tool_format,
        }
    }
    
    /// Check if this profile matches the given model name
    pub fn matches(&self, model_name: &str) -> bool {
        self.pattern.is_match(model_name)
    }
    
    /// Build a prompt for this model profile
    ///
    /// This generates the appropriate messages and tool configuration based on:
    /// - The model family's expected format
    /// - Whether tools are available
    /// - The reasoning style preference
    pub fn build_prompt(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        match self.family {
            ModelFamily::GptOss => self.build_prompt_openai_style(history, tools, options),
            ModelFamily::Phi => self.build_prompt_phi(history, tools, options),
            ModelFamily::Granite => self.build_prompt_granite(history, tools, options),
            ModelFamily::Gemma => self.build_prompt_gemma_like(history, tools, options),
            ModelFamily::Generic => self.build_prompt_generic(history, tools, options),
        }
    }
    
    /// Parse tool calls from model output
    pub fn parse_tool_calls(&self, output: &str) -> Vec<ParsedToolCall> {
        match self.tool_format {
            ToolFormat::OpenAI => {
                // OpenAI format typically uses structured responses, but we can fall back to Hermes
                parse_hermes_tool_calls(output)
            }
            ToolFormat::Hermes => parse_hermes_tool_calls(output),
            ToolFormat::Granite => parse_granite_tool_calls(output),
            ToolFormat::Gemini => parse_gemini_tool_calls(output),
            ToolFormat::TextBased => {
                // Try Hermes-style first, then fall back to generic JSON detection
                let calls = parse_hermes_tool_calls(output);
                if !calls.is_empty() {
                    calls
                } else {
                    self.parse_generic_json_tool_calls(output)
                }
            }
        }
    }
    
    // ========== OpenAI-Style Prompt Building (Qwen, GPT-OSS, LLaMA-Instruct) ==========
    
    fn build_prompt_openai_style(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        let mut messages = Vec::new();
        
        // Build system prompt based on tools availability
        let system_content = if options.tools_available && !tools.is_empty() {
            self.build_tool_system_prompt_openai(tools, options)
        } else {
            "You are a helpful assistant.".to_string()
        };
        
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_content,
        });
        
        // Add history (skip existing system messages)
        for msg in history {
            if msg.role != "system" {
                messages.push(msg.clone());
            }
        }
        
        // Convert tools to OpenAI format if available
        let openai_tools = if options.tools_available && !tools.is_empty() {
            Some(tools.iter().map(|t| self.tool_schema_to_openai(t)).collect())
        } else {
            None
        };
        
        ModelInput {
            messages,
            tools: openai_tools,
            extra_params: json!({}),
        }
    }
    
    fn build_tool_system_prompt_openai(&self, tools: &[ToolSchema], options: &PromptOptions) -> String {
        let mut prompt = String::from("You are a tool-using assistant. Use available tools when appropriate.\n\n");
        
        if options.code_mode_enabled {
            prompt.push_str(&Self::python_execution_explanation());
        }
        
        // Add reasoning style instruction
        match options.reasoning_style {
            ReasoningStyle::EncourageCot => {
                prompt.push_str("Think step by step before using tools or providing answers.\n\n");
            }
            ReasoningStyle::SuppressCot => {
                prompt.push_str("Provide concise, direct responses.\n\n");
            }
            ReasoningStyle::Default => {}
        }
        
        prompt.push_str("## Available Tools\n\n");
        for tool in tools.iter().filter(|t| !t.defer_loading) {
            prompt.push_str(&format!("**{}**", tool.name));
            if let Some(desc) = &tool.description {
                prompt.push_str(&format!(": {}", desc));
            }
            prompt.push('\n');
        }
        
        prompt
    }
    
    /// Shared python execution explanation for all model profiles
    fn python_execution_explanation() -> String {
        r#"## Python Execution

`python_execution` runs Python in a secure sandbox. Use for:
- Math/calculations (deterministic results vs token generation)
- String manipulation and data transformations
- Multi-step logic with conditionals

**IMPORTANT: You must `import` modules before using them.**
Allowed imports: math, json, random, re, datetime, collections, itertools, functools, statistics, decimal, fractions, hashlib, base64, operator, string, textwrap, copy, types, typing, abc, numbers, binascii, html.
Not available: pandas, numpy, requests, or any external packages.

**Format:** `{"name": "python_execution", "arguments": {"code": ["import math", "result = math.pi", "print(result)"]}}`

**Example:**
```python
import math
import statistics
data = [23, 45, 67, 89, 12]
print(f"Mean: {statistics.mean(data):.1f}, Sum: {math.fsum(data)}")
```

## Tool Discovery

Use `tool_search` to discover MCP tools, which then become available as async Python functions in python_execution.

"#.to_string()
    }
    
    // ========== Phi-Style Prompt Building ==========
    
    fn build_prompt_phi(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        // Phi models use Hermes-style tool calling with <tool_call> tags
        let mut messages = Vec::new();
        
        let system_content = if options.tools_available && !tools.is_empty() {
            self.build_tool_system_prompt_hermes(tools, options)
        } else {
            "You are a helpful assistant.".to_string()
        };
        
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_content,
        });
        
        for msg in history {
            if msg.role != "system" {
                messages.push(msg.clone());
            }
        }
        
        // Phi can use native tools via Hermes format
        let openai_tools = if options.tools_available && !tools.is_empty() {
            Some(tools.iter().map(|t| self.tool_schema_to_openai(t)).collect())
        } else {
            None
        };
        
        ModelInput {
            messages,
            tools: openai_tools,
            extra_params: json!({}),
        }
    }
    
    fn build_tool_system_prompt_hermes(&self, tools: &[ToolSchema], options: &PromptOptions) -> String {
        let mut prompt = String::from("You are a helpful assistant with tool-calling capabilities.\n\n");
        
        prompt.push_str("## Tool Calling Format\n\n");
        prompt.push_str("When you need to use a tool, output ONLY:\n");
        prompt.push_str("<tool_call>{\"name\": \"tool_name\", \"arguments\": {...}}</tool_call>\n\n");
        
        if options.code_mode_enabled {
            prompt.push_str(&Self::python_execution_explanation());
        }
        
        prompt.push_str("## Available Tools\n\n");
        for tool in tools.iter().filter(|t| !t.defer_loading) {
            self.append_tool_description(&mut prompt, tool);
        }
        
        prompt
    }
    
    // ========== Granite-Style Prompt Building ==========
    
    fn build_prompt_granite(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        let mut messages = Vec::new();
        
        let system_content = if options.tools_available && !tools.is_empty() {
            self.build_tool_system_prompt_granite(tools, options)
        } else {
            "You are a helpful assistant.".to_string()
        };
        
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_content,
        });
        
        for msg in history {
            if msg.role != "system" {
                messages.push(msg.clone());
            }
        }
        
        // Granite uses <function_call> format, but can also accept OpenAI-style tools
        let openai_tools = if options.tools_available && !tools.is_empty() {
            Some(tools.iter().map(|t| self.tool_schema_to_openai(t)).collect())
        } else {
            None
        };
        
        ModelInput {
            messages,
            tools: openai_tools,
            extra_params: json!({"repetition_penalty": 1.05}),
        }
    }
    
    fn build_tool_system_prompt_granite(&self, tools: &[ToolSchema], options: &PromptOptions) -> String {
        let mut prompt = String::from("You are a helpful assistant with function calling capabilities.\n\n");
        
        prompt.push_str("## Function Calling Format\n\n");
        prompt.push_str("When you need to call a function, output:\n");
        prompt.push_str("<function_call>{\"name\": \"function_name\", \"arguments\": {...}}</function_call>\n\n");
        
        if options.code_mode_enabled {
            prompt.push_str(&Self::python_execution_explanation());
        }
        
        prompt.push_str("## Available Functions\n\n");
        for tool in tools.iter().filter(|t| !t.defer_loading) {
            self.append_tool_description(&mut prompt, tool);
        }
        
        prompt
    }
    
    // ========== Gemma-Like Prompt Building (Text-based tool calling) ==========
    
    fn build_prompt_gemma_like(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        let mut messages = Vec::new();
        
        let system_content = if options.tools_available && !tools.is_empty() {
            self.build_tool_system_prompt_text_based(tools, options)
        } else {
            "You are a helpful assistant.".to_string()
        };
        
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_content,
        });
        
        for msg in history {
            if msg.role != "system" {
                messages.push(msg.clone());
            }
        }
        
        // Gemma doesn't support native tool calling, use text-based only
        ModelInput {
            messages,
            tools: None,
            extra_params: json!({"top_k": 40}),
        }
    }
    
    fn build_tool_system_prompt_text_based(&self, tools: &[ToolSchema], options: &PromptOptions) -> String {
        let mut prompt = String::from("You are a helpful assistant.\n\n");
        
        prompt.push_str("## Tool Calling\n\n");
        prompt.push_str("When you need to use a tool, output ONLY in this exact format:\n");
        prompt.push_str("<function_call>{\"name\": \"tool_name\", \"arguments\": {\"arg1\": \"value1\"}}</function_call>\n\n");
        prompt.push_str("Do not include any other text when making a tool call.\n\n");
        
        if options.code_mode_enabled {
            prompt.push_str(&Self::python_execution_explanation());
        }
        
        // Include full JSON schemas for text-based models
        let tool_schemas: Vec<Value> = tools.iter()
            .filter(|t| !t.defer_loading)
            .map(|t| json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            }))
            .collect();
        
        prompt.push_str("## Available Tools (JSON Schema)\n\n```json\n");
        prompt.push_str(&serde_json::to_string_pretty(&tool_schemas).unwrap_or_default());
        prompt.push_str("\n```\n");
        
        prompt
    }
    
    // ========== Generic/Fallback Prompt Building ==========
    
    fn build_prompt_generic(
        &self,
        history: &[ChatMessage],
        tools: &[ToolSchema],
        options: &PromptOptions,
    ) -> ModelInput {
        // Use Hermes-style as a reasonable default
        self.build_prompt_phi(history, tools, options)
    }
    
    // ========== Helper Methods ==========
    
    fn tool_schema_to_openai(&self, tool: &ToolSchema) -> OpenAITool {
        OpenAITool {
            tool_type: "function".to_string(),
            function: crate::protocol::OpenAIFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.parameters.clone()),
            },
        }
    }
    
    fn append_tool_description(&self, prompt: &mut String, tool: &ToolSchema) {
        prompt.push_str(&format!("**{}**", tool.name));
        if let Some(desc) = &tool.description {
            prompt.push_str(&format!(": {}", desc));
        }
        prompt.push('\n');
        
        if let Some(properties) = tool.parameters.get("properties").and_then(|p| p.as_object()) {
            let required: Vec<&str> = tool.parameters.get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            
            for (name, schema) in properties {
                let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                let desc = schema.get("description").and_then(|d| d.as_str()).unwrap_or("");
                let req_marker = if required.contains(&name.as_str()) { " [required]" } else { "" };
                prompt.push_str(&format!("  - `{}` ({}){}: {}\n", name, type_str, req_marker, desc));
            }
        }
        prompt.push('\n');
    }
    
    /// Try to parse tool calls as raw JSON objects (fallback for text-based models)
    fn parse_generic_json_tool_calls(&self, output: &str) -> Vec<ParsedToolCall> {
        let mut calls = Vec::new();
        
        // Try to find a standalone JSON object that looks like a tool call
        let trimmed = output.trim();
        
        // Check if the entire output is a JSON object with "name" field
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                if let Some(name) = parsed.get("name").and_then(|n| n.as_str()) {
                    let arguments = parsed.get("arguments")
                        .cloned()
                        .unwrap_or(json!({}));
                    
                    // Parse server___tool format if present
                    let (server, tool) = if name.contains("___") {
                        let parts: Vec<&str> = name.splitn(2, "___").collect();
                        (parts[0].to_string(), parts[1].to_string())
                    } else {
                        ("unknown".to_string(), name.to_string())
                    };
                    
                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw: trimmed.to_string(),
                    });
                }
            }
        }
        
        calls
    }
}

// ========== Profile Registry ==========

// Static registry of all known model profiles
lazy_static::lazy_static! {
    static ref PROFILES: Vec<ModelProfile> = vec![
        // OpenAI-style models (Qwen, GPT-OSS, LLaMA-Instruct)
        ModelProfile::new(
            "openai_style",
            r"qwen|gpt-oss|llama.*instruct|mistral.*instruct",
            ModelFamily::GptOss,
            ToolFormat::Hermes,
        ),
        // IBM Granite models
        ModelProfile::new(
            "granite",
            r"granite",
            ModelFamily::Granite,
            ToolFormat::Granite,
        ),
        // Microsoft Phi models
        ModelProfile::new(
            "phi",
            r"phi",
            ModelFamily::Phi,
            ToolFormat::Hermes,
        ),
        // Google Gemma models
        ModelProfile::new(
            "gemma",
            r"gemma",
            ModelFamily::Gemma,
            ToolFormat::TextBased,
        ),
    ];
    
    static ref DEFAULT_PROFILE: ModelProfile = ModelProfile::new(
        "default",
        r".*",  // Matches everything
        ModelFamily::Generic,
        ToolFormat::Hermes,
    );
}

/// Resolve the appropriate profile for a given model name
pub fn resolve_profile(model_name: &str) -> &'static ModelProfile {
    for profile in PROFILES.iter() {
        if profile.matches(model_name) {
            println!("[ModelProfiles] Resolved '{}' to profile '{}'", model_name, profile.id);
            return profile;
        }
    }
    
    println!("[ModelProfiles] Using default profile for '{}'", model_name);
    &DEFAULT_PROFILE
}

/// Get all registered profiles
pub fn all_profiles() -> &'static [ModelProfile] {
    &PROFILES
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_profile_matching() {
        // Test Qwen matching
        let profile = resolve_profile("Qwen2.5-32B-Instruct");
        assert_eq!(profile.id, "openai_style");
        
        // Test Granite matching
        let profile = resolve_profile("granite-3b-code-instruct");
        assert_eq!(profile.id, "granite");
        
        // Test Phi matching
        let profile = resolve_profile("Phi-4-generic-gpu:1");
        assert_eq!(profile.id, "phi");
        
        // Test Gemma matching
        let profile = resolve_profile("gemma-2-9b-it");
        assert_eq!(profile.id, "gemma");
        
        // Test fallback
        let profile = resolve_profile("unknown-model");
        assert_eq!(profile.id, "default");
    }
    
    #[test]
    fn test_tool_schema_caller_check() {
        let mut tool = ToolSchema::new("test_tool");
        
        // No restrictions
        assert!(tool.can_be_called_by(None));
        assert!(tool.can_be_called_by(Some("python_execution_20251206")));
        
        // With restrictions
        tool.allowed_callers = Some(vec!["python_execution_20251206".to_string()]);
        assert!(!tool.can_be_called_by(None));
        assert!(tool.can_be_called_by(Some("python_execution_20251206")));
        assert!(!tool.can_be_called_by(Some("other_caller")));
    }
}

