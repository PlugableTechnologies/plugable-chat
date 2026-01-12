//! RAG (Retrieval-Augmented Generation) actor and document processing.
//!
//! This module provides:
//! - `RagRetrievalActor`: Main actor for document indexing and semantic search
//! - PDF structure extraction with bookmark and font-size detection
//! - Document chunking with heading hierarchy preservation
//! - Multi-format file processing (PDF, DOCX, CSV, JSON, TXT, MD)
//! - LanceDB-based sidecar caching for embeddings

mod cache_manager;
mod document_chunker;
mod file_processor;
mod pdf_extractor;
mod retrieval_actor;

pub use retrieval_actor::RagRetrievalActor;

// Re-export commonly used items from submodules for internal use
pub use cache_manager::{
    DirectoryConnection, FileCacheEntry, IndexedChunk, RAG_CHUNKS_TABLE, RAG_FILE_CACHE_TABLE,
};
pub use document_chunker::{DocumentElement, HeadingStackManager, CHUNK_HARD_LIMIT, CHUNK_SOFT_LIMIT};
pub use file_processor::extract_plaintext_from_docx_xml;
pub use pdf_extractor::{extract_pdf_heading_structure, PdfHeading};
