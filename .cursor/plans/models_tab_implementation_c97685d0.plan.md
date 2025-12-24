---
name: Models Tab Implementation
overview: "Add a new \"Models\" settings tab as the first tab that exposes Foundry Local model management capabilities: listing catalog models with device filtering, showing cached/downloaded models, downloading models with real-time progress, loading/unloading models from memory, and removing models from cache."
todos:
  - id: backend-protocol
    content: Add CatalogModel struct and new FoundryMsg variants (UnloadModel, GetCatalogModels, GetServiceStatus)
    status: completed
  - id: backend-actor
    content: Implement handlers in FoundryActor for catalog list, unload, and cache removal
    status: completed
  - id: backend-commands
    content: "Add Tauri commands: get_catalog_models, unload_model, remove_cached_model, get_foundry_service_status"
    status: completed
  - id: frontend-types
    content: Add TypeScript interfaces for FoundryCatalogModel, FoundryServiceStatus
    status: completed
  - id: frontend-store
    content: Update settings-store activeTab type to include 'models' as first/default tab
    status: completed
  - id: frontend-models-tab
    content: Create ModelsTab component with device filter and card-based model grid with status badges and actions
    status: completed
  - id: integration-testing
    content: Wire up download progress, load/unload, and test full workflow
    status: completed
---

# Models Tab for Foundry Local Model Management

## Overview

Create a new "Models" tab in Settings that provides a unified UI for managing Foundry Local models. This tab will be the first tab (leftmost) and will expose the functionality of `foundry model` and `foundry cache` CLI commands (except `run`).

## Data Model

Based on the `/foundry/list` REST API, each catalog model has:

- `alias`: e.g., "phi-4-mini"
- `name`: e.g., "Phi-4-mini-instruct-generic-gpu:5"
- `runtime.deviceType`: "CPU" | "GPU" | "NPU"
- `fileSizeMb`: Size in MB
- `license`: e.g., "MIT"
- `supportsToolCalling`: boolean
- `task`: e.g., "chat-completion"

The key insight from the CLI output is that models are grouped by **alias** with variants for each device type (CPU/GPU). For example:

```javascript
phi-4-mini     GPU    3.72 GB    Phi-4-mini-instruct-generic-gpu:5
               CPU    4.80 GB    Phi-4-mini-instruct-generic-cpu:5
```



## UI Design - Card-Based Layout

Each model will be displayed as a **card** rather than a simple text row. This provides ample space for model metadata, status indicators, and action buttons in a visually appealing, professional format.

```mermaid
flowchart TB
    subgraph ModelsTab [Models Tab Layout]
        subgraph Header [Header Bar]
            DeviceFilter[Device Dropdown: Auto/CPU/GPU/NPU]
            ServiceStatus[Service: Running on :55530]
            RefreshBtn[Refresh Button]
        end
        
        subgraph ScrollArea [Scrollable Content Area]
            PromoBanner[Enable more models with the Plugable TBT5-AI]
            
            subgraph CardGrid [Model Card Grid]
                subgraph Card1 [Model Card: phi-4-mini]
                    C1Title[phi-4-mini]
                    C1Badge1[GPU]
                    C1Badge2[3.72 GB]
                    C1Badge3[MIT]
                    C1Badge4[chat + tools]
                    C1Status[Status: Downloaded + Loaded]
                    C1Actions[Load / Unload / Remove]
                end
                
                subgraph Card2 [Model Card: qwen2.5-7b]
                    C2Title[qwen2.5-7b]
                    C2Info[GPU | 5.2 GB | Apache]
                    C2Status[Status: Not Downloaded]
                    C2Actions[Download Button]
                end
            end
        end
        
        subgraph Footer [Footer Info]
            CacheLocation[Cache: ~/.foundry/cache/models]
        end
    end
```



### Model Card Design

Each card displays:

```javascript
+----------------------------------------------------------+
|  [GPU]  phi-4-mini                              [MIT]    |
|  --------------------------------------------------------|
|  Phi-4-mini-instruct-generic-gpu:5                       |
|  Size: 3.72 GB  |  Tasks: chat, tools                    |
|  --------------------------------------------------------|
|  Status: [Downloaded] [Loaded]                           |
|  --------------------------------------------------------|
|  [ Load ]  [ Unload ]  [ Remove ]     or    [ Download ] |
+----------------------------------------------------------+
```

**Card Elements:**

- **Header**: Device badge (GPU/CPU/NPU), model alias (bold), license badge
- **Subtitle**: Full model name/ID
- **Details Row**: File size, supported tasks (chat, tools)
- **Status Row**: Badges showing cached/downloaded state, loaded state
- **Action Row**: Context-aware buttons based on model state

**Card States:**

1. **Not Downloaded**: Shows "Download" button with estimated size
2. **Downloaded (Cached)**: Shows "Load" and "Remove" buttons
3. **Loaded**: Shows "Unload" button, highlighted border/glow
4. **Downloading**: Shows progress bar with percentage and current file

### Layout Components

1. **Header Bar**:

- Device type dropdown (Auto/CPU/GPU/NPU) - defaults to GPU when available
- Service status indicator with port (green dot = running, red = stopped)
- Refresh button to reload catalog and status

2. **Promotional Banner** (inside scrollable area, above cards):

- Single line message: "Enable more models with the [Plugable TBT5-AI](https://plugable.com/products/tbt5-ai)"
- Subtle styling (not intrusive), link opens in external browser
- Promotes the hardware product this software bundles with

3. **Card Grid** (scrollable, filtered by device type):

- Responsive grid: 1 column on narrow, 2-3 on wider screens
- Cards sorted: Loaded first, then Downloaded, then Available
- Visual distinction for loaded models (accent border/glow)

4. **Download Progress Overlay**:

- When downloading, the card shows inline progress bar
- Current file name and percentage
- Cancel button if supported

5. **Footer Section**:

- Cache directory location (clickable to reveal in system)

## Backend Commands (New Tauri Commands)

These commands need to be added to [src-tauri/src/lib.rs](src-tauri/src/lib.rs):| Command | REST Endpoint / CLI | Description ||---------|---------------------|-------------|| `get_catalog_models` | `GET /foundry/list` | List all models in catalog with full metadata || `unload_model` | `GET /openai/unload/{name}` | Unload model from memory || `remove_cached_model` | `foundry cache remove --yes` | Delete from disk cache || `get_cache_location` | `GET /openai/status` → `modelDirPath` | Get cache directory || `get_foundry_service_status` | `GET /openai/status` | Get service status including port |Existing commands already available:

- `get_cached_models` (via `GET /openai/models`)
- `download_model` (via `POST /openai/download` with progress events)
- `load_model` (via `GET /openai/load/{name}`)
- `get_loaded_models` (via `GET /openai/loadedmodels`)

## Frontend Changes

### 1. Settings Store Updates ([src/store/settings-store.ts](src/store/settings-store.ts))

Add `'models'` to the `activeTab` type union and set as default:

```typescript
activeTab: 'models' | 'system-prompt' | 'interfaces' | 'builtins' | 'tools' | 'databases' | 'schemas';
```



### 2. New Types ([src/lib/api.ts](src/lib/api.ts) or new file)

```typescript
interface FoundryCatalogModel {
    name: string;
    displayName: string;
    alias: string;
    uri: string;
    version: string;
    fileSizeMb: number;
    license: string;
    task: string;
    supportsToolCalling: boolean;
    runtime: {
        deviceType: 'CPU' | 'GPU' | 'NPU';
        executionProvider: string;
    };
}

interface FoundryServiceStatus {
    endpoints: string[];
    modelDirPath: string;
    isAutoRegistrationResolved: boolean;
}
```



### 3. ModelsTab Component ([src/components/Settings.tsx](src/components/Settings.tsx))

New `ModelsTab` component with:

- `useChatStore` for download progress state (already has `operationStatus`)
- Device filter state (default to GPU when available)
- Catalog models fetched from backend, filtered by selected device type
- Status computed by comparing catalog vs cached vs loaded models

**Sub-components:**

- `ModelCard` - Individual model card with status badges and action buttons
- `ModelCardGrid` - Responsive grid container for cards
- `DeviceFilterDropdown` - Device type selector

### 4. ModelCard Component

Card-based display for each model:

```typescript
interface ModelCardProps {
    model: FoundryCatalogModel;
    isCached: boolean;
    isLoaded: boolean;
    isDownloading: boolean;
    downloadProgress?: { file: string; progress: number };
    onDownload: () => void;
    onLoad: () => void;
    onUnload: () => void;
    onRemove: () => void;
}
```

**Visual States:**

- Default card: subtle border, white background
- Loaded model: accent border (blue/green glow), slightly elevated
- Downloading: progress bar overlay, muted action buttons
- Not cached: "Download" as primary action, size prominently displayed

### 5. Download Progress

Already implemented via `model-download-progress` Tauri event and `operationStatus` in chat-store. The card UI should show:

- Inline progress bar within the card
- Current file name and percentage
- Cancel button if supported

## Protocol Updates ([src-tauri/src/protocol.rs](src-tauri/src/protocol.rs))

Add new message variants to `FoundryMsg`:

```rust
/// Get catalog models (GET /foundry/list)
GetCatalogModels {
    respond_to: oneshot::Sender<Vec<CatalogModel>>,
},
/// Unload a model from memory (GET /openai/unload/{name})
UnloadModel {
    model_name: String,
    respond_to: oneshot::Sender<Result<(), String>>,
},
/// Get service status including cache location
GetServiceStatus {
    respond_to: oneshot::Sender<Result<ServiceStatus, String>>,
},
```



## Implementation Steps

1. **Backend Protocol**: Add `CatalogModel`, `ServiceStatus` structs and new `FoundryMsg` variants
2. **Backend Actor**: Implement handlers in `FoundryActor` for new messages
3. **Backend Commands**: Add Tauri commands for new operations
4. **Frontend Types**: Add TypeScript interfaces for catalog models and service status
5. **Frontend Store**: Add `'models'` tab type, make it default
6. **Frontend UI**: Create `ModelsTab` component with:

- Header bar with device filter dropdown and service status
- `ModelCard` component with badges, status indicators, and action buttons
- Responsive card grid layout (1-3 columns based on viewport)
- Download progress integration within cards