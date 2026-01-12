//! Foundry Local model gateway actor and related functionality.
//!
//! This module provides:
//! - `ModelGatewayActor`: Main actor for managing Foundry Local service and model operations
//! - Request building utilities for Foundry API calls
//! - Streaming response handlers
//! - Service lifecycle management

mod model_gateway_actor;
mod request_builder;
mod service_manager;
mod stream_handler;

pub use model_gateway_actor::ModelGatewayActor;

// Re-export commonly used items from submodules for internal use
pub use request_builder::{build_foundry_chat_request_body, convert_chat_messages_to_foundry_format};
pub use service_manager::{find_foundry_binary, parse_foundry_service_status_output, ServiceStatus, FoundryModel, FoundryModelsResponse, DEFAULT_FALLBACK_MODEL};
pub use stream_handler::{StreamingToolCalls, extract_text_from_stream_chunk};
