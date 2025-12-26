//! Centralized system prompt generation for Plugable Chat.
//!
//! This module serves as the single source of truth for all LLM prompt content,
//! consolidating guidance, format-specific syntax, and tool documentation.

use std::collections::HashSet;
use crate::agentic_state::{Capability, McpToolInfo, TableInfo, RagChunk};
use crate::protocol::{ToolSchema, ToolFormat};
use crate::settings::ToolCallFormatName;
use crate::tool_registry::ToolSearchResult;

// ============ SQL Guidance Constants ============

/// Core SQL execution rules
pub const SQL_CORE_RULES: &str = "\
- Execute queries to answer data questions - do NOT return SQL code
- ONLY use columns explicitly listed in the schema above
- NEVER guess column names - if not listed, it doesn't exist
- Prefer aggregation (SUM, COUNT, AVG, etc.) of numeric columns to get final answers directly
- Limit results to 25 rows or less through the design of the query
- Avoid TO_CHAR function; use CAST(column AS STRING) instead
- NEVER display SQL code to the user - only show query results
- If a query fails, read the error and fix it rather than inventing results";

/// Anti-hallucination rules for SQL
pub const SQL_NO_HALLUCINATION: &str = "\
- ONLY use columns explicitly listed in the `cols:` section above.
- NEVER guess or invent column names.
- If a query returns `null`, the data is missing - do NOT assume the tool failed.
- NEVER make up data values. All facts MUST come from executing `sql_select`.
- NEVER display SQL code to the user - only show query results.
- If the query fails, read the error and fix it rather than inventing results.";

/// Success guidance for sql_select (post-execution)
pub const SQL_SUCCESS_GUIDANCE: &str = "\n\n**NOTE**: The query results above have already been displayed to the user in a formatted table. \
Your role now is to provide helpful commentary: summarize key insights, suggest follow-up analyses, \
or answer any specific questions the user may have about the data. Do NOT repeat the raw data.";

// ============ Factual Grounding Constants ============

pub const FACTUAL_GROUNDING_BASE: &str = "\
**CRITICAL**: Never make up, infer, or guess data values. All factual information \
(numbers, dates, totals, etc.) MUST come from executing tools or referencing provided context. \
If you need data, use the appropriate tool first. If you cannot get the data, say so explicitly \
rather than inventing results.";

// ============ Python Guidance ============

pub const PYTHON_SANDBOX_RULES: &str = "\
- You must return exactly one runnable Python program in a single ```python ... ``` block. Do not return explanations or multiple blocks.
- Tool calling is only available via Python. Use the provided global functions. Do NOT emit <tool_call> tags or JSON tool calls.
- Use print(...) for user-facing markdown on stdout.
- Use sys.stderr.write(...) for handoff text, which is captured on stderr and triggers loop continuation.
- Allowed imports only: math, json, random, re, datetime, collections, itertools, functools, operator, string, textwrap, copy, types, typing, abc, numbers, decimal, fractions, statistics, hashlib, base64, binascii, html.";

// ============ Builders ============

/// Get the tool call syntax for a specific format and tool.
pub fn tool_call_syntax(format: ToolCallFormatName, tool_name: &str, table_name: Option<&str>) -> String {
    let sql = match table_name {
        Some(name) => format!("SELECT * FROM {} LIMIT 25", name),
        None => "SELECT ...".to_string(),
    };
    match format {
        ToolCallFormatName::Native => format!("<tool_call>{{\"name\": \"{}\", \"arguments\": {{\"sql\": \"{}\"}}}}</tool_call>", tool_name, sql),
        ToolCallFormatName::Hermes => format!("<tool_call>{{\"name\": \"{}\", \"arguments\": {{\"sql\": \"{}\"}}}}</tool_call>", tool_name, sql),
        ToolCallFormatName::Mistral => format!("[TOOL_CALLS] [{{\"name\": \"{}\", \"arguments\": {{\"sql\": \"{}\"}}}}] ", tool_name, sql),
        ToolCallFormatName::Pythonic => format!("{}(sql=\"{}\")", tool_name, sql),
        ToolCallFormatName::PureJson => format!("{{\"name\": \"{}\", \"arguments\": {{\"sql\": \"{}\"}}}}", tool_name, sql),
        ToolCallFormatName::CodeMode => format!("{}(sql=\"{}\")", tool_name, sql),
    }
}

/// Build the SQL action instructions for a given tool call format.
pub fn build_sql_instructions(format: ToolCallFormatName, table_name: Option<&str>) -> String {
    let syntax = tool_call_syntax(format, "sql_select", table_name);
    format!(
        "Use the `sql_select` tool to query these tables.\n\n\
        **ACTION REQUIRED**: Execute queries using the tool-call format:\n\
        ```\n\
        {}\n\
        ```\n\n\
        **CRITICAL REQUIREMENTS**:\n\
        {}",
        syntax,
        SQL_CORE_RULES
    )
}

/// Build the Python execution prompt section.
pub fn build_python_prompt(available_tools: &[String], has_attachments: bool) -> String {
    let tools_section = if available_tools.is_empty() {
        "No MCP tools discovered yet. Call `tool_search` inside Python to find relevant tools if needed.".to_string()
    } else {
        format!("Available MCP tools (call them as global functions): {}", available_tools.join(", "))
    };

    let mut prompt = format!(
        "## Python Execution\n\n\
        {}\n\n\
        **CRITICAL REQUIREMENTS**:\n\
        {}\n\n\
        Keep code concise and runnable; include prints for results the user should see.",
        tools_section,
        PYTHON_SANDBOX_RULES
    );

    if has_attachments {
        prompt.push_str("\n\nAttached files are already summarized in the conversation. Do NOT read files; work with the provided text directly.");
    }

    prompt
}

/// Build the tool documentation for tool_search.
pub fn build_tool_search_documentation(python_tool_mode: bool, use_native_tools: bool, has_deferred_tools: bool) -> String {
    if python_tool_mode {
        // Python mode format
        "Call the global function tool_search(relevant_to=\"...\") inside your Python program to discover MCP tools.\n\
         Then call the returned tools directly.".to_string()
    } else if use_native_tools {
        // Native tools: simple guidance without format examples
        "Call the `tool_search` tool to discover available MCP tools before using them.\n\
         Some MCP tools are deferred; run tool_search early to discover them.".to_string()
    } else {
        // Text-based format: include format example
        let mut s = String::from(
            "Call tool_search to list relevant MCP tools before using them. Example:\n\
             <tool_call>{\"server\": \"builtin\", \"tool\": \"tool_search\", \"arguments\": {\"queries\": [\"your goal\"], \"top_k\": 3}}</tool_call>\n\
             Then call the returned tools directly.",
        );
        if has_deferred_tools {
            s.push_str("\n\nSome MCP tools are deferred; run tool_search early to discover them.");
        }
        s
    }
}

/// Build the tool documentation for schema_search.
pub fn build_schema_search_documentation() -> String {
    "Use `schema_search` to discover database tables that may help answer the user's question.\n\
     Parameters: `query` (search term), `max_tables` (max results, default 5).\n\
     Returns table names, columns, and descriptions relevant to the query.".to_string()
}

/// Build error guidance string, optionally including the original user prompt
pub fn build_error_guidance(tool_name: &str, original_user_prompt: Option<&str>) -> String {
    let base_guidance = if tool_name == "sql_select" {
        format!(
            "**SQL ERROR - RETRY REQUIRED**: The query failed. You MUST retry (up to 3 attempts).\n\n\
            **STEP 1 - Identify the Error**:\n\
            Read the error message above carefully. Common issues:\n\
            - \"Unrecognized name\" = You used a column that doesn't exist. Check the EXACT column names in the schema.\n\
            - \"Function not found\" = Use database-appropriate functions (use CAST(column AS STRING), not TO_CHAR)\n\
            - Syntax error = Check SQL dialect compatibility\n\n\
            **STEP 2 - Review the Schema**:\n\
            Go back to the 'Database Context' section in this prompt. Look at the 'Columns:' list.\n\
            ONLY use columns that are EXPLICITLY listed there. Do NOT invent or guess column names.\n\n\
            **STEP 3 - Retry with Corrected SQL**:\n\
            Make the fix and try again immediately. Do NOT give up or tell the user you can't help.\n\
            You have tools available - USE THEM."
        )
    } else {
        "**TOOL ERROR - RETRY REQUIRED**: The tool call failed. You MUST retry (up to 3 attempts).\n\n\
        **STEP 1**: Read the error message carefully to understand what went wrong.\n\
        **STEP 2**: Review the tool schema for correct parameter names and types.\n\
        **STEP 3**: Retry with corrected parameters immediately.\n\n\
        Do NOT give up or tell the user you cannot help. You have the tools - USE THEM.".to_string()
    };

    match original_user_prompt {
        Some(prompt) if !prompt.is_empty() => {
            format!(
                "\n\n{}\n\n**REMINDER - Original User Request**: \"{}\"\n\n⚠️ TRY AGAIN NOW with a corrected tool call.",
                base_guidance, prompt
            )
        }
        _ => format!(
            "\n\n{}\n\n⚠️ TRY AGAIN NOW with a corrected tool call.",
            base_guidance
        ),
    }
}

/// Build the Capabilities section based on enabled capabilities.
pub fn build_capabilities_section(enabled_capabilities: &HashSet<Capability>, has_attachments: bool) -> Option<String> {
    let has_sql = enabled_capabilities.contains(&Capability::SqlQuery)
        || enabled_capabilities.contains(&Capability::SchemaSearch);
    let has_python = enabled_capabilities.contains(&Capability::PythonExecution);
    let has_mcp = enabled_capabilities.contains(&Capability::McpTools)
        || enabled_capabilities.contains(&Capability::ToolSearch);
    let has_rag = has_attachments;

    // If no tools are enabled, return None
    if !has_sql && !has_python && !has_mcp && !has_rag {
        return None;
    }

    let mut capability_list: Vec<&str> = Vec::new();

    if has_sql {
        capability_list.push("execute SQL queries against configured databases");
    }
    if has_python {
        capability_list.push("perform calculations in a Python sandbox");
    }
    if has_mcp {
        capability_list.push("use external tools via MCP servers");
    }
    if has_rag {
        capability_list.push("answer questions from attached documents");
    }

    if capability_list.is_empty() {
        return None;
    }

    let capabilities_str = match capability_list.len() {
        1 => capability_list[0].to_string(),
        2 => format!("{} and {}", capability_list[0], capability_list[1]),
        _ => {
            let last = capability_list.pop().unwrap();
            format!("{}, and {}", capability_list.join(", "), last)
        }
    };

    Some(format!(
        "## Capabilities\n\n\
        You are equipped with specialized tools to {}. \
        You MUST use these tools whenever the user's request requires factual data or tool execution. \
        Do NOT claim you cannot perform these tasks; use the tools listed below.",
        capabilities_str
    ))
}

/// Build the Factual Grounding section based on enabled tools.
pub fn build_factual_grounding(enabled_capabilities: &HashSet<Capability>, _has_attachments: bool) -> String {
    let has_sql = enabled_capabilities.contains(&Capability::SqlQuery);
    let has_mcp = enabled_capabilities.contains(&Capability::McpTools)
        || enabled_capabilities.contains(&Capability::ToolSearch);

    // Build tool-specific examples
    let mut tool_examples: Vec<&str> = Vec::new();
    if has_sql {
        tool_examples.push("`sql_select`");
    }
    if has_mcp {
        tool_examples.push("MCP tools");
    }

    let examples_str = if tool_examples.is_empty() {
        "the appropriate tools".to_string()
    } else {
        tool_examples.join(" or ")
    };

    format!(
        "## Factual Grounding\n\n\
        {} If you need data, use the appropriate tool like {} first.",
        FACTUAL_GROUNDING_BASE,
        examples_str
    )
}

/// Build tool format instructions based on tool_call_format.
pub fn build_format_instructions(
    primary_format: ToolCallFormatName,
    model_tool_format: Option<ToolFormat>,
) -> Option<String> {
    // Even if primary is Native, we provide instructions if the model family has a preferred tag format.
    // Local models (like Phi, Qwen, Granite) often need the explicit tag to trigger tool calling.
    let effective_format = if primary_format == ToolCallFormatName::Native {
        match model_tool_format {
            Some(ToolFormat::Hermes) => ToolCallFormatName::Hermes,
            Some(ToolFormat::Granite) => ToolCallFormatName::Mistral, // Will handle Granite specific below
            _ => ToolCallFormatName::Native,
        }
    } else {
        primary_format
    };

    match effective_format {
        ToolCallFormatName::Native => None, // Truly native models (like GPT-4) don't need instructions
        ToolCallFormatName::Hermes => Some(
            "## Tool Calling Format\n\n\
            When you need to use a tool, output ONLY:\n\
            <tool_call>{\"name\": \"tool_name\", \"arguments\": {...}}</tool_call>".to_string()
        ),
        ToolCallFormatName::Mistral => {
            match model_tool_format {
                Some(ToolFormat::Granite) => Some(
                    "## Function Calling Format\n\n\
                    When you need to call a function, output:\n\
                    <function_call>{\"name\": \"function_name\", \"arguments\": {...}}</function_call>".to_string()
                ),
                _ => Some(
                    "## Tool Calling Format\n\n\
                    When you need to use a tool, output:\n\
                    [TOOL_CALLS] [{\"name\": \"tool_name\", \"arguments\": {...}}]".to_string()
                )
            }
        },
        ToolCallFormatName::Pythonic => Some(
            "## Tool Calling Format\n\n\
            When you need to use a tool, output:\n\
            tool_name(arg1=\"value\", arg2=123)".to_string()
        ),
        ToolCallFormatName::PureJson => Some(
            "## Tool Calling Format\n\n\
            When you need to use a tool, output a JSON object:\n\
            {\"name\": \"tool_name\", \"arguments\": {...}}".to_string()
        ),
        ToolCallFormatName::CodeMode => None, // Code mode has its own section
    }
}

/// Build auto-discovery tool search section.
pub fn build_auto_tool_search_section(tools: &[ToolSearchResult]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }
    let mut body = String::from("Auto-discovered MCP tools for this prompt:");
    for tool in tools {
        let desc = tool.description.as_deref().unwrap_or("").trim();
        let mut line = format!(
            "\n- {}::{} (score {:.2})",
            tool.server_id, tool.name, tool.score
        );
        if !desc.is_empty() {
            line.push_str(&format!(" — {}", desc));
        }
        body.push_str(&line);
    }
    Some(format!("### Auto tool search\n{}", body))
}

/// Build auto-discovery schema search section.
pub fn build_auto_schema_search_section(
    tables: &[crate::tools::schema_search::TableMatchOutput],
    summary: &str,
    has_attachments: bool,
    sql_enabled: bool,
    format: ToolCallFormatName,
) -> Option<String> {
    if tables.is_empty() {
        if summary.contains("WARNING") {
            return Some(format!("### Auto schema search\n{}", summary));
        }
        return None;
    }

    // Apply rule: if we have RAG results (attachments), only include SQL context
    // if the highest relevance score is > 40%.
    let max_score = tables.iter().map(|t| t.relevance).fold(0.0f32, f32::max);
    if has_attachments && max_score <= 0.40 {
        println!(
            "[system_prompt] Auto schema_search suppressed: RAG available and max SQL score ({:.2}) <= 0.40",
            max_score
        );
        return None;
    }

    // Only show tables if they are above threshold (sql_enabled == true)
    if !sql_enabled {
        if summary.contains("WARNING") {
            return Some(format!("### Auto schema search\n{}", summary));
        }
        return None;
    }

    let mut body = String::from("Auto-discovered database tables for this prompt (can be queried using `sql_select`):");
    for table in tables {
        let mut line = format!(
            "\n- {} [{} Syntax | {}] (queryable via `sql_select`, score {:.2})",
            table.table_name, table.sql_dialect, table.source_id, table.relevance
        );
        if let Some(desc) = table.description.as_deref() {
            if !desc.trim().is_empty() {
                line.push_str(&format!(" — {}", desc.trim()));
            }
        }
        if !table.relevant_columns.is_empty() {
            let cols: Vec<String> = table
                .relevant_columns
                .iter()
                .take(40) // Show up to 40 columns
                .map(|c| format!("{} ({})", c.name, c.data_type))
                .collect();
            let cols_str = if cols.len() < table.relevant_columns.len() {
                format!("{}, ... ({} more)", cols.join(", "), table.relevant_columns.len() - cols.len())
            } else {
                cols.join(", ")
            };
            line.push_str(&format!("\n  cols: {}", cols_str));
        }
        body.push_str(&line);
    }

    let first_table = tables.first().map(|t| t.table_name.as_str());
    body.push_str(&format!("\n\n{}", build_sql_instructions(format, first_table)));

    Some(format!("### Auto schema search\n{}", body))
}

/// Build MCP tool documentation for multiple tools.
pub fn build_mcp_tools_documentation(
    active_tools: &[(String, Vec<McpToolInfo>)],
    servers: &[crate::agentic_state::McpServerInfo],
    custom_tool_prompts: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if active_tools.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    parts.push("## Active MCP Tools (Ready to Use)\n\nThese tools can be called immediately:".to_string());
    
    for (server_id, tools) in active_tools {
        if tools.is_empty() {
            continue;
        }
        
        parts.push(format!("\n### Server: `{}`\n", server_id));
        
        // Find server info for env vars
        if let Some(server_info) = servers.iter().find(|s| s.id == *server_id) {
            if !server_info.visible_env.is_empty() {
                let mut pairs: Vec<String> = server_info.visible_env
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect();
                pairs.sort();
                parts.push(format!("Environment variables: {}\n", pairs.join(", ")));
            }
        }
        
        for tool in tools {
            let mut body = format!("**{}**", tool.name);
            if let Some(desc) = &tool.description {
                body.push_str(&format!(": {}", desc));
            }
            parts.push(body);

            // Add custom tool prompt if available
            let prompt_key = format!("{}::{}", server_id, tool.name);
            if let Some(custom_prompt) = custom_tool_prompts.get(&prompt_key) {
                let trimmed = custom_prompt.trim();
                if !trimmed.is_empty() {
                    parts.push(format!("  *Instruction*: {}", trimmed));
                }
            }
            
            // Add parameter info if available
            if let Some(schema) = &tool.parameters_schema {
                if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                    let required: Vec<&str> = schema
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();
                    
                    parts.push("  Arguments:".to_string());
                    for (name, prop) in props {
                        let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                        let is_required = required.contains(&name.as_str());
                        let req_marker = if is_required { " [REQUIRED]" } else { "" };
                        parts.push(format!("  - `{}` ({}){}", name, prop_type, req_marker));
                    }
                }
            }
        }
    }

    Some(parts.join("\n"))
}

/// Build deferred MCP tool summary.
pub fn build_deferred_mcp_tool_summary(count: usize, server_count: usize) -> String {
    format!(
        "\n## Deferred MCP Tools\n\n\
        There are {} tools available across {} server(s). \
        These tools are currently deferred to save context space. \
        Use `tool_search(relevant_to=\"...\")` to discover and enable them when needed.",
        count, server_count
    )
}

/// Build summary of document context.
pub fn build_document_context_summary(relevancy: f32) -> String {
    format!(
        "## Document Context (relevancy: {:.2})\n\n\
        Document content has been provided in the conversation above.",
        relevancy
    )
}

/// Build detailed document context section.
pub fn build_retrieved_document_context(relevancy: f32, chunks_text: &str) -> String {
    format!(
        "## Retrieved Document Context\n\n\
        The following excerpts are relevant to the user's question (max relevancy: {:.2}):\n\n\
        {}\n\n\
        Answer the user's question using this context. Cite sources when helpful.\n\
        If the context doesn't fully answer the question, say so clearly.",
        relevancy,
        chunks_text
    )
}

/// Build state-specific SQL context.
pub fn build_retrieved_sql_context(relevancy: f32, table_list: &str, sql_instructions: &str) -> String {
    format!(
        "## Retrieved Database Context\n\n\
        The following database tables are relevant to the user's question and can be queried using the `sql_select` tool (max relevancy: {:.2}):\n\n\
        {}\n\n\
        ## SQL Execution Guidance\n\n\
        {}",
        relevancy,
        table_list,
        sql_instructions
    )
}

/// Format a list of tables for the prompt.
pub fn format_table_list(tables: &[TableInfo]) -> String {
    if tables.is_empty() {
        return "No tables discovered.".to_string();
    }

    tables
        .iter()
        .map(|table| {
            let cols: Vec<String> = table
                .columns
                .iter()
                .take(40) // Show up to 40 columns to give model enough context
                .map(|c| format!("{} ({})", c.name, c.data_type))
                .collect();
            let cols_str = if cols.len() < table.columns.len() {
                format!("{}, ... ({} more)", cols.join(", "), table.columns.len() - cols.len())
            } else {
                cols.join(", ")
            };
            format!(
                "- **{}** [{} Syntax] (queryable via `sql_select`, relevancy: {:.2})\n  Columns: {}",
                table.fully_qualified_name,
                table.sql_dialect,
                table.relevancy,
                cols_str
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format RAG chunks for the prompt.
pub fn format_rag_chunks(chunks: &[RagChunk]) -> String {
    if chunks.is_empty() {
        return "No document chunks available.".to_string();
    }

    chunks
        .iter()
        .map(|chunk| {
            let preview: String = chunk.content.chars().take(500).collect();
            let truncated = if chunk.content.len() > 500 { "..." } else { "" };
            format!(
                "### {} (relevancy: {:.2})\n\n{}{}",
                chunk.source_file, chunk.relevancy, preview, truncated
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Format tool schemas for the prompt.
pub fn format_tool_schemas(schemas: &[ToolSchema]) -> String {
    if schemas.is_empty() {
        return "".to_string();
    }

    schemas
        .iter()
        .map(|schema| {
            let desc = schema.description.as_deref().unwrap_or("No description");
            format!("- **{}**: {}", schema.name, desc)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_mcp_tool_documentation(
    server_name: &str,
    tool_name: &str,
    description: &str,
    args_schema: Option<&serde_json::Value>,
    is_deferred: bool,
) -> String {
    let mut body = format!("### {} ({})\n", tool_name, server_name);
    if is_deferred {
        body.push_str("**Status**: Deferred - you MUST discover this tool using `tool_search` before calling it.\n\n");
    }
    body.push_str(&format!("Description: {}\n", description));
    if let Some(schema) = args_schema {
        body.push_str(&format!("Arguments: {}\n", schema));
    }
    body
}
