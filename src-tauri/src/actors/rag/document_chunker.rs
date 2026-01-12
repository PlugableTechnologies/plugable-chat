//! Document chunking for RAG with heading hierarchy preservation.
//!
//! This module handles:
//! - Document element types for hierarchical parsing
//! - Heading stack management for context tracking
//! - Semantic chunking that respects paragraph boundaries
//! - Sentence and word-level splitting for long content

/// Soft limit for chunk size in characters (target size)
pub const CHUNK_SOFT_LIMIT: usize = 400;

/// Hard limit for chunk size in characters (never exceed)
pub const CHUNK_HARD_LIMIT: usize = 800;

/// Represents a parsed element from a document
#[derive(Debug, Clone)]
pub enum DocumentElement {
    /// A heading with level (1-6) and text
    Heading { level: u8, text: String },
    /// A paragraph of text
    Paragraph(String),
    /// A list item with indent level and text
    ListItem {
        #[allow(dead_code)]
        indent: u8,
        text: String,
    },
    /// A code block
    CodeBlock(String),
}

/// Maintains the current heading hierarchy as document is parsed
pub struct HeadingStackManager {
    stack: Vec<(u8, String)>, // (level, heading_text)
}

impl HeadingStackManager {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a heading, popping any headings at same or lower level
    pub fn push_heading(&mut self, level: u8, text: String) {
        // Pop all headings at same or higher level number (lower priority)
        while self.stack.last().map_or(false, |(l, _)| *l >= level) {
            self.stack.pop();
        }
        self.stack.push((level, text));
    }

    /// Get the current heading context as a formatted string
    pub fn get_context(&self) -> String {
        if self.stack.is_empty() {
            return String::new();
        }
        self.stack
            .iter()
            .map(|(l, t)| format!("H{}: {}", l, t))
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

impl Default for HeadingStackManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Chunk elements semantically, respecting paragraph boundaries
pub fn create_semantic_chunks(elements: &[DocumentElement]) -> Vec<(String, String)> {
    let mut chunks: Vec<(String, String)> = Vec::new();
    let mut heading_stack = HeadingStackManager::new();
    let mut current_chunk_content = String::new();
    let mut current_chunk_context = String::new();

    for element in elements {
        match element {
            DocumentElement::Heading { level, text } => {
                // Flush current chunk before heading change
                if !current_chunk_content.is_empty() {
                    chunks.push((
                        current_chunk_context.clone(),
                        current_chunk_content.trim().to_string(),
                    ));
                    current_chunk_content.clear();
                }

                heading_stack.push_heading(*level, text.clone());
                current_chunk_context = heading_stack.get_context();
            }

            DocumentElement::Paragraph(text) | DocumentElement::CodeBlock(text) => {
                append_text_to_chunk(
                    text,
                    &mut current_chunk_content,
                    &current_chunk_context,
                    &mut chunks,
                );
            }

            DocumentElement::ListItem { text, .. } => {
                // Format list item with bullet
                let formatted = format!("• {}", text);
                append_text_to_chunk(
                    &formatted,
                    &mut current_chunk_content,
                    &current_chunk_context,
                    &mut chunks,
                );
            }
        }
    }

    // Flush final chunk
    if !current_chunk_content.is_empty() {
        chunks.push((current_chunk_context, current_chunk_content.trim().to_string()));
    }

    chunks
}

/// Append text to current chunk, flushing if limits are exceeded
pub fn append_text_to_chunk(
    text: &str,
    current_chunk_content: &mut String,
    current_chunk_context: &str,
    chunks: &mut Vec<(String, String)>,
) {
    let text_len = text.chars().count();
    let current_len = current_chunk_content.chars().count();

    // If adding this text would exceed hard limit
    if current_len + text_len + 2 > CHUNK_HARD_LIMIT {
        // Flush current chunk if not empty
        if !current_chunk_content.is_empty() {
            chunks.push((
                current_chunk_context.to_string(),
                current_chunk_content.trim().to_string(),
            ));
            current_chunk_content.clear();
        }

        // If the text itself is too long, split it
        if text_len > CHUNK_HARD_LIMIT {
            split_oversized_text_into_chunks(text, current_chunk_context, chunks);
            return;
        }
    }

    // If we've hit soft limit, try to start a new chunk
    if current_len >= CHUNK_SOFT_LIMIT && !current_chunk_content.is_empty() {
        chunks.push((
            current_chunk_context.to_string(),
            current_chunk_content.trim().to_string(),
        ));
        current_chunk_content.clear();
    }

    // Add text to current chunk
    if !current_chunk_content.is_empty() {
        current_chunk_content.push_str("\n\n");
    }
    current_chunk_content.push_str(text);
}

/// Split text that exceeds hard limit into sentence-based chunks
pub fn split_oversized_text_into_chunks(
    text: &str,
    context: &str,
    chunks: &mut Vec<(String, String)>,
) {
    let sentences = split_text_into_sentences(text);
    let mut current_chunk = String::new();

    for sentence in sentences {
        let sentence_len = sentence.chars().count();
        let current_len = current_chunk.chars().count();

        if current_len + sentence_len + 1 > CHUNK_HARD_LIMIT && !current_chunk.is_empty() {
            chunks.push((context.to_string(), current_chunk.trim().to_string()));
            current_chunk.clear();
        }

        // If sentence itself is too long, split at word boundaries
        if sentence_len > CHUNK_HARD_LIMIT {
            if !current_chunk.is_empty() {
                chunks.push((context.to_string(), current_chunk.trim().to_string()));
                current_chunk.clear();
            }
            split_text_at_word_boundaries(&sentence, context, chunks);
            continue;
        }

        if !current_chunk.is_empty() {
            current_chunk.push(' ');
        }
        current_chunk.push_str(&sentence);
    }

    if !current_chunk.is_empty() {
        chunks.push((context.to_string(), current_chunk.trim().to_string()));
    }
}

/// Split text into sentences based on punctuation and newlines
pub fn split_text_into_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        current.push(c);

        // End of sentence detection
        if c == '.' || c == '!' || c == '?' {
            // Check if followed by space/newline or end
            if chars.peek().map_or(true, |&next| next.is_whitespace()) {
                sentences.push(current.trim().to_string());
                current.clear();
            }
        }
        // Also split at newlines
        else if c == '\n' {
            if !current.trim().is_empty() {
                sentences.push(current.trim().to_string());
            }
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }

    sentences
}

/// Split text at word boundaries when sentence splitting isn't enough
pub fn split_text_at_word_boundaries(
    text: &str,
    context: &str,
    chunks: &mut Vec<(String, String)>,
) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut current = String::new();

    for word in words {
        if current.chars().count() + word.len() + 1 > CHUNK_HARD_LIMIT && !current.is_empty() {
            chunks.push((context.to_string(), current.trim().to_string()));
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        chunks.push((context.to_string(), current.trim().to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_stack_basic() {
        let mut stack = HeadingStackManager::new();
        stack.push_heading(1, "Chapter 1".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1");

        stack.push_heading(2, "Section A".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section A");
    }

    #[test]
    fn test_heading_stack_pops_same_level() {
        let mut stack = HeadingStackManager::new();
        stack.push_heading(1, "Chapter 1".to_string());
        stack.push_heading(2, "Section A".to_string());
        stack.push_heading(2, "Section B".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section B");
    }

    #[test]
    fn test_heading_stack_pops_lower_levels() {
        let mut stack = HeadingStackManager::new();
        stack.push_heading(1, "Chapter 1".to_string());
        stack.push_heading(2, "Section A".to_string());
        stack.push_heading(3, "Subsection".to_string());
        stack.push_heading(2, "Section B".to_string());
        assert_eq!(stack.get_context(), "H1: Chapter 1 > H2: Section B");
    }

    #[test]
    fn test_split_text_into_sentences() {
        let text = "Hello world. How are you? I am fine!";
        let sentences = split_text_into_sentences(text);
        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[0], "Hello world.");
    }

    #[test]
    fn test_split_text_into_sentences_with_newlines() {
        let text = "Line one\nLine two\nLine three";
        let sentences = split_text_into_sentences(text);
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn test_semantic_chunk_with_headings() {
        let elements = vec![
            DocumentElement::Heading {
                level: 1,
                text: "Title".to_string(),
            },
            DocumentElement::Paragraph("Some content here.".to_string()),
        ];
        let chunks = create_semantic_chunks(&elements);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, "H1: Title");
        assert_eq!(chunks[0].1, "Some content here.");
    }
}
