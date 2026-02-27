# Plugable Chat: Innovative Approach

**How a local-first desktop app makes small AI models punch above their weight**

---

Small language models are fast, private, and free. They run entirely on your hardware, never phone home, and don't cost a cent per token. But they hallucinate more, lose track of context faster, and struggle with multi-step reasoning. The conventional wisdom is that you need a large frontier model for serious analytical work — querying databases, retrieving documents, writing and executing code.

Plugable Chat challenges that assumption. It is an orchestration layer — a harness — that breaks complex tasks into steps small enough for a local model to handle reliably, then verifies each step with transparent, auditable evidence. The result is a desktop application where a 4-billion-parameter model running on your laptop can do the kind of data analysis work that usually demands GPT-4 or Claude.

Think of it as **Cursor for SQL**: the same philosophy of intelligent code assistance, applied to databases, documents, and data pipelines, running entirely offline.

---

## The Small Model Problem

A frontier model with 400 billion parameters can hold an entire database schema in context, reason about which tables to join, write correct SQL on the first try, and explain the results in plain English. A small model with 4 billion parameters can do none of these things reliably — unless you change the rules of the game.

Small models fail in predictable ways:

- **Context overload.** Feed a small model 200 columns across 30 tables and it will reference columns that don't exist, confuse table names, or simply ignore most of the schema.
- **Hallucinated data.** Ask a small model about revenue trends and it may fabricate plausible-sounding numbers rather than admitting it needs to run a query.
- **Fragile tool use.** Small models produce malformed function calls, forget required parameters, or call the wrong tool entirely.
- **Compounding errors.** When a SQL query fails, a small model often retries with the same mistake, or introduces a new one, spiraling into a loop.

Every feature in Plugable Chat exists to solve one or more of these failure modes. The philosophy is simple: **don't make the model figure it out — tell it exactly what to do, and verify that it did.**

---

## Progressive Schema Discovery

The conventional approach to database-connected AI is to dump the entire schema into the prompt. This works well enough for cloud models with 128K-token context windows. For a small local model with an effective context of 4K–8K tokens, it is a death sentence.

Plugable Chat takes a different approach: **discover schemas progressively, on demand.**

When you connect a database, Plugable Chat indexes every table and column into a local vector store using embedding models. But it doesn't inject any of that into the prompt yet. Instead, when you ask a question — say, "What were the top-selling products last quarter?" — the system embeds your question and performs a semantic search against the schema index. Only the tables and columns relevant to your query are surfaced to the model.

This is the same principle behind Anthropic's [Tool Search Tool](https://www.anthropic.com/engineering/advanced-tool-use), which reduces token consumption by 85% while maintaining access to the full tool library. Plugable Chat applies the same idea to database schemas: **full access, minimal context.**

But Plugable Chat goes further. Not all columns are created equal. A table with 200 columns — common in enterprise data warehouses — still overwhelms a small model even when the table itself is relevant. So the system applies **hybrid column selection**: all non-numeric columns (text, dates, booleans) are always included because they define the shape and meaning of data, while numeric columns are ranked by semantic relevance and capped at a configurable maximum. The model sees the columns it needs, not the columns it doesn't.

The final refinement: for each selected column, the system includes the **top three most common values with their frequency percentages**. When a small model sees that the `city` column contains "Chicago (23.5%), New York (18.2%), Los Angeles (12.1%)", it knows exactly what valid values look like — and is far less likely to hallucinate a city name that doesn't exist in the data.

All of this happens automatically, before the model generates a single token. The user asks a question; the system discovers the relevant schema; the model sees a focused, information-dense prompt instead of an overwhelming wall of metadata.

---

## Progressive Tool Discovery

The same problem that afflicts schemas afflicts tools. A rich MCP (Model Context Protocol) ecosystem might offer 50+ tools across multiple servers — GitHub, filesystem, databases, APIs. Loading all 50 tool definitions into the prompt consumes tens of thousands of tokens and confuses small models about which tool to use.

Plugable Chat implements **two-tier tool visibility**:

1. **Active tools** are fully documented in the system prompt and immediately callable. These are the tools most likely to be needed — SQL queries, Python execution, document retrieval.
2. **Deferred tools** are hidden from the prompt but discoverable via semantic search. Their descriptions are pre-embedded, and when the model (or the system) determines one is needed, it is **materialized** — promoted from deferred to active.

Before the first turn of a conversation, the system performs automatic tool discovery: it embeds the user's message, searches the deferred tool index, and pre-loads any tools likely to be relevant. Mid-conversation, the model can call `tool_search` explicitly to discover additional capabilities.

This mirrors Anthropic's concept of [deferred tool loading](https://www.anthropic.com/engineering/advanced-tool-use), where tools marked with `defer_loading: true` are excluded from the initial prompt and discovered on demand. The key difference is that Plugable Chat does this locally, with a small model that benefits even more from the context savings.

The prompt tells the model exactly what it can't yet see:

> *There are 45 tools available across 3 server(s). These tools are currently deferred to save context space. Use `tool_search(relevant_to="...")` to discover and enable them when needed.*

This gives the model awareness of capability without the cost of specification.

---

## The State Machine: Prompt Refinement, Not Context Accumulation

Most AI chat applications work by appending every message, tool result, and system instruction to a growing conversation history. By the tenth turn, the context window is a graveyard of stale instructions, superseded results, and irrelevant metadata. Frontier models can wade through this noise. Small models drown in it.

Plugable Chat takes a fundamentally different approach: **it regenerates the system prompt from scratch on every turn.**

At the heart of the application is a three-tier state machine:

### Settings Layer (app lifetime)
Evaluates the user's configuration — which databases are connected, which tools are enabled, which model is loaded — and determines the **operational mode**: conversational, SQL, code execution, tool orchestration, or a hybrid of several.

### Turn Layer (per request)
Transitions between twelve distinct states based on what happened in the previous turn. Did a SQL query execute? The state becomes `SqlResultCommentary`, and the system prompt is rewritten to say: "The query results have already been displayed. Your role now is to provide helpful commentary." Did a query fail? The state becomes `SqlErrorRecovery`, and the prompt is rewritten with the failed query, the error message, and the full schema of the relevant tables.

### Mid-Turn Layer (within the agentic loop)
Tracks tool execution during a single model turn — counting calls, deciding whether to continue or stop, detecting stuck loops.

The critical insight is that **the system prompt is not a static document**. It is a computed artifact of the current state. When the model transitions from schema discovery to SQL execution to result commentary, the prompt transforms with it. Sections are added, removed, and rewritten. The model never sees stale instructions from a previous phase of the conversation.

This is especially powerful for small models because it means the prompt is always **focused**. In SQL mode, there are no Python instructions cluttering the context. During error recovery, the failing query and relevant schema are injected directly, rather than hoping the model will scroll back through history to find them. Every token in the prompt earns its place.

---

## Programmatic Tool Calling and Code Execution

Anthropic's [Programmatic Tool Calling](https://www.anthropic.com/engineering/advanced-tool-use) lets Claude write Python code that orchestrates multiple tool calls, keeping intermediate results out of the model's context window. Plugable Chat implements a similar philosophy for local models — but adapted to the constraints of smaller, less capable systems.

When a user attaches a CSV or Excel file, Plugable Chat doesn't simply pass the file path to the model and hope for the best. It pre-processes the file: inferring column types, stripping currency symbols, converting percentage strings to floats, and injecting the cleaned data as Python variables (`headers1`, `rows1`) directly into the execution sandbox. The model writes code against well-structured data, not raw messy input.

Python execution serves as a critical **escape hatch** from the limitations of natural language reasoning. When a task requires iteration, conditional logic, or mathematical precision — "calculate compound interest over 30 years," "find outliers more than two standard deviations from the mean" — the model writes code, and the sandbox executes it. The model doesn't have to do the math. It just has to write correct Python, which is a task small models handle far more reliably than mental arithmetic.

The system validates Python syntax using an AST parser *before* execution, catching malformed code early and preventing wasted cycles. If the code produces no output, the system generates specific diagnostic guidance rather than silently failing.

MCP tools discovered through progressive tool discovery become callable as Python functions within the sandbox, enabling the kind of multi-step orchestration that Anthropic describes — fetching data from one tool, transforming it, passing it to another — without each intermediate result consuming context.

---

## Error Recovery: The Agentic Loop

The agentic loop is where Plugable Chat's "Cursor for SQL" philosophy is most visible. When something goes wrong — and with small models, things go wrong regularly — the system doesn't just pass the error back and hope the model figures it out. It **re-injects the context the model needs to fix the mistake.**

### SQL Error Recovery

When a SQL query fails, the system:

1. Extracts the failed query and the database error message.
2. Retrieves the compact schema of every relevant table — column names, types, primary keys, foreign keys.
3. Builds a structured recovery prompt that forces chain-of-thought reasoning:

> *BEFORE you retry, you MUST answer these questions:*
> *1. What column caused the error?*
> *2. Which table did you query?*
> *3. Does that column exist in that table?*
> *4. Could the column exist in a DIFFERENT table?*

4. If the problematic column exists in a different table than the one queried, the system explicitly flags it: "⚠️ IMPORTANT: I can see 'product' exists in one of the tables listed above. You may be querying the WRONG table!"

This transforms error recovery from a guessing game into guided correction. The model doesn't have to search its context for the schema — the schema is right there. It doesn't have to figure out what went wrong — the system tells it exactly where to look.

### Repeated Error Detection

If the same error occurs twice — a sign that the model is stuck in a loop — the system takes a different approach entirely: it disables tool calling and prompts the model to answer the user's question directly from whatever data it already has. This is a graceful degradation strategy: rather than spinning forever, the system acknowledges the limitation and gives the user the best answer it can.

### Repetition Detection

Small models are particularly prone to output loops — generating the same text over and over. Plugable Chat uses a period-analysis algorithm to detect repetitive patterns in the model's output stream. When a loop is detected, generation is cancelled immediately, and the user is notified. This prevents the wasted compute and user frustration of watching a model repeat itself for 30 seconds.

### Early Stopping

During streaming, the system detects tool call boundaries (like `</tool_call>`) and stops generation early. This prevents a common small-model failure: successfully producing a correct tool call, then continuing to generate hallucinated text after it — "results" the model invents before the tool has actually executed.

---

## Radical Transparency

AI-generated analysis is only useful if you can verify it. This is true for any model, but it is **especially critical for small models**, where hallucination rates are higher and reasoning is less reliable.

Plugable Chat is built around the principle that every claim should be one click away from its evidence:

### SQL Results: Formatted and Raw

When the model queries a database, the results are displayed in two layers:

- **The model's interpretation** appears in the main chat: a natural-language summary of what the data shows, trends it identifies, comparisons it draws.
- **The raw data table** is always one click away in an expandable accordion: every row, every column, properly formatted with numeric alignment and row counts. The user can verify every claim against the actual query results.

### Tool Calls: Transparent Execution

Every tool invocation is visible:

- **During execution**, a live processing indicator shows the tool name, the server it's running on, the parsed arguments, and an elapsed-time counter.
- **After execution**, an expandable block shows the full details: which tool was called, what arguments were passed, whether it succeeded or failed, how long it took, and the complete raw response.

On error, the arguments are auto-expanded — because when something goes wrong, the first thing you need to see is exactly what was attempted.

### Why Transparency Matters More for Small Models

When a frontier model says "revenue increased 12% year-over-year," you can be reasonably confident it read the data correctly. When a small model says the same thing, you need to check. Plugable Chat's transparency features make checking effortless: the raw data is right there, the query is right there, and the model's interpretation is presented as a summary *of evidence you can inspect*, not as an unverifiable assertion.

This transforms the user's relationship with the AI from **trust** to **verify**. You don't have to trust the model. You just have to check its work — and checking is fast because the evidence is always at hand.

---

## Model-Aware Orchestration

Not all small models are created equal. Phi-4 uses a different tool-calling format than Llama, which uses a different format than Gemma, which uses a different format than Mistral. Some models expect Hermes-style XML tags. Others expect JSON blocks. Others expect Python function call syntax.

Plugable Chat maintains **model-specific profiles** that configure:

- **Tool call format**: Hermes (`<tool_call>...</tool_call>`), Mistral (`[TOOL_CALLS]`), Pythonic (`function_name(arg="value")`), and more — with format-specific examples injected into the system prompt so the model sees exactly how to call tools in *its* native syntax.
- **Execution parameters**: Temperature, top-k, repetition penalty, and max tokens tuned for each model family.
- **Parsing chains**: Twelve distinct tool-call parsers with extensive fallback logic. When a small model produces a slightly malformed tool call — missing a closing brace, using the wrong quote style — the system tries multiple parsing strategies before giving up.

This multi-format tolerance is essential. Small models are unpredictable in their output formatting. A system that demands pixel-perfect JSON will fail constantly. A system that flexibly interprets the model's *intent* — even when the syntax is imperfect — succeeds far more often.

---

## Document Retrieval (RAG)

Plugable Chat includes a full retrieval-augmented generation pipeline that runs entirely locally:

- **Structure-aware extraction** from PDFs, using bookmarks and font sizes to detect headings and preserve document hierarchy.
- **Intelligent chunking** that respects heading boundaries, so a chunk about "Q3 Revenue" doesn't bleed into a chunk about "Q4 Projections."
- **Local vector indexing** in LanceDB, with relevancy thresholds that control how many chunks are injected into the prompt.
- **Dominance detection**: When document retrieval is highly relevant to the user's question, SQL context is suppressed — preventing the model from being confused by competing context about databases and documents simultaneously.

The same GPU/CPU memory management applies: bulk document indexing uses the GPU embedding model for speed, but real-time search during chat uses the CPU embedding model to avoid evicting the LLM from GPU memory. The user never notices this — their chat model stays warm and responsive.

---

## Use Cases

### The Financial Analyst

Sarah connects her company's PostgreSQL database — 150 tables, thousands of columns. She asks: "What were our top 10 customers by revenue last quarter, and how does that compare to the same quarter last year?"

Plugable Chat discovers the relevant `orders`, `customers`, and `line_items` tables. The model sees only those three tables, with only the columns that matter. It writes a SQL query with joins and date filtering. The results appear as a formatted table. Sarah clicks to expand the raw data and sees every row. She clicks the tool call accordion and sees the exact SQL that was executed. The numbers check out. Total time: 8 seconds, on her laptop, with no data leaving her machine.

### The Researcher

David drags a 200-page PDF of a climate report into the chat and asks: "What does this report say about methane emissions from agriculture?"

The document is chunked with heading-aware boundaries and indexed locally. The system retrieves the three most relevant chunks — from sections titled "Agricultural Sources" and "Methane Budget" — and injects them into the prompt. The model synthesizes an answer grounded in the actual text. David can see which chunks were retrieved and read them himself.

### The Data Scientist

Maria attaches a CSV of sensor readings — 50,000 rows, 12 columns — and asks: "Are there any anomalous readings in the temperature sensor over the last month?"

The CSV is pre-processed with type inference. The model writes Python code to calculate rolling averages and flag readings beyond two standard deviations. The code executes in the sandbox and produces a summary of 14 anomalous readings with timestamps. Maria can see the code that was executed and the raw output.

### The Operations Engineer

James has MCP servers connected for GitHub, Jira, and his monitoring stack — 58 tools total. He asks: "What are the open critical bugs assigned to my team?"

Only the `tool_search` function is loaded initially. The system discovers `jira.searchIssues` and materializes it. The model calls it with the right parameters. James sees the results, and he sees the exact API call that was made. The other 57 tools never consumed a token.

---

## The Philosophy

The techniques in Plugable Chat — progressive discovery, prompt refinement, transparent execution, error recovery — are not just optimizations. They represent a different philosophy of AI interaction.

Cloud AI services give you a powerful model and leave the rest to you. Plugable Chat gives you a modest model and wraps it in an intelligent harness that compensates for its limitations. The harness breaks complex tasks into simple steps. It discovers context on demand instead of dumping it upfront. It regenerates instructions to match the current state instead of accumulating stale history. It catches errors and guides correction instead of hoping the model self-corrects. And it shows its work at every step, so you can verify instead of trust.

This approach is inspired by the same principles that Anthropic has formalized in their [advanced tool use platform](https://www.anthropic.com/engineering/advanced-tool-use): Tool Search for on-demand discovery, Programmatic Tool Calling for efficient orchestration, and Tool Use Examples for teaching correct invocation. Plugable Chat brings these ideas to the local-model world, where they are not just beneficial but essential.

Small models can do significant work. They just need careful guidance. Plugable Chat provides that guidance — automatically, transparently, and entirely on your machine.

---

## Privacy and Independence

Everything described above runs locally:

- **Your data stays on your machine.** Database queries execute against your local or network databases. Documents are indexed on your hard drive. Chat history is stored in a local SQLite database.
- **No internet required.** Analyze data on a plane, in a secure facility, or anywhere without connectivity.
- **No subscriptions.** Run open-weight models — Phi-4, Llama, Gemma, Mistral — as much as you want, as often as you want, without per-token fees.

Privacy isn't a feature of Plugable Chat. It's the foundation everything else is built on.

---

## Try Plugable Chat

Plugable Chat runs on hardware you may already own. Any MacBook with Apple Silicon (M1 or later) or a Windows PC with a discrete NVIDIA GPU can run small models like Phi-4-mini and Gemma locally. The experience described in this document — progressive discovery, transparent execution, agentic error recovery — works on consumer hardware today.

For higher throughput and larger models, the [Plugable TBT5-AI](https://plugable.com) accelerator drives significantly faster token generation, enabling models like Llama 3.1 8B and Phi-4 14B to run at interactive speeds. More parameters mean stronger reasoning and fewer hallucinations — and the same orchestration harness that makes small models succeed makes larger models even more effective.

Plugable Chat is open source under the **Apache 2.0 license**. Clone the repository, install the dependencies, and start exploring:

**[github.com/PlugableTechnologies/plugable-chat](https://github.com/PlugableTechnologies/plugable-chat)**
