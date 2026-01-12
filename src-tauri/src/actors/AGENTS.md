# Actors & Model Profiles

## Model Profiles (`model_profiles.rs`)
Each model has a `ModelProfile` that defines:
- **`ModelFamily`**: `GptOss`, `Phi`, `Gemma`, `Granite`, `Generic`
- **`ToolFormat`**: How the model outputs tool calls (`OpenAI`, `Hermes`, `Granite`, `Gemini`, `TextBased`).
- **`ReasoningFormat`**: `None`, `ThinkTags`, `ThinkingTags`, `ChannelBased`.

## Execution Parameters (`foundry/request_builder.rs`)
`build_foundry_chat_request_body()` sets model-family-specific parameters:
- **GptOss**: `max_tokens=16384`, `temperature=0.7`, native tools.
- **Phi**: Supports `reasoning_effort` when reasoning model.
- **Gemma**: `top_k=40`.
- **Granite**: `repetition_penalty=1.05`.

## Vector Store Actor (`vector_actor.rs`)
- **Schema**: Defined in `get_expected_schema()`.
- **Initialization**: `setup_table()` handles schema checks and destructive migration.
- **Guardrail**: If schema mismatch is detected, the table is dropped and recreated.

## Database Toolbox Actor (`database_toolbox_actor.rs`)
- **Capabilities**: Schema discovery (`schema_search`, enumeration) and SQL execution (`sql_select`).
- **Caching**: Table schemas and embeddings are cached on disk to enable fast semantic search.
- **Safety**: Queries are executed with read-only permissions where possible.
