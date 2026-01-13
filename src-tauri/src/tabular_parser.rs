//! Tabular file parsing with automatic type inference.
//!
//! Parses CSV, TSV, XLS, and XLSX files, automatically inferring column types
//! and converting values to typed representations for Python injection.
//!
//! ## Preprocessing Philosophy
//! The orchestration layer does the heavy lifting so small models succeed:
//! - Numeric columns are pre-converted to int/float
//! - Missing values normalized to None
//! - Currency symbols and thousands separators stripped
//! - Percentages converted to decimals
//! - Dates parsed to datetime objects

use calamine::{open_workbook_auto, Data, Reader};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A typed value from a tabular cell.
/// Serializes to JSON for Python context injection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum TypedValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// DateTime stored as ISO format string for JSON serialization.
    /// Python sandbox will parse this back to datetime object.
    DateTime(String),
    String(String),
}

impl TypedValue {
    /// Get the column type for this value (ignoring Null).
    pub fn column_type(&self) -> Option<ColumnType> {
        match self {
            TypedValue::Null => None,
            TypedValue::Bool(_) => Some(ColumnType::Bool),
            TypedValue::Int(_) => Some(ColumnType::Int),
            TypedValue::Float(_) => Some(ColumnType::Float),
            TypedValue::DateTime(_) => Some(ColumnType::DateTime),
            TypedValue::String(_) => Some(ColumnType::String),
        }
    }
}

/// Inferred column type for system prompt documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Bool,
    Int,
    Float,
    DateTime,
    String,
    /// Mixed types in column (fallback to string)
    Mixed,
}

impl std::fmt::Display for ColumnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColumnType::Bool => write!(f, "bool"),
            ColumnType::Int => write!(f, "int"),
            ColumnType::Float => write!(f, "float"),
            ColumnType::DateTime => write!(f, "datetime"),
            ColumnType::String => write!(f, "str"),
            ColumnType::Mixed => write!(f, "mixed"),
        }
    }
}

/// Column metadata for system prompt generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub column_type: ColumnType,
    pub null_count: usize,
    pub sample_values: Vec<String>,
}

/// Parsed tabular file data ready for Python injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabularFileData {
    /// Original file path
    pub file_path: String,
    /// File name only (for display)
    pub file_name: String,
    /// Column headers
    pub headers: Vec<String>,
    /// Rows of typed values
    pub rows: Vec<Vec<TypedValue>>,
    /// Column type information
    pub columns: Vec<ColumnInfo>,
    /// Total row count
    pub row_count: usize,
}

/// Header preview for UI display (lightweight, no full data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabularHeaderPreview {
    pub file_path: String,
    pub file_name: String,
    pub headers: Vec<String>,
    pub row_count: usize,
    pub column_types: Vec<ColumnType>,
}

/// Parse a tabular file (CSV, TSV, XLS, XLSX) with full type inference.
pub fn parse_tabular_file(path: &Path) -> Result<TabularFileData, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (headers, raw_rows) = match extension.as_str() {
        "csv" => parse_csv(path, b',')?,
        "tsv" => parse_csv(path, b'\t')?,
        "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" => parse_excel(path)?,
        _ => {
            // Try to detect delimiter from content
            parse_csv_auto_detect(path)?
        }
    };

    // Convert raw string rows to typed values
    let typed_rows = infer_and_convert_rows(&raw_rows);

    // Analyze column types
    let columns = analyze_columns(&headers, &typed_rows);

    Ok(TabularFileData {
        file_path: path.to_string_lossy().to_string(),
        file_name,
        headers,
        row_count: typed_rows.len(),
        rows: typed_rows,
        columns,
    })
}

/// Parse just the headers and row count for UI preview.
pub fn parse_tabular_headers(path: &Path) -> Result<TabularHeaderPreview, String> {
    // Parse full file to get accurate type inference
    let data = parse_tabular_file(path)?;

    Ok(TabularHeaderPreview {
        file_path: data.file_path,
        file_name: data.file_name,
        headers: data.headers,
        row_count: data.row_count,
        column_types: data.columns.iter().map(|c| c.column_type).collect(),
    })
}

/// Parse a CSV/TSV file with the given delimiter.
fn parse_csv(path: &Path, delimiter: u8) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .flexible(true) // Allow varying number of fields
        .from_path(path)
        .map_err(|e| format!("Failed to open CSV file: {}", e))?;

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("Failed to read CSV headers: {}", e))?
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut rows = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|e| format!("Failed to read CSV row: {}", e))?;
        let row: Vec<String> = record.iter().map(|s| s.to_string()).collect();
        rows.push(row);
    }

    Ok((headers, rows))
}

/// Auto-detect delimiter and parse CSV.
fn parse_csv_auto_detect(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    // Read first few lines to detect delimiter
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    let first_line = content.lines().next().unwrap_or("");

    // Count potential delimiters
    let comma_count = first_line.matches(',').count();
    let tab_count = first_line.matches('\t').count();
    let semicolon_count = first_line.matches(';').count();

    let delimiter = if tab_count > comma_count && tab_count > semicolon_count {
        b'\t'
    } else if semicolon_count > comma_count {
        b';'
    } else {
        b','
    };

    parse_csv(path, delimiter)
}

/// Parse an Excel file (XLS, XLSX, etc.).
fn parse_excel(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut workbook = open_workbook_auto(path)
        .map_err(|e| format!("Failed to open Excel file: {}", e))?;

    // Get the first worksheet
    let sheet_names = workbook.sheet_names().to_vec();
    let sheet_name = sheet_names
        .first()
        .ok_or_else(|| "Excel file has no worksheets".to_string())?;

    let range = workbook
        .worksheet_range(sheet_name)
        .map_err(|e| format!("Failed to read worksheet: {}", e))?;

    let mut rows_iter = range.rows();

    // First row is headers
    let headers: Vec<String> = rows_iter
        .next()
        .map(|row| row.iter().map(|cell| excel_cell_to_string(cell)).collect())
        .unwrap_or_default();

    // Remaining rows are data
    let rows: Vec<Vec<String>> = rows_iter
        .map(|row| row.iter().map(|cell| excel_cell_to_string(cell)).collect())
        .collect();

    Ok((headers, rows))
}

/// Convert an Excel cell to a string value.
fn excel_cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            // Format floats nicely (remove trailing zeros)
            if f.fract() == 0.0 {
                format!("{:.0}", f)
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => {
            // ExcelDateTime - convert to ISO string
            // Format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS
            format!("{}", dt)
        }
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{:?}", e),
    }
}

// ============ Type Inference ============

/// Convert rows of raw strings to typed values.
fn infer_and_convert_rows(raw_rows: &[Vec<String>]) -> Vec<Vec<TypedValue>> {
    raw_rows
        .iter()
        .map(|row| row.iter().map(|cell| infer_and_convert(cell)).collect())
        .collect()
}

/// Infer type and convert a raw string value to a TypedValue.
fn infer_and_convert(raw: &str) -> TypedValue {
    let trimmed = raw.trim();

    // Check for null/missing
    if is_missing_value(trimmed) {
        return TypedValue::Null;
    }

    // Check for boolean
    if let Some(b) = try_parse_bool(trimmed) {
        return TypedValue::Bool(b);
    }

    // Try numeric (with currency/percentage handling)
    if let Some(num) = try_parse_numeric(trimmed) {
        return num;
    }

    // Try datetime
    if let Some(dt) = try_parse_datetime(trimmed) {
        return TypedValue::DateTime(dt);
    }

    // Fall back to string
    TypedValue::String(trimmed.to_string())
}

/// Check if a value represents a missing/null value.
fn is_missing_value(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }

    let lower = s.to_lowercase();
    matches!(
        lower.as_str(),
        "n/a" | "na" | "null" | "nil" | "none" | "-" | "--" | "." | "#n/a" | "#null" | "nan"
    )
}

/// Try to parse a boolean value.
fn try_parse_bool(s: &str) -> Option<bool> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "true" | "yes" | "y" | "1" | "on" => Some(true),
        "false" | "no" | "n" | "0" | "off" => Some(false),
        _ => None,
    }
}

/// Try to parse a numeric value, handling currency and percentages.
fn try_parse_numeric(s: &str) -> Option<TypedValue> {
    let mut cleaned = s.to_string();

    // Check for percentage
    let is_percentage = cleaned.ends_with('%');
    if is_percentage {
        cleaned = cleaned.trim_end_matches('%').to_string();
    }

    // Remove currency symbols
    let currency_chars = ['$', '€', '£', '¥', '₹', '₽', '₩', '฿'];
    for c in &currency_chars {
        cleaned = cleaned.replace(*c, "");
    }

    // Remove thousands separators (commas)
    cleaned = cleaned.replace(',', "");

    // Remove parentheses used for negative numbers in accounting: (123) -> -123
    if cleaned.starts_with('(') && cleaned.ends_with(')') {
        cleaned = format!("-{}", &cleaned[1..cleaned.len() - 1]);
    }

    // Trim whitespace again
    cleaned = cleaned.trim().to_string();

    // Try to parse as integer first
    if let Ok(i) = cleaned.parse::<i64>() {
        if is_percentage {
            return Some(TypedValue::Float(i as f64 / 100.0));
        }
        return Some(TypedValue::Int(i));
    }

    // Try to parse as float
    if let Ok(f) = cleaned.parse::<f64>() {
        if f.is_finite() {
            if is_percentage {
                return Some(TypedValue::Float(f / 100.0));
            }
            return Some(TypedValue::Float(f));
        }
    }

    None
}

/// Try to parse a datetime value from various formats.
fn try_parse_datetime(s: &str) -> Option<String> {
    // List of date/datetime formats to try
    let datetime_formats = [
        // ISO formats
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        // US formats
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
        "%m/%d/%Y",
        "%m/%d/%y",
        "%m-%d-%Y",
        // EU formats
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%d/%m/%Y",
        "%d-%m-%Y",
        "%d.%m.%Y",
        // Month name formats
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y",
        "%d %B %Y",
        "%b %d %Y",
        "%B %d %Y",
    ];

    // Try parsing as NaiveDateTime first
    for fmt in &datetime_formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }

    // Try parsing as NaiveDate (add midnight time)
    let date_formats = [
        "%Y-%m-%d",
        "%m/%d/%Y",
        "%m/%d/%y",
        "%d/%m/%Y",
        "%d-%m-%Y",
        "%d.%m.%Y",
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y",
        "%d %B %Y",
    ];

    for fmt in &date_formats {
        if let Ok(date) = NaiveDate::parse_from_str(s, fmt) {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }

    None
}

// ============ Column Analysis ============

/// Analyze columns to determine types and gather statistics.
fn analyze_columns(headers: &[String], rows: &[Vec<TypedValue>]) -> Vec<ColumnInfo> {
    let num_columns = headers.len();
    let mut columns = Vec::with_capacity(num_columns);

    for (col_idx, header) in headers.iter().enumerate() {
        let mut type_counts: std::collections::HashMap<ColumnType, usize> =
            std::collections::HashMap::new();
        let mut null_count = 0;
        let mut sample_values: Vec<String> = Vec::new();

        for row in rows {
            if col_idx < row.len() {
                let value = &row[col_idx];
                if let Some(ct) = value.column_type() {
                    *type_counts.entry(ct).or_insert(0) += 1;
                    // Collect up to 3 sample values
                    if sample_values.len() < 3 {
                        sample_values.push(typed_value_to_sample_string(value));
                    }
                } else {
                    null_count += 1;
                }
            } else {
                null_count += 1;
            }
        }

        // Determine dominant type
        let column_type = if type_counts.is_empty() {
            ColumnType::String // All nulls
        } else if type_counts.len() == 1 {
            *type_counts.keys().next().unwrap()
        } else {
            // Multiple types - check if int/float mix (promote to float)
            let has_int = type_counts.contains_key(&ColumnType::Int);
            let has_float = type_counts.contains_key(&ColumnType::Float);
            let only_numeric = type_counts.keys().all(|t| matches!(t, ColumnType::Int | ColumnType::Float));
            
            if only_numeric && (has_int || has_float) {
                ColumnType::Float
            } else {
                ColumnType::Mixed
            }
        };

        columns.push(ColumnInfo {
            name: header.clone(),
            column_type,
            null_count,
            sample_values,
        });
    }

    columns
}

/// Convert a TypedValue to a sample string for display.
fn typed_value_to_sample_string(value: &TypedValue) -> String {
    match value {
        TypedValue::Null => "None".to_string(),
        TypedValue::Bool(b) => b.to_string(),
        TypedValue::Int(i) => i.to_string(),
        TypedValue::Float(f) => format!("{:.2}", f),
        TypedValue::DateTime(s) => s.clone(),
        TypedValue::String(s) => {
            if s.len() > 20 {
                format!("{}...", &s[..17])
            } else {
                format!("\"{}\"", s)
            }
        }
    }
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_missing_value() {
        assert!(is_missing_value(""));
        assert!(is_missing_value("N/A"));
        assert!(is_missing_value("n/a"));
        assert!(is_missing_value("null"));
        assert!(is_missing_value("NULL"));
        assert!(is_missing_value("-"));
        assert!(is_missing_value("NA"));
        assert!(is_missing_value("NaN"));
        assert!(!is_missing_value("0"));
        assert!(!is_missing_value("hello"));
    }

    #[test]
    fn test_try_parse_bool() {
        assert_eq!(try_parse_bool("true"), Some(true));
        assert_eq!(try_parse_bool("TRUE"), Some(true));
        assert_eq!(try_parse_bool("yes"), Some(true));
        assert_eq!(try_parse_bool("Y"), Some(true));
        assert_eq!(try_parse_bool("false"), Some(false));
        assert_eq!(try_parse_bool("no"), Some(false));
        assert_eq!(try_parse_bool("maybe"), None);
    }

    #[test]
    fn test_try_parse_numeric_integer() {
        assert_eq!(try_parse_numeric("123"), Some(TypedValue::Int(123)));
        assert_eq!(try_parse_numeric("-456"), Some(TypedValue::Int(-456)));
        assert_eq!(try_parse_numeric("0"), Some(TypedValue::Int(0)));
    }

    #[test]
    fn test_try_parse_numeric_float() {
        assert_eq!(try_parse_numeric("12.34"), Some(TypedValue::Float(12.34)));
        assert_eq!(try_parse_numeric("-0.5"), Some(TypedValue::Float(-0.5)));
        assert_eq!(try_parse_numeric(".5"), Some(TypedValue::Float(0.5)));
    }

    #[test]
    fn test_try_parse_numeric_currency() {
        assert_eq!(try_parse_numeric("$1,234.56"), Some(TypedValue::Float(1234.56)));
        assert_eq!(try_parse_numeric("€99.99"), Some(TypedValue::Float(99.99)));
        assert_eq!(try_parse_numeric("£1,000"), Some(TypedValue::Int(1000)));
    }

    #[test]
    fn test_try_parse_numeric_percentage() {
        assert_eq!(try_parse_numeric("50%"), Some(TypedValue::Float(0.5)));
        assert_eq!(try_parse_numeric("12.5%"), Some(TypedValue::Float(0.125)));
        assert_eq!(try_parse_numeric("100%"), Some(TypedValue::Float(1.0)));
    }

    #[test]
    fn test_try_parse_numeric_accounting_negative() {
        assert_eq!(try_parse_numeric("(123)"), Some(TypedValue::Int(-123)));
        assert_eq!(try_parse_numeric("($1,234.56)"), Some(TypedValue::Float(-1234.56)));
    }

    #[test]
    fn test_try_parse_datetime() {
        assert_eq!(
            try_parse_datetime("2024-01-15"),
            Some("2024-01-15".to_string())
        );
        assert_eq!(
            try_parse_datetime("01/15/2024"),
            Some("2024-01-15".to_string())
        );
        assert_eq!(
            try_parse_datetime("2024-01-15T10:30:00"),
            Some("2024-01-15T10:30:00".to_string())
        );
    }

    #[test]
    fn test_infer_and_convert() {
        assert_eq!(infer_and_convert(""), TypedValue::Null);
        assert_eq!(infer_and_convert("N/A"), TypedValue::Null);
        assert_eq!(infer_and_convert("true"), TypedValue::Bool(true));
        assert_eq!(infer_and_convert("123"), TypedValue::Int(123));
        assert_eq!(infer_and_convert("12.34"), TypedValue::Float(12.34));
        assert_eq!(infer_and_convert("$1,000"), TypedValue::Int(1000));
        assert_eq!(infer_and_convert("50%"), TypedValue::Float(0.5));
        assert_eq!(
            infer_and_convert("hello"),
            TypedValue::String("hello".to_string())
        );
    }

    #[test]
    fn test_column_type_display() {
        assert_eq!(format!("{}", ColumnType::Int), "int");
        assert_eq!(format!("{}", ColumnType::Float), "float");
        assert_eq!(format!("{}", ColumnType::DateTime), "datetime");
        assert_eq!(format!("{}", ColumnType::String), "str");
    }

    #[test]
    fn test_typed_value_column_type() {
        assert_eq!(TypedValue::Null.column_type(), None);
        assert_eq!(TypedValue::Bool(true).column_type(), Some(ColumnType::Bool));
        assert_eq!(TypedValue::Int(42).column_type(), Some(ColumnType::Int));
        assert_eq!(TypedValue::Float(3.14).column_type(), Some(ColumnType::Float));
        assert_eq!(TypedValue::DateTime("2024-01-15".to_string()).column_type(), Some(ColumnType::DateTime));
        assert_eq!(TypedValue::String("hello".to_string()).column_type(), Some(ColumnType::String));
    }

    #[test]
    fn test_infer_and_convert_rows() {
        let raw_rows = vec![
            vec!["Alice".to_string(), "30".to_string(), "$1,000.00".to_string()],
            vec!["Bob".to_string(), "N/A".to_string(), "€2,500.50".to_string()],
        ];
        
        let typed = infer_and_convert_rows(&raw_rows);
        
        assert_eq!(typed.len(), 2);
        assert_eq!(typed[0][0], TypedValue::String("Alice".to_string()));
        assert_eq!(typed[0][1], TypedValue::Int(30));
        assert_eq!(typed[0][2], TypedValue::Float(1000.0));
        assert_eq!(typed[1][1], TypedValue::Null); // N/A converted to Null
        assert_eq!(typed[1][2], TypedValue::Float(2500.50));
    }

    #[test]
    fn test_analyze_columns() {
        let headers = vec!["name".to_string(), "age".to_string(), "salary".to_string()];
        let rows = vec![
            vec![TypedValue::String("Alice".to_string()), TypedValue::Int(30), TypedValue::Float(50000.0)],
            vec![TypedValue::String("Bob".to_string()), TypedValue::Null, TypedValue::Float(60000.0)],
            vec![TypedValue::String("Carol".to_string()), TypedValue::Int(28), TypedValue::Float(55000.0)],
        ];
        
        let columns = analyze_columns(&headers, &rows);
        
        assert_eq!(columns.len(), 3);
        assert_eq!(columns[0].name, "name");
        assert_eq!(columns[0].column_type, ColumnType::String);
        assert_eq!(columns[0].null_count, 0);
        
        assert_eq!(columns[1].name, "age");
        assert_eq!(columns[1].column_type, ColumnType::Int);
        assert_eq!(columns[1].null_count, 1); // One null value
        
        assert_eq!(columns[2].name, "salary");
        assert_eq!(columns[2].column_type, ColumnType::Float);
        assert_eq!(columns[2].null_count, 0);
    }

    #[test]
    fn test_analyze_columns_mixed_int_float_promotes_to_float() {
        let headers = vec!["value".to_string()];
        let rows = vec![
            vec![TypedValue::Int(100)],
            vec![TypedValue::Float(3.14)],
            vec![TypedValue::Int(200)],
        ];
        
        let columns = analyze_columns(&headers, &rows);
        
        // Mixed int/float should promote to float
        assert_eq!(columns[0].column_type, ColumnType::Float);
    }

    #[test]
    fn test_try_parse_datetime_us_format() {
        // US format: MM/DD/YYYY
        assert_eq!(try_parse_datetime("12/25/2024"), Some("2024-12-25".to_string()));
        assert_eq!(try_parse_datetime("1/5/2024"), Some("2024-01-05".to_string()));
    }

    #[test]
    fn test_try_parse_datetime_with_time() {
        assert_eq!(
            try_parse_datetime("2024-01-15 14:30:00"),
            Some("2024-01-15T14:30:00".to_string())
        );
    }

    #[test]
    fn test_whitespace_handling() {
        // Leading/trailing whitespace should be trimmed
        assert_eq!(infer_and_convert("  123  "), TypedValue::Int(123));
        assert_eq!(infer_and_convert("  hello  "), TypedValue::String("hello".to_string()));
        assert_eq!(infer_and_convert("  "), TypedValue::Null);
    }

    #[test]
    fn test_typed_value_serialization() {
        // Test JSON serialization of TypedValue
        let values = vec![
            TypedValue::Null,
            TypedValue::Bool(true),
            TypedValue::Int(42),
            TypedValue::Float(3.14),
            TypedValue::String("hello".to_string()),
        ];
        
        let json = serde_json::to_string(&values).unwrap();
        assert!(json.contains("null"));
        assert!(json.contains("true"));
        assert!(json.contains("42"));
        assert!(json.contains("3.14"));
        assert!(json.contains("\"hello\""));
    }

    #[test]
    fn test_column_info_serialization() {
        let col = ColumnInfo {
            name: "test".to_string(),
            column_type: ColumnType::Int,
            null_count: 5,
            sample_values: vec!["1".to_string(), "2".to_string()],
        };
        
        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"column_type\":\"int\""));
        assert!(json.contains("\"null_count\":5"));
    }
}
