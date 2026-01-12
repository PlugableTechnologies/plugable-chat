//! File processing for RAG document ingestion.
//!
//! This module handles:
//! - Multi-format text extraction (PDF, DOCX, CSV, JSON, TXT, MD)
//! - Document parsing into hierarchical elements
//! - File type detection and validation

use crate::protocol::RagProgressEvent;
use std::collections::HashMap;
use std::path::Path;
use tauri::{AppHandle, Emitter};

use super::document_chunker::DocumentElement;
use super::pdf_extractor::{
    extract_pdf_heading_structure, normalize_heading_text_for_matching, PdfHeading,
};

/// Check if a file is a supported RAG file type
pub fn is_rag_supported_file_type(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "txt" | "csv" | "tsv" | "md" | "json" | "pdf" | "docx"
        )
    } else {
        false
    }
}

/// Extract text from a file based on its type
pub fn extract_text_from_file(
    file_path: &Path,
    content: &str,
    file_index: usize,
    total_files: usize,
    app_handle: Option<&AppHandle>,
) -> Result<String, String> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "csv" => parse_csv_to_text(content, ','),
        "tsv" => parse_csv_to_text(content, '\t'),
        "json" => parse_json_to_text(content),
        "pdf" => extract_pdf_text_with_progress_events(file_path, file_index, total_files, app_handle),
        "docx" => extract_docx_text_content(file_path),
        _ => Ok(content.to_string()),
    }
}

/// Fallback PDF text extraction using lopdf when pdf-extract fails.
/// Less accurate for complex fonts but more tolerant of malformed PDFs.
pub fn extract_pdf_text_via_lopdf(file_path: &Path) -> Result<String, String> {
    use lopdf::{Document, Object};

    let doc = Document::load(file_path).map_err(|e| format!("Failed to load PDF: {}", e))?;

    let mut all_text = String::new();
    let pages = doc.get_pages();

    for (_page_num, page_id) in pages {
        if let Ok(content) = doc.get_page_content(page_id) {
            let operations = lopdf::content::Content::decode(&content)
                .map(|c| c.operations)
                .unwrap_or_default();

            for op in operations {
                match op.operator.as_str() {
                    // Tj: Show text string
                    "Tj" => {
                        if let Some(Object::String(bytes, _)) = op.operands.first() {
                            // Try UTF-8 first, then Latin-1 fallback
                            let text = String::from_utf8(bytes.clone())
                                .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
                            all_text.push_str(&text);
                        }
                    }
                    // TJ: Show text array (with kerning)
                    "TJ" => {
                        if let Some(Object::Array(arr)) = op.operands.first() {
                            for item in arr {
                                if let Object::String(bytes, _) = item {
                                    let text = String::from_utf8(bytes.clone())
                                        .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
                                    all_text.push_str(&text);
                                }
                            }
                        }
                    }
                    // Text positioning that indicates new line/paragraph
                    "Td" | "TD" | "T*" | "'" | "\"" => {
                        if !all_text.ends_with('\n') && !all_text.ends_with(' ') {
                            all_text.push(' ');
                        }
                    }
                    "ET" => {
                        // End of text block - add newline
                        if !all_text.ends_with('\n') {
                            all_text.push('\n');
                        }
                    }
                    _ => {}
                }
            }
        }
        all_text.push('\n'); // Page break
    }

    Ok(all_text)
}

/// Extract PDF text with progress events
pub fn extract_pdf_text_with_progress_events(
    file_path: &Path,
    file_index: usize,
    total_files: usize,
    app_handle: Option<&AppHandle>,
) -> Result<String, String> {
    // pdf-extract has better font encoding handling than raw lopdf
    // Use catch_unwind to capture panics from pdf-extract library
    let pages_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_by_pages(file_path)
    }));

    let pages = match pages_result {
        Ok(Ok(pages)) => pages,
        Ok(Err(e)) => {
            // Try lopdf fallback
            println!(
                "[RAG] pdf-extract failed for {:?}, trying lopdf fallback: {}",
                file_path.file_name().unwrap_or_default(),
                e
            );
            match extract_pdf_text_via_lopdf(file_path) {
                Ok(text) => {
                    println!("[RAG] lopdf fallback succeeded, extracted {} chars", text.len());
                    return Ok(text);
                }
                Err(_fallback_err) => {
                    let filename = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    return Err(format!(
                        "Cannot read '{}': This PDF has an incompatible format. Re-attaching it won't help. Try re-exporting the PDF from its source application.",
                        filename
                    ));
                }
            }
        }
        Err(panic_payload) => {
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };

            // Try lopdf fallback after panic
            println!(
                "[RAG] pdf-extract panicked for {:?}, trying lopdf fallback: {}",
                file_path.file_name().unwrap_or_default(),
                panic_msg
            );
            match extract_pdf_text_via_lopdf(file_path) {
                Ok(text) => {
                    println!("[RAG] lopdf fallback succeeded, extracted {} chars", text.len());
                    return Ok(text);
                }
                Err(_fallback_err) => {
                    let filename = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    return Err(format!(
                        "Cannot read '{}': This PDF has an incompatible format. Re-attaching it won't help. Try re-exporting the PDF from its source application.",
                        filename
                    ));
                }
            }
        }
    };

    let total_pages = pages.len() as u32;
    let mut extracted_text = String::new();

    for (i, page_text) in pages.iter().enumerate() {
        extracted_text.push_str(page_text);
        extracted_text.push('\n');

        // Emit progress every 5 pages or on last page
        if i % 5 == 0 || i == pages.len() - 1 {
            let progress = ((i + 1) as f32 / pages.len() as f32 * 100.0) as u8;
            if let Some(handle) = app_handle {
                let _ = handle.emit(
                    "rag-progress",
                    RagProgressEvent {
                        phase: "extracting_text".to_string(),
                        total_files,
                        processed_files: file_index,
                        total_chunks: 0,
                        processed_chunks: 0,
                        current_file: file_path.to_string_lossy().to_string(),
                        is_complete: false,
                        extraction_progress: Some(progress),
                        extraction_total_pages: Some(total_pages),
                        compute_device: None,
                    },
                );
            }
        }
    }

    Ok(extracted_text)
}

/// Extract text content from a DOCX file
pub fn extract_docx_text_content(file_path: &Path) -> Result<String, String> {
    use std::io::Read;

    let file =
        std::fs::File::open(file_path).map_err(|e| format!("Failed to open DOCX: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Invalid DOCX archive: {}", e))?;

    let mut doc_xml = archive
        .by_name("word/document.xml")
        .map_err(|_| "No document.xml found in DOCX".to_string())?;

    let mut xml_content = String::new();
    doc_xml
        .read_to_string(&mut xml_content)
        .map_err(|e| format!("Failed to read document.xml: {}", e))?;

    Ok(extract_plaintext_from_docx_xml(&xml_content))
}

/// Parse CSV content to text with header-value pairs
pub fn parse_csv_to_text(content: &str, delimiter: char) -> Result<String, String> {
    let mut result = String::new();
    let mut lines = content.lines();

    let header: Vec<&str> = if let Some(header_line) = lines.next() {
        header_line.split(delimiter).collect()
    } else {
        return Ok(String::new());
    };

    for line in lines {
        let values: Vec<&str> = line.split(delimiter).collect();
        let mut row_text = String::new();

        for (i, value) in values.iter().enumerate() {
            if i < header.len() && !value.trim().is_empty() {
                if !row_text.is_empty() {
                    row_text.push_str(", ");
                }
                row_text.push_str(&format!("{}: {}", header[i].trim(), value.trim()));
            }
        }

        if !row_text.is_empty() {
            result.push_str(&row_text);
            result.push('\n');
        }
    }

    Ok(result)
}

/// Parse JSON content to readable text
pub fn parse_json_to_text(content: &str) -> Result<String, String> {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => Ok(convert_json_value_to_text(&value, "")),
        Err(_) => Ok(content.to_string()),
    }
}

/// Convert JSON value to readable text with path prefixes
pub fn convert_json_value_to_text(value: &serde_json::Value, prefix: &str) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut result = String::new();
            for (key, val) in map {
                let new_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                result.push_str(&convert_json_value_to_text(val, &new_prefix));
            }
            result
        }
        serde_json::Value::Array(arr) => {
            let mut result = String::new();
            for (i, val) in arr.iter().enumerate() {
                let new_prefix = format!("{}[{}]", prefix, i);
                result.push_str(&convert_json_value_to_text(val, &new_prefix));
            }
            result
        }
        serde_json::Value::String(s) => format!("{}: {}\n", prefix, s),
        serde_json::Value::Number(n) => format!("{}: {}\n", prefix, n),
        serde_json::Value::Bool(b) => format!("{}: {}\n", prefix, b),
        serde_json::Value::Null => String::new(),
    }
}

/// Parse document into structured elements based on file type
pub fn parse_document_to_elements(
    extension: &str,
    content: &str,
    file_path: Option<&Path>,
) -> Vec<DocumentElement> {
    match extension {
        "md" => parse_markdown_to_elements(content),
        "docx" => parse_docx_to_elements(content),
        "pdf" => parse_pdf_to_elements(content, file_path),
        "txt" => parse_plaintext_to_elements(content),
        _ => parse_plaintext_to_elements(content),
    }
}

/// Parse Markdown document into elements
pub fn parse_markdown_to_elements(content: &str) -> Vec<DocumentElement> {
    let mut elements = Vec::new();
    let mut current_paragraph = String::new();
    let mut in_code_block = false;
    let mut code_block_content = String::new();

    for line in content.lines() {
        // Handle code blocks
        if line.starts_with("```") {
            if in_code_block {
                // End of code block
                elements.push(DocumentElement::CodeBlock(
                    code_block_content.trim().to_string(),
                ));
                code_block_content.clear();
                in_code_block = false;
            } else {
                // Start of code block - flush paragraph first
                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(
                        current_paragraph.trim().to_string(),
                    ));
                    current_paragraph.clear();
                }
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            if !code_block_content.is_empty() {
                code_block_content.push('\n');
            }
            code_block_content.push_str(line);
            continue;
        }

        // Handle headings
        if let Some(level) = detect_markdown_heading_level(line) {
            // Flush current paragraph
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            let text = line.trim_start_matches('#').trim().to_string();
            elements.push(DocumentElement::Heading { level, text });
            continue;
        }

        // Handle list items
        if let Some((indent, text)) = detect_markdown_list_item(line) {
            // Flush current paragraph
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            elements.push(DocumentElement::ListItem { indent, text });
            continue;
        }

        // Handle blank lines
        if line.trim().is_empty() {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            continue;
        }

        // Accumulate paragraph text
        if !current_paragraph.is_empty() {
            current_paragraph.push(' ');
        }
        current_paragraph.push_str(line.trim());
    }

    // Flush remaining paragraph
    if !current_paragraph.is_empty() {
        elements.push(DocumentElement::Paragraph(
            current_paragraph.trim().to_string(),
        ));
    }

    elements
}

/// Detect markdown heading level from a line
pub fn detect_markdown_heading_level(line: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        let level = trimmed.chars().take_while(|&c| c == '#').count();
        if level >= 1 && level <= 6 && trimmed.chars().nth(level) == Some(' ') {
            return Some(level as u8);
        }
    }
    None
}

/// Detect markdown list item from a line
pub fn detect_markdown_list_item(line: &str) -> Option<(u8, String)> {
    let leading_spaces = line.len() - line.trim_start().len();
    let indent = (leading_spaces / 2) as u8;
    let trimmed = line.trim_start();

    // Bullet list: - item, * item, + item
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        return Some((indent, trimmed[2..].to_string()));
    }

    // Numbered list: 1. item, 2. item, etc.
    if let Some(dot_pos) = trimmed.find(". ") {
        if dot_pos > 0 && trimmed[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
            return Some((indent, trimmed[dot_pos + 2..].to_string()));
        }
    }

    None
}

/// Parse DOCX content (already extracted to text) - uses plaintext parsing
pub fn parse_docx_to_elements(content: &str) -> Vec<DocumentElement> {
    parse_plaintext_to_elements(content)
}

/// Parse PDF content using hybrid structure extraction
pub fn parse_pdf_to_elements(content: &str, file_path: Option<&Path>) -> Vec<DocumentElement> {
    // Try hybrid structure extraction if file path is available
    if let Some(path) = file_path {
        if let Ok(headings) = extract_pdf_heading_structure(path) {
            if !headings.is_empty() {
                // Validate that heading titles actually appear in the content
                let normalized_content = normalize_heading_text_for_matching(content);
                let matching_headings: Vec<_> = headings
                    .iter()
                    .filter(|h| {
                        let normalized_title = normalize_heading_text_for_matching(&h.title);
                        normalized_content.contains(&normalized_title)
                    })
                    .cloned()
                    .collect();

                // Only use font-size headings if most of them match the content
                if matching_headings.len() >= headings.len() / 2 && !matching_headings.is_empty() {
                    return merge_pdf_headings_with_content(&matching_headings, content);
                }
            }
        }
    }

    // Fall back to text-based heuristics
    parse_pdf_to_elements_by_heuristics(content)
}

/// Merge extracted PDF headings with text content
pub fn merge_pdf_headings_with_content(
    headings: &[PdfHeading],
    content: &str,
) -> Vec<DocumentElement> {
    let mut elements = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    // Create a map of normalized heading titles to their levels
    let heading_map: HashMap<String, u8> = headings
        .iter()
        .map(|h| (normalize_heading_text_for_matching(&h.title), h.level))
        .collect();

    let mut current_paragraph = String::new();

    for line in lines {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            continue;
        }

        let normalized = normalize_heading_text_for_matching(trimmed);

        // Check if this line matches a known heading
        if let Some(&level) = heading_map.get(&normalized) {
            // Flush paragraph before heading
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            elements.push(DocumentElement::Heading {
                level,
                text: trimmed.to_string(),
            });
        } else {
            // Also check for partial matches
            let is_heading = heading_map
                .iter()
                .any(|(h_title, _)| normalized.starts_with(h_title) || h_title.starts_with(&normalized));

            if is_heading {
                let level = heading_map
                    .iter()
                    .find(|(h_title, _)| {
                        normalized.starts_with(*h_title) || h_title.starts_with(&normalized)
                    })
                    .map(|(_, &l)| l)
                    .unwrap_or(2);

                if !current_paragraph.is_empty() {
                    elements.push(DocumentElement::Paragraph(
                        current_paragraph.trim().to_string(),
                    ));
                    current_paragraph.clear();
                }
                elements.push(DocumentElement::Heading {
                    level,
                    text: trimmed.to_string(),
                });
            } else {
                // Accumulate paragraph
                if !current_paragraph.is_empty() {
                    current_paragraph.push(' ');
                }
                current_paragraph.push_str(trimmed);
            }
        }
    }

    // Flush final paragraph
    if !current_paragraph.is_empty() {
        elements.push(DocumentElement::Paragraph(
            current_paragraph.trim().to_string(),
        ));
    }

    elements
}

/// Parse PDF content using text-based heuristics (fallback)
pub fn parse_pdf_to_elements_by_heuristics(content: &str) -> Vec<DocumentElement> {
    let mut elements = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut current_paragraph = String::new();
    let mut prev_blank = true;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            prev_blank = true;
            continue;
        }

        // Detect heading level using heuristics
        let next_line = lines.get(i + 1).copied();
        if let Some(level) = detect_pdf_heading_level_from_text(trimmed, prev_blank, next_line) {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            elements.push(DocumentElement::Heading {
                level,
                text: trimmed.to_string(),
            });
            prev_blank = false;
            continue;
        }

        // Accumulate paragraph
        if !current_paragraph.is_empty() {
            current_paragraph.push(' ');
        }
        current_paragraph.push_str(trimmed);
        prev_blank = false;
    }

    if !current_paragraph.is_empty() {
        elements.push(DocumentElement::Paragraph(
            current_paragraph.trim().to_string(),
        ));
    }

    elements
}

/// Detect PDF heading level based on structural heuristics
pub fn detect_pdf_heading_level_from_text(
    line: &str,
    prev_blank: bool,
    next_line: Option<&str>,
) -> Option<u8> {
    let len = line.len();
    let has_alpha = line.chars().any(|c| c.is_alphabetic());

    // Must have alphabetic content and reasonable length
    if !has_alpha || len < 3 || len > 100 {
        return None;
    }

    // Structural signals
    let is_all_caps = line
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_uppercase());
    let ends_with_sentence_punct = line.ends_with('.') || line.ends_with('?') || line.ends_with('!');
    let ends_with_colon = line.ends_with(':');
    let next_is_blank = next_line.map_or(true, |n| n.trim().is_empty());
    let words: Vec<&str> = line.split_whitespace().collect();
    let word_count = words.len();

    // Headings typically don't end with sentence punctuation
    if ends_with_sentence_punct {
        return None;
    }

    // Count Title Case words
    let title_case_count = words
        .iter()
        .filter(|w| w.chars().next().map_or(false, |c| c.is_uppercase()))
        .count();
    let is_title_case = word_count >= 2 && title_case_count > word_count / 2;

    // H1: ALL CAPS, short, standalone
    if is_all_caps && len < 40 && prev_blank && next_is_blank {
        return Some(1);
    }

    // H2: ALL CAPS (not standalone) or short standalone Title Case
    if is_all_caps && len > 3 && len < 60 {
        return Some(2);
    }
    if is_title_case && len < 40 && prev_blank && word_count <= 6 {
        return Some(2);
    }

    // H3: Title Case, medium length, or ends with colon
    if is_title_case && len < 60 && word_count >= 2 && word_count <= 8 {
        return Some(3);
    }
    if ends_with_colon && len < 50 && word_count <= 6 {
        return Some(3);
    }

    // H4: Short lines that look like labels/headers
    if len < 50 && word_count >= 2 && word_count <= 8 {
        let first_cap = words
            .first()
            .and_then(|w| w.chars().next())
            .map_or(false, |c| c.is_uppercase());
        if first_cap && next_is_blank {
            return Some(4);
        }
    }

    None
}

/// Parse plain text document into elements
pub fn parse_plaintext_to_elements(content: &str) -> Vec<DocumentElement> {
    let mut elements = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut current_paragraph = String::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            continue;
        }

        // Heuristic: ALL CAPS lines or lines followed by underlines may be headings
        let next_line = lines.get(i + 1).copied();
        if looks_like_plaintext_heading(trimmed, next_line) {
            if !current_paragraph.is_empty() {
                elements.push(DocumentElement::Paragraph(
                    current_paragraph.trim().to_string(),
                ));
                current_paragraph.clear();
            }
            elements.push(DocumentElement::Heading {
                level: 2,
                text: trimmed.to_string(),
            });
            continue;
        }

        // Skip underlines (===, ---)
        if trimmed.chars().all(|c| c == '=' || c == '-') && trimmed.len() > 3 {
            continue;
        }

        // Accumulate paragraph
        if !current_paragraph.is_empty() {
            current_paragraph.push(' ');
        }
        current_paragraph.push_str(trimmed);
    }

    if !current_paragraph.is_empty() {
        elements.push(DocumentElement::Paragraph(
            current_paragraph.trim().to_string(),
        ));
    }

    elements
}

/// Check if a line looks like a plaintext heading
pub fn looks_like_plaintext_heading(line: &str, next_line: Option<&str>) -> bool {
    // ALL CAPS
    let is_all_caps = line
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_uppercase())
        && line.len() > 3
        && line.len() < 80;

    // Followed by underline
    let followed_by_underline = next_line.map_or(false, |n| {
        let n = n.trim();
        n.len() > 3 && n.chars().all(|c| c == '=' || c == '-')
    });

    is_all_caps || followed_by_underline
}

/// Extract text content from DOCX XML (word/document.xml)
pub fn extract_plaintext_from_docx_xml(xml: &str) -> String {
    let mut result = String::new();
    let mut in_text = false;
    let mut chars = xml.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                tag.push(tc);
            }

            if tag.starts_with("w:t") && !tag.starts_with("w:t/") && !tag.ends_with('/') {
                in_text = true;
            } else if tag == "/w:t" {
                in_text = false;
            } else if tag.starts_with("w:p") && !tag.starts_with("w:p/") && !tag.ends_with('/') {
                if !result.is_empty() && !result.ends_with('\n') {
                    result.push('\n');
                }
            }
        } else if in_text {
            result.push(c);
        }
    }

    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown_headings() {
        let content = "# Title\n\nSome text\n\n## Section\n\nMore text";
        let elements = parse_markdown_to_elements(content);
        assert!(matches!(elements[0], DocumentElement::Heading { level: 1, .. }));
        assert!(matches!(elements[2], DocumentElement::Heading { level: 2, .. }));
    }

    #[test]
    fn test_parse_markdown_bullets() {
        let content = "- Item 1\n- Item 2\n- Item 3";
        let elements = parse_markdown_to_elements(content);
        assert_eq!(elements.len(), 3);
        assert!(matches!(elements[0], DocumentElement::ListItem { .. }));
    }

    #[test]
    fn test_parse_markdown_code_blocks() {
        let content = "Some text\n\n```python\nprint('hello')\n```\n\nMore text";
        let elements = parse_markdown_to_elements(content);
        assert!(elements.iter().any(|e| matches!(e, DocumentElement::CodeBlock(_))));
    }

    #[test]
    fn test_detect_markdown_heading_level() {
        assert_eq!(detect_markdown_heading_level("# Title"), Some(1));
        assert_eq!(detect_markdown_heading_level("## Section"), Some(2));
        assert_eq!(detect_markdown_heading_level("### Subsection"), Some(3));
        assert_eq!(detect_markdown_heading_level("Not a heading"), None);
    }

    #[test]
    fn test_is_rag_supported_file_type() {
        assert!(is_rag_supported_file_type(Path::new("test.pdf")));
        assert!(is_rag_supported_file_type(Path::new("test.docx")));
        assert!(is_rag_supported_file_type(Path::new("test.md")));
        assert!(!is_rag_supported_file_type(Path::new("test.exe")));
    }
}
