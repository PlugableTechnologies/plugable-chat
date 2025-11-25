# Reasoning Model Configuration

## Overview
The Phi-4-mini-reasoning model is a **reasoning model** that uses extended thinking to solve complex problems. Unlike standard chat models, reasoning models:

1. **Output their thinking process** in `<think></think>` XML tags
2. **Should provide a final answer** after the thinking process
3. Support a `reasoning_effort` parameter to control depth of reasoning

## The Problem
Initially, the model was outputting very long reasoning processes but **never providing a final answer**. This is because reasoning models need explicit instruction to output both:
- Their internal reasoning (in `<think>` tags)
- A clear final answer (outside the tags)

## Solution Implemented

### 1. Added `reasoning_effort` Parameter
**File**: `src-tauri/src/actors/foundry_actor.rs`

```rust
let body = json!({
    "model": model, 
    "messages": messages,
    "stream": true,
    "max_tokens": 16384,
    "reasoning_effort": "medium"  // Options: "low", "medium", "high"
});
```

**What it does**:
- `"low"`: Faster responses, less thorough reasoning (good for simple questions)
- `"medium"`: Balanced approach (default, good for most use cases)
- `"high"`: Maximum reasoning depth (best for complex problems, slower)

### 2. Added System Message for Reasoning Models
**File**: `src-tauri/src/actors/foundry_actor.rs`

The code now automatically prepends a system message that instructs the model to:
1. Use `<think></think>` tags for reasoning
2. **Always provide a final answer** outside the tags

```rust
// For reasoning models, ensure we have a system message that instructs
// the model to provide a final answer after thinking
let mut messages = history.clone();
let has_system_msg = messages.iter().any(|m| m.role == "system");

if !has_system_msg {
    // Prepend system message for reasoning models
    messages.insert(0, crate::protocol::ChatMessage {
        role: "system".to_string(),
        content: "You are a helpful AI assistant. When answering questions, you may use <think></think> tags to show your reasoning process. After your thinking, always provide a clear, concise final answer outside the think tags.".to_string(),
    });
}
```

## How It Works

### User Experience
1. **User asks a question**: "What happened in 1971?"
2. **Model thinks** (shown in collapsible "Thought Process" section):
   ```
   <think>
   Let me recall major events from 1971...
   - Apollo missions were ongoing
   - Vietnam War was still active
   - Nixon took the US off the gold standard
   ...extensive reasoning...
   </think>
   ```
3. **Model provides final answer** (shown as main response):
   ```
   Several significant events occurred in 1971:
   - The United States abandoned the gold standard (Nixon Shock)
   - Apollo 14 and 15 missions to the moon
   - The Pentagon Papers were published
   ...
   ```

### Technical Flow
```
User Input
    ↓
Frontend adds to message history
    ↓
Backend prepends system message (if needed)
    ↓
API request with:
  - messages (including system instruction)
  - max_tokens: 16384
  - reasoning_effort: "medium"
  - stream: true
    ↓
Model generates:
  1. <think>reasoning process</think>
  2. Final answer text
    ↓
Frontend parses and displays both parts
```

## Customization Options

### Adjusting Reasoning Depth
You can modify the `reasoning_effort` parameter based on your needs:

```rust
// For quick, simple questions
"reasoning_effort": "low"

// For balanced performance (current default)
"reasoning_effort": "medium"

// For complex problems requiring deep analysis
"reasoning_effort": "high"
```

### Customizing the System Message
If you want to change how the model behaves, modify the system message in `foundry_actor.rs`:

```rust
content: "You are a helpful AI assistant. When answering questions, you may use <think></think> tags to show your reasoning process. After your thinking, always provide a clear, concise final answer outside the think tags.".to_string(),
```

Examples of alternative instructions:
- **Concise mode**: "Provide brief answers with minimal reasoning."
- **Detailed mode**: "Show extensive reasoning, then provide a comprehensive answer."
- **Educational mode**: "Explain your reasoning step-by-step, then summarize the answer."

## UI Display
The frontend (`ChatArea.tsx`) automatically:
1. **Parses `<think>` tags** and displays them in a collapsible "Thought Process" section
2. **Displays final answer** as the main response text
3. **Supports streaming** so users see the response build in real-time

## Benefits
✅ **Transparent reasoning**: Users can see how the model arrived at its answer  
✅ **Better accuracy**: Extended thinking leads to more thoughtful responses  
✅ **Configurable depth**: Adjust reasoning effort based on question complexity  
✅ **Complete answers**: System message ensures the model always provides a final answer  

## Testing
Try asking complex questions that benefit from reasoning:
- "What happened in 1971?"
- "Explain quantum entanglement"
- "How would you solve the traveling salesman problem?"
- "What are the implications of the halting problem?"

You should see:
1. A collapsible "Thought Process" section with the model's reasoning
2. A clear final answer below the thinking section
