This is a revised architectural specification for **Plugable Chat**, shifting the foundation to **Tauri v2**.

By moving to Tauri, we leverage the web ecosystem (React/Tailwind) for the ChatGPT interface, while keeping the heavy compute (LLM inference, Vector Search, File I/O) in a highly concurrent Rust backend.

To satisfy the "maximally parallel" requirement, this design implements an **Async Actor System** using Tokio channels. This ensures that heavy database queries or model inference never block the UI thread or each other.

Our goal with every decision is to avoid complexity and choose the simplest and most concise method to achieve the goal.

-----

## 1\. System Architecture: The Actor Model

Instead of a monolithic state, the backend is split into isolated "Actors" (async tasks). They communicate exclusively via message passing (Multi-producer, Single-consumer channels).

### The Actor Topology

1.  **The Main Orchestrator:** The bridge between the Tauri Frontend and the Backend Actors. It owns the receiving ends of the channels and dispatches events back to the UI.
2.  **Foundry Actor:** Manages the `microsoft foundry local` subprocess (stdin/stdout) and handles HTTP requests to the local API.
3.  **Vector Actor:** Dedicated to `lancedb`. Handles embedding storage and similarity search.
4.  **Context Actor:** Manages the "Window" of active chat, token counting, and pruning history.
5.  **MCP Actor:** Handles dynamic tool discovery and execution.

-----

## 2\. Frontend Design (The View)

**Stack:** React + Vite + Tailwind CSS + Radix UI (for accessibility).
**State:** `TanStack Query` (for async server state) + `Zustand` (for local UI state).

### UI Components & Layout

  * **Layout:** A CSS Grid layout matching the ChatGPT screenshot.
  * **Markdown Engine:** `react-markdown` with `remark-gfm` (tables) and `rehype-katex` (math).
  * **Canvas/Editor:** `Monaco Editor` (VS Code's editor) embedded in the right-hand tab for code viewing/editing.
  * **Streaming:** The UI does not wait for a full response. It listens for `stream_chunk` events and appends tokens to the DOM immediately.

### Parallel UI Behavior

The UI is decoupled from the backend logic.

  * **Input:** When the user types, the UI updates instantly.
  * **Side-Effects:** A "fire-and-forget" event is sent to Rust to trigger auto-complete and vector sorting. The UI does *not* await these. It updates smoothly when the Rust actors push data back.

-----

## 3\. The Parallel Backend Implementation (Rust)

### A. Message Passing Schema

We define an internal protocol for how actors speak to one another.

```rust
// The centralized Event Bus
pub enum SystemMessage {
    // From UI
    UserInput { text: String, chat_id: String },
    RequestCompletion { partial_text: String },
    
    // To Vector Actor
    IndexMessage { content: String, id: String },
    SearchHistory { query: String },

    // To Foundry Actor
    GenerateResponse { model: String, context: Vec<Message> },
    
    // To MCP Actor
    ExecuteTool { tool_name: String, args: serde_json::Value },
}
```

### B. Feature Implementation via Parallel Actors

#### 1\. The "Smart Sort" History (Concurrent Vector Search)

This runs in parallel to the user typing, ensuring zero keystroke latency.

1.  **Trigger:** User types "Rust borrow checker..." in the input.
2.  **Frontend:** Emits `search_history_trigger` (debounced 300ms) to Rust.
3.  **Orchestrator:** Routes this to the **Vector Actor**.
4.  **Vector Actor (Parallel):**
      * Calls Foundry API to get embedding (Network I/O).
      * Queries `lancedb` for cosine similarity (Disk I/O).
      * *Crucial:* This happens on a dedicated thread pool, not blocking the main loop.
5.  **Result:** Vector Actor sends `HistorySorted(Vec<ChatSummary>)` back to Orchestrator.
6.  **Update:** Orchestrator emits a Tauri event `update-sidebar`. The React Sidebar component re-renders with an animation.

#### 2\. Predictive Auto-Complete (Speculative Execution)

1.  **Trigger:** User types.
2.  **Orchestrator:** Spawns a tokio task to hit the completion endpoint of the Foundry Local model.
3.  **Concurrency:** If the user types another character before the previous completion returns, the previous task is `aborted` immediately to save resources.
4.  **Display:** When the task succeeds, a `suggestion_ready` event is emitted. The Frontend displays the suggestion as "ghost text" (opacity 50%).

#### 3\. The Foundry Supervisor

Since Foundry is an external CLI, we need a dedicated supervisor to ensure it stays alive.

  * **Startup:** Spawns `Command::new("foundry").arg("local")...`.
  * **Monitoring:** Reads `stdout` in a separate thread. When it detects `Listening on...`, it parses the port.
  * **Health Check:** Periodically pings the API. If the process dies, the Supervisor restarts it automatically and notifies the frontend to show a "Reconnecting..." toast.

#### 4\. MCP Tool Execution

Tools are executed asynchronously.

  * The LLM stream pauses when a tool call is detected.
  * The **MCP Actor** takes the tool name and arguments.
  * It executes the tool (which might involve file I/O or web requests).
  * While this happens, the UI shows a "Working..." spinner in the chat stream.
  * Once the MCP Actor returns data, the Orchestrator injects it back into the prompt and resumes the LLM stream.

-----

## 4\. Data Structures

### The Shared State (Tauri Managed)

While we use message passing for logic, we use `tauri::State` for immutable references (like database connections).

```rust
struct AppState {
    // Channels to send messages to actors
    tx_vector: mpsc::Sender<VectorMsg>,
    tx_foundry: mpsc::Sender<FoundryMsg>,
    tx_mcp: mpsc::Sender<McpMsg>,
    
    // Shared configuration
    config: RwLock<AppConfig>,
}
```

### LanceDB Schema

[Image of vector database schema diagram]

We use a highly optimized schema for fast retrieval.

  * **Table:** `conversations`
  * **Columns:**
      * `id`: UUID
      * `title`: String
      * `summary`: String (Generated by LLM lazily)
      * `last_active`: Timestamp
      * `vector`: FixedSizeList\<Float32\> (The embedding)

-----

## 5\. Markdown & Rendering Specs

The frontend handles all rendering logic to keep the Rust backend pure.

  * **Input:** Backend sends raw Markdown chunks.
  * **Processing:**
      * User types: `| Name | Age |` ...
      * Frontend `Remark` plugin detects table syntax.
      * Renders a styled HTML `<table>` with Tailwind classes (`border-collapse`, `border-gray-700`, etc.).
  * **Code Blocks:**
      * Detected via triple backticks.
      * Rendered with a "Copy" button and a "Open in Canvas" button.
      * Clicking "Open in Canvas" sends an event to switch the right-hand view to the Monaco Editor with that content.

-----

## 6\. Implementation Strategy

### Phase 1: The Skeleton

1.  Initialize Tauri v2 project.
2.  Set up the `tokio` runtime.
3.  Implement the `FoundryManager` to start/stop the CLI and capture the port.

### Phase 2: The Nervous System

1.  Create the `mpsc` channels.
2.  Build the "Event Loop" in the main Tauri setup hook that listens for Frontend events and dispatches to channels.

### Phase 3: The Brains

1.  Implement `lancedb` integration.
2.  Hook up the "Debounced Input" $\rightarrow$ "Vector Sort" pipeline.

### Phase 4: The UI Polish

1.  Build the React components to match the ChatGPT screenshot.
2.  Implement the Auto-complete ghost text logic.

This specification details the **Rust Tauri v2** backend implementation.

### 1\. Project Dependencies (`Cargo.toml`)

Compatibility between `lancedb` and `arrow` is critical. As of late 2024/2025, we pin these versions to ensure stability.

```toml
[package]
name = "plugable-chat"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = { version = "2.0.0-rc", features = ["protocol-asset"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
# LanceDB & Arrow Stack
lancedb = "0.4" 
arrow = "50.0" # Must match lancedb's internal arrow version
arrow-array = "50.0"
arrow-schema = "50.0"
futures = "0.3"
uuid = { version = "1.0", features = ["v4", "serde"] }
```

-----

### 2\. The Message Protocol

We define a strictly typed protocol for inter-actor communication. This allows the UI to fire events without caring about the implementation details.

```rust
// src/protocol.rs
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub score: f32, // Similarity score
}

pub enum VectorMsg {
    /// Index a new chat or update an existing one
    UpsertChat {
        id: String,
        title: String,
        content: String,
        // The actor will handle embedding generation internally via Foundry
        // or receive a pre-computed vector.
        vector: Option<Vec<f32>>, 
    },
    /// Search for similar chats
    SearchHistory {
        query_vector: Vec<f32>, 
        limit: usize,
        // Channel to send results back to the caller (Orchestrator)
        respond_to: oneshot::Sender<Vec<ChatSummary>>,
    },
}

pub enum FoundryMsg {
    /// Generate an embedding for a string
    GetEmbedding {
        text: String,
        respond_to: oneshot::Sender<Vec<f32>>,
    },
}
```

-----

### 3\. The Vector Actor (`lancedb` Logic)

This actor manages the database connection. Crucially, to be **maximally parallel**, it does not await queries in its main loop. Instead, it spawns a sub-task for each request, allowing it to handle high-throughput typing events without blocking.

```rust
// src/actors/vector_actor.rs
use crate::protocol::{VectorMsg, ChatSummary};
use lancedb::{connect, Table, Connection};
use arrow_array::{RecordBatch, RecordBatchIterator, FixedSizeListArray, StringArray, Float32Array};
use arrow_array::types::Float32Type;
use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct VectorActor {
    rx: mpsc::Receiver<VectorMsg>,
    table: Table,
}

impl VectorActor {
    pub async fn new(rx: mpsc::Receiver<VectorMsg>, db_path: &str) -> Self {
        let db = connect(db_path).execute().await.expect("Failed to connect to LanceDB");
        
        // Ensure table exists
        let table = setup_table(&db).await;

        Self { rx, table }
    }

    pub async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            // Clone table handle for parallel execution (it's cheap, just an Arc internally)
            let table = self.table.clone();

            // Spawn a detached task for every request.
            // This ensures the actor mailbox never clogs, even if a query takes 100ms.
            tokio::spawn(async move {
                match msg {
                    VectorMsg::SearchHistory { query_vector, limit, respond_to } => {
                        let results = perform_search(table, query_vector, limit).await;
                        // Ignore errors if receiver dropped (UI navigated away)
                        let _ = respond_to.send(results);
                    }
                    VectorMsg::UpsertChat { id, title, content, vector } => {
                        if let Some(vec) = vector {
                           let _ = perform_upsert(table, id, title, content, vec).await;
                        }
                    }
                }
            });
        }
    }
}

async fn perform_search(table: Table, vector: Vec<f32>, limit: usize) -> Vec<ChatSummary> {
    // LanceDB Async Query
    let stream = table
        .query()
        .nearest_to(&vector) // Vector search
        .limit(limit)
        .execute()
        .await;

    if stream.is_err() { return vec![]; }
    
    // Process Arrow RecordBatches (Simplified for brevity)
    // In production, you would iterate the stream and map columns to ChatSummary structs
    vec![] 
}

async fn setup_table(db: &Connection) -> Table {
    // Define Arrow Schema for: id (utf8), title (utf8), vector (fixed_size_list<384>)
    // If table doesn't exist, create it. If it does, open it.
    db.open_table("chats").execute().await.unwrap_or_else(|_| {
        // Create logic here...
        panic!("Table creation logic placeholder");
    })
}

async fn perform_upsert(table: Table, id: String, title: String, content: String, vector: Vec<f32>) {
    // Convert Rust Vecs to Arrow Arrays and perform .add()
}
```

-----

### 4\. The Orchestrator (Tauri Main)

This bridges the frontend JS world with the backend Rust actors.

```rust
// src/main.rs
mod protocol;
mod actors;

use protocol::{VectorMsg, FoundryMsg, ChatSummary};
use actors::vector_actor::VectorActor;
use tauri::{State, Manager, Emitter};
use tokio::sync::{mpsc, oneshot};
use std::sync::Arc;

// State managed by Tauri
struct ActorHandles {
    vector_tx: mpsc::Sender<VectorMsg>,
    foundry_tx: mpsc::Sender<FoundryMsg>,
}

#[tauri::command]
async fn search_history(
    query: String, 
    handles: State<'_, ActorHandles>,
    app_handle: tauri::AppHandle
) -> Result<(), String> {
    // 1. Ask Foundry Actor for embedding (simulated here)
    let (emb_tx, emb_rx) = oneshot::channel();
    handles.foundry_tx.send(FoundryMsg::GetEmbedding { 
        text: query, 
        respond_to: emb_tx 
    }).await.map_err(|e| e.to_string())?;

    // 2. Wait for embedding (Non-blocking to the runtime, but this async fn waits)
    let embedding = emb_rx.await.map_err(|_| "Foundry actor died")?;

    // 3. Send to Vector Actor
    let (search_tx, search_rx) = oneshot::channel();
    handles.vector_tx.send(VectorMsg::SearchHistory {
        query_vector: embedding,
        limit: 10,
        respond_to: search_tx
    }).await.map_err(|e| e.to_string())?;

    // 4. Wait for results and Emit to Frontend
    // We emit an event instead of returning to support the "fire-and-forget" UI pattern
    let results = search_rx.await.map_err(|_| "Vector actor died")?;
    
    app_handle.emit("sidebar-update", results).map_err(|e| e.to_string())?;
    
    Ok(())
}

#[tokio::main]
async fn main() {
    // 1. Setup Channels
    let (vector_tx, vector_rx) = mpsc::channel(32);
    let (foundry_tx, foundry_rx) = mpsc::channel(32);

    // 2. Spawn Actors
    // Note: We use tauri::async_runtime or tokio::spawn
    tokio::spawn(async move {
        let actor = VectorActor::new(vector_rx, "./data/lancedb").await;
        actor.run().await;
    });

    // (Mock Foundry Actor spawn for completeness)
    tokio::spawn(async move {
        // Foundry actor loop...
    });

    // 3. Build Tauri
    tauri::Builder::default()
        .manage(ActorHandles { vector_tx, foundry_tx })
        .invoke_handler(tauri::generate_handler![search_history])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

### Key Technical Considerations

1.  **Concurrency Safety:** The `VectorActor` spawns a new `tokio::spawn` for *every* search request. This means if the user types quickly, multiple searches will run in parallel on the thread pool. The `oneshot` channel ensures the result finds its way back to the specific requestor.
2.  **Arrow Integration:** LanceDB uses `Arrow` types. The `perform_upsert` function (omitted for brevity) must strictly convert Rust `Vec<f32>` into `arrow_array::FixedSizeListArray`. This is the most complex part of the boilerplate but ensures zero-copy operations with the DB.
3.  **Error Handling:** Notice the use of `map_err` in the command. If an actor panics (e.g., DB file corruption), the channel closes, and the `.await` on the oneshot will fail. We catch this to prevent the UI from hanging silently.

This addition to the specification details the automated build pipeline. It leverages **GitHub Actions** to produce signed, notarized, and auto-update-ready binaries for Windows and macOS.

### 7\. Automated Build & Distribution Pipeline

We utilize a **Matrix Build Strategy** on GitHub Actions to build for Windows and macOS in parallel. The pipeline is triggered on `push` to a release tag (e.g., `v1.0.0`).

#### A. CI/CD Environment Prerequisites

To support code signing and notarization (required for OS X to not block the app), the following **GitHub Repository Secrets** must be configured:

| Platform | Secret Name | Description |
| :--- | :--- | :--- |
| **Common** | `TAURI_SIGNING_PRIVATE_KEY` | Private key for Tauri's built-in updater (generated via `tauri signer generate`). |
| | `TAURI_SIGNING_PASSWORD` | Password for the updater key. |
| **MacOS** | `APPLE_CERTIFICATE` | Base64-encoded `.p12` certificate (Developer ID Application). |
| | `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` file. |
| | `APPLE_API_ISSUER` | Issuer ID from App Store Connect (for Notarization). |
| | `APPLE_API_KEY` | Key ID from App Store Connect. |
| **Windows** | `WINDOWS_CERTIFICATE` | Base64-encoded `.pfx` code signing certificate. |
| | `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx` file. |

#### B. Configuration (`tauri.conf.json`)

The configuration dynamically reads environment variables during the CI process to inject signing keys.

```json
{
  "bundle": {
    "active": true,
    "targets": "all",
    "macOS": {
      "signingIdentity": "Developer ID Application: Your Name (TEAMID)",
      "entitlements": "./entitlements.plist" // Required for notarization
    },
    "windows": {
      "certificateThumbprint": null, // Injected by CI script
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com"
    }
  }
}
```

#### C. The GitHub Actions Workflow (`release.yml`)

This workflow uses the official `tauri-action` to handle the heavy lifting of environment setup, building, and uploading assets to a GitHub Release.

```yaml
name: Release
on:
  push:
    tags:
      - 'v*'

jobs:
  publish-tauri:
    strategy:
      fail-fast: false
      matrix:
        platform: [macos-latest, windows-latest]
    
    runs-on: ${{ matrix.platform }}
    
    steps:
      - uses: actions/checkout@v4
      
      # 1. Setup Node (Frontend) & Rust (Backend)
      - name: setup node
        uses: actions/setup-node@v4
        with:
          node-version: 20
      - name: install rust stable
        uses: dtolnay/rust-toolchain@stable
      
      # 2. Install Dependencies (Platform Specific)
      - name: install webkit2gtk (ubuntu only - if added later)
        if: matrix.platform == 'ubuntu-latest'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.0-dev libappindicator3-dev librsvg2-dev patchelf

      # 3. Frontend Build
      - name: install frontend dependencies
        run: npm install
      - name: build frontend
        run: npm run build

      # 4. Build & Sign (Tauri Action)
      - uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          # Mac Signing & Notarization
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: "Developer ID Application: Your Name"
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
          # Windows Signing
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PASSWORD: ${{ secrets.TAURI_SIGNING_PASSWORD }}
        with:
          tagName: app-v__VERSION__ 
          releaseName: "Plugable Chat v__VERSION__"
          releaseBody: "See the assets to download this version and install."
          releaseDraft: true
          prerelease: false
          # Arguments to inject Windows Certificate on the fly (Shell script approach)
          args: ${{ matrix.platform == 'windows-latest' && '--config src-tauri/tauri.conf.json' || '' }}
```

#### D. Output Artifacts

Upon successful completion, the GitHub Release page will automatically be populated with:

1.  **Windows:**
      * `Plugable-Chat_x.x.x_x64_en-US.msi` (Standard Installer)
      * `Plugable-Chat_x.x.x_x64-setup.exe` (NSIS Installer - Preferred)
      * `Plugable-Chat_x.x.x_x64.sig` (Update signature)
2.  **macOS:**
      * `Plugable-Chat_x.x.x_x64.dmg` (Intel Mac)
      * `Plugable-Chat_x.x.x_aarch64.dmg` (Apple Silicon M1/M2/M3)
      * `Plugable-Chat.app.tar.gz` (Update bundle)

#### E. Update Logic

The app checks for updates on startup using the built-in Tauri Updater. It queries a JSON endpoint (hosted on GitHub Pages or S3) that points to these releases. The `tauri-action` can be configured to automatically generate and upload this `latest.json` file.

This addition to the specification details the "Local Build" workflow, allowing developers to generate run-ready binaries for their own machines without purchasing certificates or configuring complex secrets.

### 8\. Local / Unsigned Build (Dev Mode)

For internal testing or personal use, you can build the application without code signing identities. The operating system will flag these binaries as "Unknown Publisher," but they are fully functional.

#### A. Windows (Unsigned .exe/.msi)

On Windows, if you do not provide certificate paths in `tauri.conf.json`, Tauri automatically skips the signing step.

1.  **Configuration:** Ensure your `tauri.conf.json` **does not** contain the `bundle > windows > certificateThumbprint` fields (or leave them null).
2.  **Build Command:**
    ```bash
    npm run tauri build
    # OR
    cargo tauri build
    ```
3.  **Output:**
      * **Installer:** `src-tauri/target/release/bundle/msi/Plugable Chat_0.1.0_x64_en-US.msi`
      * **Standalone Binary:** `src-tauri/target/release/plugable-chat.exe`
4.  **Running it:**
      * When you run the installer, Windows SmartScreen will appear.
      * Click **"More Info"** -\> **"Run Anyway"**.

#### B. macOS (Ad-Hoc Signing)

macOS requires *some* signature to run an app, even locally. Tauri handles this automatically using "Ad-Hoc Signing" (a signature that is valid only on the machine that built it) when no valid Apple Developer Identity is found.

1.  **Configuration:** In `tauri.conf.json`, explicitly set the identity to null or a hyphen to force ad-hoc signing if you have a cert but want to ignore it:
    ```json
    "bundle": {
      "macOS": {
        "signingIdentity": null
      }
    }
    ```
2.  **Build Command:**
    ```bash
    npm run tauri build
    ```
3.  **Output:**
      * **Disk Image:** `src-tauri/target/release/bundle/dmg/Plugable Chat_0.1.0_x64.dmg`
      * **App Bundle:** `src-tauri/target/release/bundle/macos/Plugable Chat.app`
4.  **Running it:**
      * **On your machine:** It will open immediately.
      * **On a friend's Mac:** They will see "App cannot be opened because the developer cannot be verified."
      * **The Bypass:** They must **Right-Click** the app -\> Select **Open** -\> Click **Open** in the warning dialog.

#### C. Cross-Compilation Note

You generally **cannot** build a macOS binary from Windows or vice-versa locally due to linker dependencies. You must build on the native OS or use the CI/CD pipeline defined in Section 7.