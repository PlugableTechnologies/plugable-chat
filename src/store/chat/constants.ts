// Model fetch retry configuration
export const MODEL_FETCH_MAX_RETRIES = 3;
export const MODEL_FETCH_INITIAL_DELAY_MS = 1000;

// Relevance search debounce/cancellation configuration
export const RELEVANCE_SEARCH_DEBOUNCE_MS = 400; // Wait 400ms after typing stops
export const RELEVANCE_SEARCH_MIN_LENGTH = 3; // Minimum chars before searching

// Default model to download if no models are available
// Using 'phi-4-mini-instruct' to specifically match the instruct version (not reasoning)
// This matches the alias 'Phi-4-mini-instruct-generic-gpu:5' in the Foundry catalog
export const DEFAULT_MODEL_TO_DOWNLOAD = 'phi-4-mini-instruct';
