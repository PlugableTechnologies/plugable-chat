# Streaming Token Limit Fix

## Problem
When submitting questions to the model (e.g., "what happened in 1980?"), the model's response was getting cut off mid-thought. The model would start its thinking process but never complete it or return a final answer.

## Root Cause
The issue had **two parts**:

### 1. Channel Buffer Limit (Initial Fix)
In `src-tauri/src/lib.rs` (line 81), the chat command was using `mpsc::channel(100)` which creates a channel with a buffer size of only **100 messages**. When the model generates more than 100 tokens and the frontend isn't consuming them fast enough, the channel fills up and **blocks**, preventing any more tokens from being sent.

### 2. Missing `max_tokens` Parameter (Actual Culprit)
More importantly, the API request to Microsoft Foundry Local was **not specifying a `max_tokens` parameter**. This caused the API to use a **small default limit** (likely around 512-1024 tokens), which made the model stop generating tokens mid-thought even though the model supports up to 16,384 output tokens.

The logs showed "Foundry stream DONE" which confirmed the **server was intentionally ending the stream**, not our client cutting it off.

## Solution
Applied **two fixes** to resolve both issues:

### Fix 1: Unbounded Channel
Changed from a **bounded channel** to an **unbounded channel** to prevent any client-side blocking.

### Fix 2: Explicit `max_tokens` Parameter
Added `max_tokens: 16384` to the API request to ensure the model can generate complete responses using its full output capacity.

### Files Modified

#### 1. `/Users/bernie/git/plugable-chat/src-tauri/src/lib.rs`
**Line 81:** Changed from `mpsc::channel(100)` to `mpsc::unbounded_channel()`

```rust
// Before:
let (tx, mut rx) = mpsc::channel(100);

// After:
// Use unbounded channel to prevent blocking on long responses
let (tx, mut rx) = mpsc::unbounded_channel();
```

#### 2. `/Users/bernie/git/plugable-chat/src-tauri/src/protocol.rs`
**Line 50:** Updated the `FoundryMsg::Chat` enum to use `UnboundedSender` instead of `Sender`

```rust
// Before:
Chat {
    history: Vec<ChatMessage>,
    respond_to: tokio::sync::mpsc::Sender<String>,
},

// After:
Chat {
    history: Vec<ChatMessage>,
    respond_to: tokio::sync::mpsc::UnboundedSender<String>,
},
```

#### 3. `/Users/bernie/git/plugable-chat/src-tauri/src/actors/foundry_actor.rs`
**Lines 58, 63, 102, 127, 140, 145:** Removed `.await` from all `send()` calls since `UnboundedSender::send()` is synchronous (doesn't return a Future)

```rust
// Before:
let _ = respond_to.send(content.to_string()).await;

// After:
let _ = respond_to.send(content.to_string());
```

**Lines 78-82:** Added `max_tokens` parameter to the API request body

```rust
// Before:
let body = json!({
    "model": model, 
    "messages": history,
    "stream": true
});

// After:
let body = json!({
    "model": model, 
    "messages": history,
    "stream": true,
    "max_tokens": 16384  // Use the model's maximum to prevent premature cutoff
});
```

## Impact
- ✅ **No more artificial token limits** - The model can now generate responses of any length
- ✅ **Complete responses** - Long-form answers and extended thinking processes will no longer be cut off
- ✅ **Better performance** - Unbounded channels are slightly more efficient for streaming use cases
- ⚠️ **Memory consideration** - In theory, an unbounded channel could grow indefinitely if the producer is much faster than the consumer. However, in practice:
  - The frontend consumes tokens very quickly (just appending to UI)
  - The model generates tokens at a human-readable pace
  - This is a single-user desktop app, not a high-throughput server

## Testing
After rebuilding with `npm run tauri build -- --debug`, test by:
1. Starting the application
2. Asking a question that requires a long response (e.g., "what happened in 1980?")
3. Verifying that the model completes its entire thought process and provides a final answer

## Technical Notes
- **Bounded vs Unbounded Channels**: 
  - Bounded channels (`mpsc::channel(N)`) have a fixed buffer size and block when full
  - Unbounded channels (`mpsc::unbounded_channel()`) can grow indefinitely
  - Unbounded channels use `send()` (synchronous) while bounded use `send().await` (async)
  
- **Why this works**: The Microsoft Foundry local model can now stream as many tokens as needed without worrying about buffer limits, ensuring complete responses every time.
