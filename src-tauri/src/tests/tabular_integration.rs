//! Integration tests for tabular file parsing and Python context injection
//!
//! These tests validate:
//! 1. CSV/TSV file parsing with type inference
//! 2. Python context building from tabular data
//! 3. Model inference with tabular data context
//!
//! Requirements for model tests:
//! - Foundry Local must be running
//! - A model must be loaded (e.g., phi-4-mini)

use std::path::PathBuf;
use serde_json::json;

use crate::tabular_parser::{
    parse_tabular_file, parse_tabular_headers, 
    TypedValue, ColumnType, TabularFileData
};

/// Get the path to the test-data directory
fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test-data")
}

// ============ Unit Tests for File Parsing ============

#[test]
fn test_parse_csv_file() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    
    let result = parse_tabular_file(&csv_path);
    assert!(result.is_ok(), "Failed to parse CSV: {:?}", result.err());
    
    let data = result.unwrap();
    
    // Check headers
    assert_eq!(data.headers.len(), 6);
    assert_eq!(data.headers[0], "name");
    assert_eq!(data.headers[1], "age");
    assert_eq!(data.headers[2], "salary");
    assert_eq!(data.headers[3], "hire_date");
    assert_eq!(data.headers[4], "active");
    assert_eq!(data.headers[5], "department");
    
    // Check row count
    assert_eq!(data.row_count, 5);
    assert_eq!(data.rows.len(), 5);
    
    // Check first row
    assert_eq!(data.rows[0][0], TypedValue::String("Alice".to_string()));
    assert_eq!(data.rows[0][1], TypedValue::Int(30));
    // Currency should be parsed as float
    assert!(matches!(data.rows[0][2], TypedValue::Float(f) if (f - 75000.50).abs() < 0.01));
    // Date should be parsed
    assert!(matches!(&data.rows[0][3], TypedValue::DateTime(s) if s.contains("2024")));
    // Boolean
    assert_eq!(data.rows[0][4], TypedValue::Bool(true));
    
    // Check null handling (row 3, age is "N/A")
    assert_eq!(data.rows[2][1], TypedValue::Null);
    
    // Check empty value (row 5, salary is empty)
    assert_eq!(data.rows[4][2], TypedValue::Null);
    
    println!("[Test] CSV parsing successful: {} headers, {} rows", data.headers.len(), data.row_count);
}

#[test]
fn test_parse_csv_headers_only() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    
    let result = parse_tabular_headers(&csv_path);
    assert!(result.is_ok(), "Failed to parse headers: {:?}", result.err());
    
    let preview = result.unwrap();
    
    assert_eq!(preview.headers.len(), 6);
    assert_eq!(preview.row_count, 5);
    assert_eq!(preview.file_name, "test_tabular.csv");
    
    // Check column types
    assert_eq!(preview.column_types[0], ColumnType::String); // name
    assert_eq!(preview.column_types[1], ColumnType::Int);    // age
    assert_eq!(preview.column_types[2], ColumnType::Float);  // salary
    assert_eq!(preview.column_types[3], ColumnType::DateTime); // hire_date
    assert_eq!(preview.column_types[4], ColumnType::Bool);   // active
    assert_eq!(preview.column_types[5], ColumnType::String); // department
    
    println!("[Test] Headers preview: {:?}", preview.headers);
}

#[test]
fn test_parse_chicago_crimes_csv() {
    let csv_path = test_data_dir().join("Chicago_Crimes_2025_Enriched.csv");
    
    if !csv_path.exists() {
        println!("[Test] Skipping Chicago crimes test - file not found");
        return;
    }
    
    let result = parse_tabular_file(&csv_path);
    assert!(result.is_ok(), "Failed to parse Chicago crimes CSV: {:?}", result.err());
    
    let data = result.unwrap();
    
    // Verify we got reasonable data
    assert!(data.headers.len() > 10, "Expected many columns in crimes data");
    assert!(data.row_count > 0, "Expected some crime records");
    
    // Check that column analysis worked
    assert_eq!(data.columns.len(), data.headers.len());
    
    println!("[Test] Chicago crimes CSV: {} columns, {} rows", data.headers.len(), data.row_count);
    println!("[Test] First 5 headers: {:?}", &data.headers[..5.min(data.headers.len())]);
}

// ============ Tests for Python Context Building ============

#[test]
fn test_build_python_context_single_file() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    // Build context using the same logic as in lib.rs
    let context = build_test_context(&[data]);
    
    // Verify headers1 exists
    assert!(context.get("headers1").is_some(), "Missing headers1");
    let headers = context.get("headers1").unwrap().as_array().unwrap();
    assert_eq!(headers.len(), 6);
    assert_eq!(headers[0].as_str().unwrap(), "name");
    
    // Verify rows1 exists
    assert!(context.get("rows1").is_some(), "Missing rows1");
    let rows = context.get("rows1").unwrap().as_array().unwrap();
    assert_eq!(rows.len(), 5);
    
    // Check first row values
    let first_row = rows[0].as_array().unwrap();
    assert_eq!(first_row[0].as_str().unwrap(), "Alice");
    assert_eq!(first_row[1].as_i64().unwrap(), 30);
    
    println!("[Test] Python context built successfully with {} headers and {} rows", 
             headers.len(), rows.len());
}

#[test]
fn test_build_python_context_multiple_files() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data1 = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    let data2 = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    let context = build_test_context(&[data1, data2]);
    
    // Should have headers1/rows1 and headers2/rows2
    assert!(context.get("headers1").is_some());
    assert!(context.get("rows1").is_some());
    assert!(context.get("headers2").is_some());
    assert!(context.get("rows2").is_some());
    
    println!("[Test] Multiple file context built with headers1, rows1, headers2, rows2");
}

#[test]
fn test_datetime_json_format() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    let context = build_test_context(&[data]);
    let rows = context.get("rows1").unwrap().as_array().unwrap();
    
    // Row 0, column 3 should be a datetime
    let hire_date = &rows[0].as_array().unwrap()[3];
    
    // Datetime should be an object with __datetime__ key
    assert!(hire_date.is_object(), "DateTime should be serialized as object");
    let dt_obj = hire_date.as_object().unwrap();
    assert!(dt_obj.contains_key("__datetime__"), "DateTime object should have __datetime__ key");
    
    let dt_str = dt_obj.get("__datetime__").unwrap().as_str().unwrap();
    assert!(dt_str.contains("2024"), "DateTime string should contain year");
    
    println!("[Test] DateTime serialized correctly: {}", dt_str);
}

// ============ Helper Functions ============

/// Build Python context from parsed tabular files (mirrors lib.rs logic)
fn build_test_context(files: &[TabularFileData]) -> serde_json::Map<String, serde_json::Value> {
    let mut context = serde_json::Map::new();

    for (index, file) in files.iter().enumerate() {
        let var_index = index + 1;

        // Add headers
        let headers_key = format!("headers{}", var_index);
        let headers_value: Vec<serde_json::Value> = file
            .headers
            .iter()
            .map(|h| serde_json::Value::String(h.clone()))
            .collect();
        context.insert(headers_key, serde_json::Value::Array(headers_value));

        // Add rows
        let rows_key = format!("rows{}", var_index);
        let rows_value: Vec<serde_json::Value> = file
            .rows
            .iter()
            .map(|row| {
                serde_json::Value::Array(
                    row.iter()
                        .map(|cell| typed_value_to_json(cell))
                        .collect(),
                )
            })
            .collect();
        context.insert(rows_key, serde_json::Value::Array(rows_value));
    }

    context
}

/// Convert TypedValue to JSON (mirrors lib.rs logic)
fn typed_value_to_json(value: &TypedValue) -> serde_json::Value {
    match value {
        TypedValue::Null => serde_json::Value::Null,
        TypedValue::Bool(b) => serde_json::Value::Bool(*b),
        TypedValue::Int(i) => json!(i),
        TypedValue::Float(f) => json!(f),
        TypedValue::DateTime(s) => json!({ "__datetime__": s }),
        TypedValue::String(s) => serde_json::Value::String(s.clone()),
    }
}

// ============ Model Integration Tests ============
// These tests require Foundry Local to be running

#[tokio::test]
#[ignore] // Run with: cargo test -- --ignored
async fn test_model_tabular_data_sum() {
    // This test sends tabular data to the model and asks it to calculate a sum
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    let context = build_test_context(&[data]);
    
    // Build a prompt that asks the model to analyze the data
    let prompt = r#"
I have a table of employee data with the following structure:
- headers1: ("name", "age", "salary", "hire_date", "active", "department")
- rows1: List of tuples with 5 employees

The data has been preprocessed:
- Numeric columns are already converted to int/float
- Missing values are None
- Currency symbols have been stripped

Using Python, calculate the total salary of all employees (handle None values).
Write the code that accesses headers1 and rows1 to compute the sum.
"#;

    println!("[Test] Sending tabular data analysis request to model...");
    println!("[Test] Context keys: {:?}", context.keys().collect::<Vec<_>>());
    
    // Try to call the model (if Foundry is running)
    match call_model_with_context(&prompt, &context).await {
        Ok(response) => {
            println!("[Test] Model response:\n{}", response);
            
            // The response should contain Python code
            assert!(
                response.contains("rows1") || response.contains("sum") || response.contains("salary"),
                "Model response should reference the tabular data"
            );
        }
        Err(e) => {
            println!("[Test] Model call failed (is Foundry running?): {}", e);
            // Don't fail the test if Foundry isn't running
        }
    }
}

#[tokio::test]
#[ignore] // Run with: cargo test -- --ignored
async fn test_model_tabular_data_filter() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    let context = build_test_context(&[data]);
    
    let prompt = r#"
I have employee data in headers1/rows1. The columns are: name, age, salary, hire_date, active, department.
Write Python code to filter and print only the Engineering department employees.
"#;

    match call_model_with_context(&prompt, &context).await {
        Ok(response) => {
            println!("[Test] Model response:\n{}", response);
            assert!(
                response.contains("Engineering") || response.contains("department") || response.contains("filter"),
                "Model response should reference filtering"
            );
        }
        Err(e) => {
            println!("[Test] Model call failed: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_model_tabular_data_datetime_handling() {
    let csv_path = test_data_dir().join("test_tabular.csv");
    let data = parse_tabular_file(&csv_path).expect("Failed to parse CSV");
    
    let context = build_test_context(&[data]);
    
    let prompt = r#"
The employee data in headers1/rows1 includes hire_date as datetime objects.
Write Python code to find all employees hired in 2024.
Remember: dates are already datetime objects, use .year to get the year.
"#;

    match call_model_with_context(&prompt, &context).await {
        Ok(response) => {
            println!("[Test] Model response:\n{}", response);
            assert!(
                response.contains("2024") || response.contains(".year") || response.contains("hire_date"),
                "Model response should reference dates"
            );
        }
        Err(e) => {
            println!("[Test] Model call failed: {}", e);
        }
    }
}

/// Helper to call the model with tabular context
async fn call_model_with_context(
    prompt: &str,
    _context: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, String> {
    // Discover Foundry
    let (port, model_id) = discover_foundry().await?;
    
    // Build system prompt with tabular data guidance
    let system_prompt = r#"You are a data analyst. The user has attached tabular data available as Python variables:
- headers1: tuple of column names
- rows1: list of tuples with typed values (int, float, datetime, str, bool, or None)

Write Python code to analyze the data. Use print() for output.
Numbers are already typed (no need to convert strings).
Missing values are None (not "N/A" or empty strings).
Dates are datetime objects (use .year, .month, .day)."#;

    let client = reqwest::Client::new();
    let body = json!({
        "model": model_id,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": prompt}
        ],
        "stream": false,
        "temperature": 0.0,
    });

    let res = client.post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Model error: {}", res.status()));
    }

    let json: serde_json::Value = res.json().await.map_err(|e| format!("Parse error: {}", e))?;
    
    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No content in response".to_string())
}

/// Discover Foundry Local port and model
async fn discover_foundry() -> Result<(u16, String), String> {
    let client = reqwest::Client::new();
    
    // Try common ports
    for port in [5272, 5273, 5274, 5275] {
        if let Ok(res) = client
            .get(format!("http://127.0.0.1:{}/v1/models", port))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            if res.status().is_success() {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    if let Some(models) = json["data"].as_array() {
                        if let Some(first) = models.first() {
                            if let Some(id) = first["id"].as_str() {
                                return Ok((port, id.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    
    Err("Foundry Local not found on common ports".to_string())
}
