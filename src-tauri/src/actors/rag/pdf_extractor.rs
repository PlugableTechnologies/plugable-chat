//! PDF structure extraction for RAG document processing.
//!
//! This module handles:
//! - Extracting heading structure from PDF bookmarks (Tier 1)
//! - Detecting headings by font size analysis (Tier 2)
//! - PDF string decoding (UTF-8, UTF-16BE/LE, PDFDocEncoding)

use std::collections::HashMap;
use std::path::Path;

/// Extracted heading from PDF with explicit level (from bookmarks or font size)
#[derive(Debug, Clone)]
pub struct PdfHeading {
    pub level: u8,      // 1-4
    pub title: String,
    #[allow(dead_code)]
    pub page: Option<u32>,
}

/// Extract structure from PDF using hybrid approach:
/// 1. Try bookmarks/outlines first (explicit hierarchy from PDF metadata)
/// 2. Fall back to font-size detection (infer hierarchy from typography)
pub fn extract_pdf_heading_structure(path: &Path) -> Result<Vec<PdfHeading>, String> {
    // Tier 1: Try bookmarks first (most reliable when available)
    match extract_pdf_bookmarks(path) {
        Ok(headings) if !headings.is_empty() => {
            println!("RagActor: Found {} bookmarks in PDF", headings.len());
            return Ok(headings);
        }
        Ok(_) => {
            println!("RagActor: No bookmarks found, trying font-size detection");
        }
        Err(e) => {
            println!("RagActor: Bookmark extraction failed ({}), trying font-size detection", e);
        }
    }
    
    // Tier 2: Fall back to font-size detection
    match extract_pdf_by_font_size(path) {
        Ok(headings) if !headings.is_empty() => {
            println!("RagActor: Detected {} headings from font sizes", headings.len());
            Ok(headings)
        }
        Ok(_) => {
            println!("RagActor: No font-size headings detected, will use text heuristics");
            Ok(Vec::new())
        }
        Err(e) => {
            println!("RagActor: Font-size detection failed ({}), will use text heuristics", e);
            Ok(Vec::new())
        }
    }
}

/// Extract PDF bookmarks/outlines (Tier 1)
/// Uses lopdf to read the document outline tree
fn extract_pdf_bookmarks(path: &Path) -> Result<Vec<PdfHeading>, String> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| format!("Failed to load PDF: {}", e))?;
    
    let mut headings = Vec::new();
    
    // Recursively traverse bookmark tree
    fn traverse_bookmarks(
        doc: &lopdf::Document,
        bookmark_ids: &[u32],
        level: u8,
        headings: &mut Vec<PdfHeading>,
    ) {
        for &id in bookmark_ids {
            if let Some(bookmark) = doc.bookmark_table.get(&id) {
                // bookmark.page is (page_num, y_offset) tuple - extract page number
                let page_num = Some(bookmark.page.0);
                headings.push(PdfHeading {
                    level: level.min(4), // Cap at H4
                    title: bookmark.title.clone(),
                    page: page_num,
                });
                // Recurse into children
                traverse_bookmarks(doc, &bookmark.children, level + 1, headings);
            }
        }
    }
    
    traverse_bookmarks(&doc, &doc.bookmarks, 1, &mut headings);
    Ok(headings)
}

/// Decode PDF string bytes to a Rust String
/// PDF strings can be UTF-8, UTF-16BE (with BOM 0xFEFF), or PDFDocEncoding
pub fn decode_pdf_bytes_to_string(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    
    // Check for UTF-16BE BOM (0xFE 0xFF)
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        // UTF-16BE with BOM - skip the BOM and decode
        let utf16_chars: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter_map(|chunk| {
                if chunk.len() == 2 {
                    Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                } else {
                    None
                }
            })
            .collect();
        return String::from_utf16(&utf16_chars).ok();
    }
    
    // Check for UTF-16LE pattern (common: alternating null bytes with ASCII)
    // Pattern: 0xXX 0x00 0xYY 0x00 where XX, YY are ASCII
    if bytes.len() >= 4 {
        let looks_like_utf16le = bytes.chunks(2).take(4).all(|chunk| {
            chunk.len() == 2 && chunk[1] == 0 && chunk[0] < 128
        });
        if looks_like_utf16le {
            let utf16_chars: Vec<u16> = bytes
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() == 2 {
                        Some(u16::from_le_bytes([chunk[0], chunk[1]]))
                    } else {
                        None
                    }
                })
                .collect();
            if let Ok(s) = String::from_utf16(&utf16_chars) {
                // Filter out control characters
                let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    
    // Check for UTF-16BE pattern without BOM (alternating 0x00 0xXX)
    if bytes.len() >= 4 {
        let looks_like_utf16be = bytes.chunks(2).take(4).all(|chunk| {
            chunk.len() == 2 && chunk[0] == 0 && chunk[1] < 128
        });
        if looks_like_utf16be {
            let utf16_chars: Vec<u16> = bytes
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() == 2 {
                        Some(u16::from_be_bytes([chunk[0], chunk[1]]))
                    } else {
                        None
                    }
                })
                .collect();
            if let Ok(s) = String::from_utf16(&utf16_chars) {
                let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }
    
    // Try UTF-8 first
    if let Ok(s) = String::from_utf8(bytes.to_vec()) {
        let cleaned: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    
    // Fall back to Latin-1 / PDFDocEncoding (treat each byte as Unicode codepoint)
    let s: String = bytes.iter()
        .filter_map(|&b| {
            let c = b as char;
            if c.is_control() && c != ' ' { None } else { Some(c) }
        })
        .collect();
    if !s.is_empty() { Some(s) } else { None }
}

/// Extract headings by font size (Tier 2)
/// Parses PDF content streams to find text with different font sizes,
/// then maps the largest fonts to heading levels H1-H4
fn extract_pdf_by_font_size(path: &Path) -> Result<Vec<PdfHeading>, String> {
    use lopdf::{Document, Object};
    
    let doc = Document::load(path)
        .map_err(|e| format!("Failed to load PDF: {}", e))?;
    
    // Collect all text runs with their font sizes
    let mut text_runs: Vec<(f32, String)> = Vec::new();
    
    for (page_num, page_id) in doc.get_pages() {
        if let Ok(content) = doc.get_page_content(page_id) {
            let operations = lopdf::content::Content::decode(&content)
                .map(|c| c.operations)
                .unwrap_or_default();
            
            let mut current_font_size: f32 = 12.0; // Default
            let mut current_text = String::new();
            
            for op in operations {
                match op.operator.as_str() {
                    // Tf: Set text font and size
                    "Tf" => {
                        // Flush previous text if any
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                        // Extract font size (second operand)
                        if let Some(Object::Real(size)) = op.operands.get(1) {
                            current_font_size = *size as f32;
                        } else if let Some(Object::Integer(size)) = op.operands.get(1) {
                            current_font_size = *size as f32;
                        }
                    }
                    // Tj: Show text string
                    "Tj" => {
                        if let Some(Object::String(bytes, _)) = op.operands.first() {
                            if let Some(text) = decode_pdf_bytes_to_string(bytes) {
                                current_text.push_str(&text);
                            }
                        }
                    }
                    // TJ: Show text array (with kerning)
                    "TJ" => {
                        if let Some(Object::Array(arr)) = op.operands.first() {
                            for item in arr {
                                if let Object::String(bytes, _) = item {
                                    if let Some(text) = decode_pdf_bytes_to_string(bytes) {
                                        current_text.push_str(&text);
                                    }
                                }
                            }
                        }
                    }
                    // Text positioning operators that indicate new line/block
                    "Td" | "TD" | "T*" | "'" | "\"" => {
                        // Flush current text as a separate run
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                    }
                    // BT/ET: Begin/End text object
                    "BT" => {
                        current_text.clear();
                    }
                    "ET" => {
                        if !current_text.trim().is_empty() {
                            text_runs.push((current_font_size, current_text.trim().to_string()));
                            current_text.clear();
                        }
                    }
                    _ => {}
                }
            }
            
            // Flush any remaining text
            if !current_text.trim().is_empty() {
                text_runs.push((current_font_size, current_text.trim().to_string()));
            }
        }
        
        // Limit to first few pages to avoid excessive processing
        if page_num > 10 {
            break;
        }
    }
    
    if text_runs.is_empty() {
        return Ok(Vec::new());
    }
    
    // Determine heading levels based on font size distribution
    // Collect unique font sizes and sort descending
    let mut sizes: Vec<f32> = text_runs.iter().map(|(s, _)| *s).collect();
    sizes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.5); // Treat similar sizes as same
    
    // Find the body text size (most common size, typically)
    let mut size_counts: HashMap<i32, usize> = HashMap::new();
    for (size, _) in &text_runs {
        *size_counts.entry((*size * 10.0) as i32).or_insert(0) += 1;
    }
    let body_size = size_counts.iter()
        .max_by_key(|(_, count)| *count)
        .map(|(size, _)| *size as f32 / 10.0)
        .unwrap_or(12.0);
    
    // Only consider sizes larger than body text as headings
    let heading_sizes: Vec<f32> = sizes.into_iter()
        .filter(|&s| s > body_size + 1.0)
        .take(4) // Max 4 heading levels
        .collect();
    
    if heading_sizes.is_empty() {
        return Ok(Vec::new());
    }
    
    // Map font sizes to heading levels
    let size_to_level: HashMap<i32, u8> = heading_sizes.iter()
        .enumerate()
        .map(|(i, &s)| ((s * 10.0) as i32, (i + 1) as u8))
        .collect();
    
    // Convert text runs to headings (only for heading-sized text)
    let headings: Vec<PdfHeading> = text_runs.iter()
        .filter_map(|(size, text)| {
            let size_key = (*size * 10.0) as i32;
            size_to_level.get(&size_key).map(|&level| PdfHeading {
                level,
                title: text.clone(),
                page: None,
            })
        })
        .filter(|h| !h.title.is_empty() && h.title.len() < 200) // Filter out very long text
        .collect();
    
    Ok(headings)
}

/// Normalize text for matching (lowercase, collapse whitespace)
pub fn normalize_heading_text_for_matching(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdf_heading_struct() {
        let heading = PdfHeading {
            level: 1,
            title: "Chapter 1".to_string(),
            page: Some(5),
        };
        assert_eq!(heading.level, 1);
        assert_eq!(heading.title, "Chapter 1");
    }

    #[test]
    fn test_normalize_heading_text_for_matching() {
        assert_eq!(normalize_heading_text_for_matching("  Hello   World  "), "hello world");
        assert_eq!(normalize_heading_text_for_matching("TEST"), "test");
    }
}
