//! MCP tools that can be called straight from the model drop-down, with no local model in between.
//!
//! A tool qualifies when it takes exactly one text field: whatever the user types is that field's
//! value. The list is built from the tools the connected servers reported, so it exists the moment a
//! server connects, whether or not the local model works.

use crate::actors::mcp_host_actor::McpTool;
use serde::Serialize;
use serde_json::Value;

/// What one connected server reported, as the input of `qualifying_direct_tools`.
#[derive(Debug, Clone)]
pub struct DirectToolSource {
    /// The id of the server in settings; the call is routed by it.
    pub server_id: String,
    /// The name the user gave the server in settings.
    pub configured_name: String,
    /// `serverInfo.name` from the server's `initialize` reply, when it sent one.
    pub reported_name: Option<String>,
    pub tools: Vec<McpTool>,
}

/// One entry of the drop-down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectTool {
    pub server_id: String,
    /// The server's short name: what it reports about itself, else its configured name, else its id.
    pub server_name: String,
    pub tool_name: String,
    /// The one text field the message is sent as.
    pub field_name: String,
    /// The text shown in the drop-down: `server - tool`.
    pub label: String,
    pub description: Option<String>,
}

/// The name of the tool's only input when it is a single text field; `None` for every other shape
/// (no inputs, two or more inputs even if the second is optional, one input that is not text).
pub fn single_text_field(input_schema: Option<&Value>) -> Option<String> {
    let schema = input_schema?.as_object()?;
    if schema.get("type")?.as_str()? != "object" {
        return None;
    }
    let properties = schema.get("properties")?.as_object()?;
    if properties.len() != 1 {
        return None;
    }
    let (name, property) = properties.iter().next()?;
    (property.get("type")?.as_str()? == "string").then(|| name.clone())
}

/// The short name shown for a server.
pub fn short_server_name(reported: Option<&str>, configured: &str, server_id: &str) -> String {
    [reported.unwrap_or(""), configured, server_id]
        .iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .unwrap_or(server_id)
        .to_string()
}

/// The qualifying tools of the connected servers, in the order the drop-down lists them.
pub fn qualifying_direct_tools(sources: &[DirectToolSource]) -> Vec<DirectTool> {
    let mut found: Vec<DirectTool> = Vec::new();
    for source in sources {
        let server_name = short_server_name(source.reported_name.as_deref(), &source.configured_name, &source.server_id);
        for tool in &source.tools {
            let Some(field_name) = single_text_field(tool.input_schema.as_ref()) else { continue };
            found.push(DirectTool {
                server_id: source.server_id.clone(),
                label: format!("{} - {}", server_name, tool.name),
                server_name: server_name.clone(),
                tool_name: tool.name.clone(),
                field_name,
                description: tool.description.clone(),
            });
        }
    }
    found.sort_by(|a, b| {
        a.label.to_lowercase().cmp(&b.label.to_lowercase()).then_with(|| a.server_id.cmp(&b.server_id))
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, schema: Option<Value>) -> McpTool {
        McpTool { name: name.to_string(), description: Some(format!("{name} tool")), input_schema: schema, input_examples: None, allowed_callers: None }
    }

    /// The tool the product owner gave as the example.
    fn ask_tool() -> McpTool {
        serde_json::from_value(json!({
            "name": "ask",
            "description": "Ask PlugableIQ a business question. Returns its answer with the footer: sources, data-through dates, receipts, facts used and confidence.",
            "inputSchema": {"type": "object", "properties": {"question": {"type": "string"}}, "required": ["question"]}
        }))
        .unwrap()
    }

    fn source(id: &str, configured: &str, reported: Option<&str>, tools: Vec<McpTool>) -> DirectToolSource {
        DirectToolSource { server_id: id.to_string(), configured_name: configured.to_string(), reported_name: reported.map(String::from), tools }
    }

    #[test]
    fn the_ask_tool_of_plugableiq_is_offered_as_plugableiq_dash_ask_with_its_one_field() {
        let tools = qualifying_direct_tools(&[source("srv-1", "My IQ box", Some("plugableiq"), vec![ask_tool()])]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].label, "plugableiq - ask");
        assert_eq!((tools[0].server_name.as_str(), tools[0].tool_name.as_str(), tools[0].field_name.as_str()), ("plugableiq", "ask", "question"));
        assert_eq!(tools[0].server_id, "srv-1", "the call is routed by the settings id, not the reported name");
    }

    #[test]
    fn tools_that_do_not_take_exactly_one_text_field_are_left_out() {
        let shapes = [
            ("two_fields", json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"]})),
            ("second_optional_field", json!({"type":"object","properties":{"question":{"type":"string"},"limit":{"type":"string"}},"required":["question"]})),
            ("number_field", json!({"type":"object","properties":{"n":{"type":"number"}}})),
            ("boolean_field", json!({"type":"object","properties":{"flag":{"type":"boolean"}}})),
            ("array_field", json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}}}})),
            ("nullable_text", json!({"type":"object","properties":{"q":{"type":["string","null"]}}})),
            ("no_properties", json!({"type":"object","properties":{}})),
            ("properties_missing", json!({"type":"object"})),
            ("not_an_object_schema", json!({"type":"string"})),
            ("property_without_type", json!({"type":"object","properties":{"q":{}}})),
        ];
        let mut tools: Vec<McpTool> = shapes.iter().map(|(name, schema)| tool(name, Some(schema.clone()))).collect();
        tools.push(tool("no_schema_at_all", None));
        tools.push(ask_tool());
        let offered = qualifying_direct_tools(&[source("s", "s", Some("plugableiq"), tools)]);
        let names: Vec<&str> = offered.iter().map(|t| t.tool_name.as_str()).collect();
        assert_eq!(names, vec!["ask"], "only the single-text-field tool qualifies");
    }

    #[test]
    fn the_short_name_is_what_the_server_reports_then_the_configured_name_then_the_id() {
        assert_eq!(short_server_name(Some("plugableiq"), "My IQ box", "srv-1"), "plugableiq");
        assert_eq!(short_server_name(None, "My IQ box", "srv-1"), "My IQ box");
        assert_eq!(short_server_name(Some("  "), "My IQ box", "srv-1"), "My IQ box");
        assert_eq!(short_server_name(Some(""), "", "srv-1"), "srv-1");
    }

    #[test]
    fn entries_are_sorted_by_text_and_a_server_with_no_qualifying_tool_adds_nothing() {
        let sources = [
            source("b", "zeta", None, vec![tool("run", Some(json!({"type":"object","properties":{"cmd":{"type":"string"}}})))]),
            source("a", "alpha", Some("Alpha"), vec![ask_tool(), tool("list", Some(json!({"type":"object","properties":{}})))]),
            source("c", "empty", None, vec![]),
        ];
        let labels: Vec<String> = qualifying_direct_tools(&sources).into_iter().map(|t| t.label).collect();
        assert_eq!(labels, vec!["Alpha - ask", "zeta - run"]);
    }
}
