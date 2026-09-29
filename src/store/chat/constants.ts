// Model fetch retry configuration
export const MODEL_FETCH_MAX_RETRIES = 3;
export const MODEL_FETCH_INITIAL_DELAY_MS = 1000;

// Relevance search debounce/cancellation configuration
export const RELEVANCE_SEARCH_DEBOUNCE_MS = 400; // Wait 400ms after typing stops
export const RELEVANCE_SEARCH_MIN_LENGTH = 3; // Minimum chars before searching

// Default model to download if no models are available. A short name: the backend resolves it
// to the variant best suited to this machine (GPU and CUDA first).
// If it cannot run here, the backend blocklists it and the fallback below is used.
export const DEFAULT_MODEL_TO_DOWNLOAD = 'qwen3.5-4b';

// Last-resort model when the default is missing or incompatible (matches DEFAULT_FALLBACK_MODEL
// in the Rust gateway). Pinned to the instruct version, not the reasoning one.
export const FALLBACK_MODEL = 'phi-4-mini-instruct';
