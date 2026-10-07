import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { ReasoningEffort } from "./store/chat-store";
import { Sidebar } from "./components/Sidebar";
import { ChatArea } from "./components/ChatArea";
import { SettingsModal } from "./components/settings";
import { useChatStore } from "./store/chat-store";
import { useSettingsStore } from "./store/settings-store";
import { AlertTriangle, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";

// Help message shown when no models are cached. Plugable Chat bundles its own Foundry runtime and
// downloads the default model itself, so this must never tell the user to install Foundry Local.
const NO_MODELS_HELP_MESSAGE = `## Welcome to Plugable Chat! 👋

No AI model is on this computer yet. Plugable Chat downloads one for you; there is nothing to install first.

### What happens next

1. The first model download starts automatically and can take several minutes.
2. Progress appears in the status bar at the bottom of the window.
3. When it finishes, the model name replaces **"No models"** in the header and you can start chatting.

### If the download does not start or fails

- Check that this computer is connected to the internet and that about 6 GB of disk space is free.
- Click the **"No models (click to refresh)"** dropdown in the header to try again.
- Open **Settings → Models** to pick a different model.
- If it still fails, send us the text shown in the status bar so we can help.`;

// Full-pane card for catastrophic failures (the AI engine cannot start). The backend writes the
// whole message in plain language, so this only renders it, makes links clickable and lets the
// user copy it for support.
function StartupFailureCard() {
  const { modelState, retryConnection } = useChatStore();
  const [dismissedAt, setDismissedAt] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);

  if (modelState.state !== 'service_unavailable' || !modelState.errorMessage) return null;
  if (dismissedAt !== null && dismissedAt === modelState.timestamp) return null;
  const text = modelState.errorMessage;

  const renderLine = (line: string, i: number) => {
    const parts = line.split(/(https?:\/\/[^\s]+)/g);
    return (
      <p key={i} className="mb-2 break-words">
        {parts.map((part, j) =>
          /^https?:\/\//.test(part) ? (
            <a key={j} href={part} className="text-blue-700 underline"
               onClick={(e) => { e.preventDefault(); openUrl(part).catch(() => {}); }}>{part}</a>
          ) : part
        )}
      </p>
    );
  };

  const copy = async () => {
    await navigator.clipboard.writeText(text).catch(() => {});
    setCopied(true);
  };

  return (
    <div className="startup-failure-card absolute inset-0 z-50 flex items-center justify-center bg-white/95 p-6">
      <div className="max-w-2xl w-full max-h-full overflow-auto bg-red-50 border border-red-300 text-red-900 rounded-lg shadow-lg p-6">
        <div className="flex items-center gap-3 mb-4">
          <AlertTriangle className="text-red-600" size={24} />
          <h2 className="text-lg font-semibold">Plugable Chat can&apos;t start</h2>
        </div>
        <div className="text-sm">{text.split('\n').filter((l) => l.trim()).map(renderLine)}</div>
        <div className="flex gap-2 mt-4">
          <button onClick={copy} className="px-3 py-1.5 rounded bg-red-600 text-white text-sm">
            {copied ? 'Copied' : 'Copy details for support'}
          </button>
          <button onClick={() => retryConnection()} className="px-3 py-1.5 rounded border border-red-300 text-sm">Try again</button>
          <button onClick={() => setDismissedAt(modelState.timestamp ?? 0)} className="px-3 py-1.5 rounded border border-red-300 text-sm">Dismiss</button>
        </div>
      </div>
    </div>
  );
}

// Shown once when the previous launch never finished starting (e.g. a native crash, which leaves no log).
function PreviousLaunchNotice() {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    invoke<string | null>("get_previous_launch_crash").then(setText).catch(() => {});
  }, []);
  if (!text) return null;
  return (
    <div className="absolute inset-x-4 top-4 z-40 max-h-[70%] overflow-auto bg-amber-50 border border-amber-300 text-amber-900 rounded-lg shadow-md p-4 text-sm">
      <div className="flex justify-between items-start gap-3">
        <h2 className="font-semibold mb-2">Plugable Chat did not finish starting last time</h2>
        <button onClick={() => setText(null)} aria-label="Dismiss"><X size={16} /></button>
      </div>
      {text.split('\n').filter((l) => l.trim()).slice(1).map((l, i) => <p key={i} className="mb-2 break-words">{l}</p>)}
    </div>
  );
}

function ErrorBanner() {
  const { backendError, clearError } = useChatStore();

  if (!backendError) return null;

  return (
    <div className="error-banner absolute top-4 left-4 right-4 z-50 flex items-center justify-between bg-red-50 border border-red-300 text-red-800 px-4 py-3 rounded-lg shadow-md">
      <div className="flex items-center gap-3">
        <AlertTriangle className="text-red-600" size={20} />
        <span className="font-medium text-sm">{backendError}</span>
      </div>
      <button
        onClick={clearError}
        className="p-1 hover:bg-red-100 rounded-lg transition-colors text-red-600"
      >
        <X size={16} />
      </button>
    </div>
  );
}


function App() {
  const { currentModel, cachedModels, modelInfo, reasoningEffort, setReasoningEffort, isConnecting, retryConnection, fetchCachedModels, startSystemChat, chatMessages, hasFetchedCachedModels, loadModel, operationStatus, startupState, handshakeComplete } = useChatStore();
  const launchState = useChatStore((s) => s.modelState.state);
  // Startup counts as finished once the model is ready or a failure card is showing; until then a
  // vanished window is reported at the next launch (see launch_marker.rs).
  useEffect(() => {
    if (launchState === 'ready' || launchState === 'service_unavailable' || launchState === 'error') {
      invoke("mark_launch_ok").catch(() => {});
    }
  }, [launchState]);
  const effortOptions: ReasoningEffort[] = ['low', 'medium', 'high'];
  const hasShownHelpChat = useRef(false);
  console.log("App component rendering...");
  
  // Check if current model supports various features
  const currentModelInfo = modelInfo.find(m => m.id === currentModel);
  const hasToolCalling = currentModelInfo?.tool_calling ?? false;
  const hasReasoning = currentModelInfo?.reasoning ?? currentModel.toLowerCase().includes('reasoning');
  const supportsReasoningEffort = currentModelInfo?.supports_reasoning_effort ?? false;
  
  // Detect when startup is fully complete but no models are available
  // Show help chat to guide user on loading models
  useEffect(() => {
    // Only trigger once, when:
    // 1. Not connecting anymore
    // 2. We've actually completed fetching cached models (hasFetchedCachedModels)
    // 3. currentModel is 'No models' (not 'Downloading...' or other transient states)
    // 4. Haven't shown help chat before
    // 5. No existing messages in chat
    // Note: We show help even during auto-download so users understand what's happening
    const shouldShowHelp = currentModel === 'No models' || currentModel === 'Downloading...';
    if (!isConnecting && hasFetchedCachedModels && shouldShowHelp && !hasShownHelpChat.current && chatMessages.length === 0) {
      hasShownHelpChat.current = true;
      console.log('[App] No cached models found after startup complete. Showing help chat.');
      startSystemChat(NO_MODELS_HELP_MESSAGE, 'Getting Started');
    }
  }, [isConnecting, hasFetchedCachedModels, currentModel, startSystemChat, chatMessages.length]);
  
  // Handle clicking on the "no models" dropdown to refresh
  const handleRefreshModels = async () => {
    console.log('[App] Refreshing cached models...');
    await fetchCachedModels();
    // Also retry connection to update availableModels
    await retryConnection();
  };


  const debugLayout = async () => {
    const log = async (msg: string, data?: any) => {
      console.log(msg, data || '');
      try {
        const message = data ? `${msg} ${JSON.stringify(data, null, 2)}` : msg;
        await invoke('log_to_terminal', { message });
      } catch (e) {
        console.error('Failed to log to terminal:', e);
      }
    };

    await log('\n=== 🔍 LAYOUT DEBUG INFO ===');
    await log('💡 TIP: Open DevTools (Cmd+Option+I on Mac, Ctrl+Shift+I on Windows) to see full output\n');

    await log('=== WINDOW INFO ===', {
      innerWidth: window.innerWidth,
      innerHeight: window.innerHeight,
      outerWidth: window.outerWidth,
      outerHeight: window.outerHeight,
      devicePixelRatio: window.devicePixelRatio,
      screenWidth: window.screen.width,
      screenHeight: window.screen.height,
    });

    await log('\n=== DOCUMENT INFO ===', {
      scrollWidth: document.documentElement.scrollWidth,
      scrollHeight: document.documentElement.scrollHeight,
      clientWidth: document.documentElement.clientWidth,
      clientHeight: document.documentElement.clientHeight,
      offsetWidth: document.documentElement.offsetWidth,
      offsetHeight: document.documentElement.offsetHeight,
    });

    await log('\n=== KEY ELEMENTS ===');
    const selectors = [
      'html',
      'body',
      '#root',
      '.fixed.inset-0', // Main app container
      '.h-14.bg-\\[\\#0d1117\\]', // Header
      '.flex-1.flex.overflow-hidden', // Main content area
      '.flex-\\[1\\]', // Sidebar container
      '.flex-\\[2\\]', // Chat area container
    ];

    for (const selector of selectors) {
      try {
        const el = document.querySelector(selector);
        if (el) {
          const rect = el.getBoundingClientRect();
          const styles = window.getComputedStyle(el);
          await log(`${selector}:`, {
            dimensions: {
              width: rect.width,
              height: rect.height,
              computedWidth: styles.width,
              computedHeight: styles.height,
            },
            position: {
              top: rect.top,
              left: rect.left,
              right: rect.right,
              bottom: rect.bottom,
            },
            computed: {
              display: styles.display,
              position: styles.position,
              margin: styles.margin,
              padding: styles.padding,
              boxSizing: styles.boxSizing,
              overflow: styles.overflow,
              flex: styles.flex,
            }
          });
        } else {
          await log(`${selector}: NOT FOUND`);
        }
      } catch (error) {
        await log(`${selector}: ERROR - ${error}`);
      }
    }

    await log('\n=== ALL VISIBLE ELEMENTS (with dimensions > 0) ===');
    const allElements = document.querySelectorAll('*');
    let count = 0;
    // Collect all visible elements first to avoid too many async calls in loop causing delay
    const visibleElements: { identifier: string; data: any }[] = [];
    allElements.forEach((el) => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        const styles = window.getComputedStyle(el);
        const identifier = `${el.tagName.toLowerCase()}${el.id ? '#' + el.id : ''}${el.className ? '.' + String(el.className).split(' ').filter(c => c).join('.') : ''}`;
        visibleElements.push({
          identifier,
          data: {
            size: { w: Math.round(rect.width), h: Math.round(rect.height) },
            pos: { x: Math.round(rect.left), y: Math.round(rect.top) },
            display: styles.display,
            position: styles.position,
          }
        });
        count++;
      }
    });

    for (const item of visibleElements) {
      await log(`[${visibleElements.indexOf(item)}] ${item.identifier}`, item.data);
    }

    await log(`\nTotal visible elements: ${count}`);
    await log('\n=== END LAYOUT DEBUG ===\n');
  };

  // Log layout info after initial render (disabled - use Ctrl+Shift+L to trigger manually)
  // useEffect(() => {
  //   const timer = setTimeout(() => {
  //     console.log('📊 Initial layout debug (after first render):');
  //     debugLayout();
  //   }, 100);
  //   return () => clearTimeout(timer);
  // }, []);
  
  // Fetch settings and sync MCP servers on app startup
  useEffect(() => {
    console.log('[App] Fetching settings and syncing MCP servers...');
    useSettingsStore.getState().fetchSettings();
  }, []);

  // Lightweight backend heartbeat (1s). Shows a warning bar if backend is unresponsive.
  useEffect(() => {
    const HEARTBEAT_INTERVAL_MS = 1000;
    const HEARTBEAT_TIMEOUT_MS = 1500;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const failureStartRef: { current: number | null } = { current: null };

    const sendHeartbeat = async () => {
      if (cancelled) return;
      const startedAt = Date.now();

      const heartbeatPromise = new Promise<void>((resolve, reject) => {
        const timeoutId = setTimeout(() => reject(new Error('heartbeat-timeout')), HEARTBEAT_TIMEOUT_MS);
        invoke('heartbeat_ping')
          .then(() => {
            clearTimeout(timeoutId);
            resolve();
          })
          .catch((err) => {
            clearTimeout(timeoutId);
            reject(err);
          });
      });

      try {
        await heartbeatPromise;
        if (failureStartRef.current !== null) {
          const recoveredMs = Date.now() - failureStartRef.current;
          console.warn(`[Heartbeat] Backend recovered after ${recoveredMs}ms`);
        }
        failureStartRef.current = null;
        const store = useChatStore.getState();
        if (store.heartbeatWarningStart !== null) {
          store.setHeartbeatWarning(null, null);
        }
      } catch (_err) {
        const store = useChatStore.getState();
        if (failureStartRef.current === null) {
          failureStartRef.current = startedAt;
        }
        const elapsedMs = Date.now() - failureStartRef.current;
        store.setHeartbeatWarning(
          failureStartRef.current,
          `Backend unresponsive for ${Math.round(elapsedMs / 1000)}s`
        );
      } finally {
        if (!cancelled) {
          timer = setTimeout(sendHeartbeat, HEARTBEAT_INTERVAL_MS);
        }
      }
    };

    sendHeartbeat();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  // Global event listeners (persist across component remounts)
  useEffect(() => {
    const store = useChatStore.getState();
    store.setupListeners();
    return () => {
      store.cleanupListeners();
    };
  }, []);

  // Heartbeat/stall detection with auto-resubscribe and backend reconciliation
  useEffect(() => {
    const SOFT_STALL_MS = 8000;
    const HARD_STALL_MS = 15000;
    const RESET_COOLDOWN_MS = 4000;
    let lastResetTs = 0;

    type TurnStatus = {
      active: boolean;
      chat_id: string | null;
      generation_id: number;
      last_token_index: number;
      assistant_response: string;
      finished: boolean;
      had_tool_calls: boolean;
      timestamp_ms: number;
    };

    const reconcileFromBackend = async () => {
      try {
        const status = await invoke<TurnStatus>("get_turn_status");
        const now = Date.now();
        useChatStore.setState((state) => {
          const newMessages = [...state.chatMessages];
          const lastIdx = newMessages.length - 1;
          if (lastIdx >= 0 && newMessages[lastIdx].role === "assistant") {
            newMessages[lastIdx] = {
              ...newMessages[lastIdx],
              content: status.assistant_response || newMessages[lastIdx].content,
            };
          } else if (status.assistant_response) {
            newMessages.push({
              id: `${Date.now()}`,
              role: "assistant",
              content: status.assistant_response,
              timestamp: Date.now(),
            });
          }
          return {
            chatMessages: newMessages,
            assistantStreamingActive: status.active && !status.finished,
            streamingChatId: status.chat_id,
            operationStatus: status.finished ? null : state.operationStatus,
            lastStreamActivityTs: now,
          };
        });
      } catch (e) {
        console.warn("[App] Turn status reconciliation failed", e);
      }
    };

    const timer = setInterval(() => {
      const state = useChatStore.getState();
      const streaming = state.assistantStreamingActive || !!state.toolExecution.currentTool;
      if (!streaming) {
        return;
      }
      const lastActivity = state.lastStreamActivityTs;
      if (!lastActivity) {
        return;
      }
      const now = Date.now();
      const stalledFor = now - lastActivity;
      const isSoftStalled = stalledFor > SOFT_STALL_MS;
      const isHardStalled = stalledFor > HARD_STALL_MS;
      const cooledDown = now - lastResetTs > RESET_COOLDOWN_MS;
      if (isSoftStalled && cooledDown) {
        lastResetTs = now;
        const currentMsg = state.operationStatus?.message || '';
        console.warn(`[App] Long wait detected: stalledFor=${stalledFor}ms, currentStatus="${currentMsg}"`);
        
        // Update the status message to show elapsed time, but don't destroy listeners
        // The backend may still be processing (Foundry can take 20-30s for first token)
        const waitSeconds = Math.floor(stalledFor / 1000);
        let newMessage = currentMsg;
        
        // Update status to show we're still waiting
        if (currentMsg === 'Generating response...' || currentMsg === 'Preparing model request...') {
          newMessage = `Waiting for model (${waitSeconds}s)...`;
        } else if (currentMsg === 'Sending request to model...') {
          newMessage = `Model processing (${waitSeconds}s)...`;
        } else if (currentMsg.includes('Waiting for model') || currentMsg.includes('Model processing')) {
          // Already showing wait message, just update the time
          newMessage = currentMsg.includes('Model processing') 
            ? `Model processing (${waitSeconds}s)...`
            : `Waiting for model (${waitSeconds}s)...`;
        }
        
        if (newMessage !== currentMsg) {
          state.setOperationStatus({
            type: "streaming",
            message: newMessage,
            startTime: state.operationStatus?.startTime || Date.now(),
          });
        }
        
        // Update activity timestamp to reset the stall timer
        state.setLastStreamActivityTs(now);
      }
      if (isHardStalled) {
        console.warn("[App] Hard stall detected. Reconciling with backend and unblocking UI.");
        reconcileFromBackend();
      }
    }, 2000);

    return () => clearInterval(timer);
  }, []);

  // Set up keyboard shortcut: Ctrl+Shift+L
  useEffect(() => {
    const handleKeyPress = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'l') {
        event.preventDefault();
        console.log('⌨️  Ctrl+Shift+L pressed - Running layout debug:');
        debugLayout();
      }
    };

    window.addEventListener('keydown', handleKeyPress);
    return () => window.removeEventListener('keydown', handleKeyPress);
  }, []);

  return (
    <>
    <SettingsModal />
    <div id="app-shell" className="app-shell h-screen w-screen fixed inset-0 bg-white text-gray-800 overflow-hidden font-sans antialiased flex items-start justify-center pt-0 pb-3">
      <div className="app-window-frame w-[calc(100%-24px)] h-[calc(100%-12px)] sm:w-[calc(100%-32px)] sm:h-[calc(100%-16px)] bg-white rounded-b-2xl shadow-lg overflow-hidden flex flex-col">
        {/* Header */}
        <div className="app-header-bar h-14 shrink-0 flex items-center px-4 sm:px-6 bg-white">
          <div className="app-branding-block flex items-center gap-3">
            <img src="/plugable-logo.png" alt="Plugable" className="app-logo h-12 max-w-[240px] w-auto object-contain" />
            <span className="app-product-label font-semibold text-sm text-gray-900">
              Accelerate with the <a href="https://plugable.com/products/tbt5-AI" target="_blank" rel="noopener noreferrer" className="text-blue-600 hover:underline">TBT5-AI</a>
            </span>
          </div>
          <div className="flex-1" />
          <div className="app-model-controls flex items-center gap-2 text-sm text-gray-500">
            <span className="app-model-label">Model:</span>
            {(isConnecting || !handshakeComplete || startupState === 'initializing' || startupState === 'connecting_to_foundry') ? (
              <span className="app-model-status text-gray-500 flex items-center gap-1.5">
                <span className="inline-block w-3 h-3 border-2 border-gray-400 border-t-transparent rounded-full animate-spin"></span>
                {startupState === 'connecting_to_foundry' ? 'Connecting to Foundry...' : 'Starting...'}
              </span>
            ) : currentModel === 'Unavailable' ? (
              <button onClick={retryConnection} className="app-model-unavailable text-red-600 hover:text-red-800 underline underline-offset-2 transition-colors" title="Click to retry connection">
                Unavailable (retry)
              </button>
            ) : currentModel === 'No models' ? (
              <button 
                onClick={handleRefreshModels}
                className="app-model-refresh text-amber-600 hover:text-amber-800 text-[11px] font-semibold underline underline-offset-2 transition-colors"
                title="No models found. Click to check for newly loaded models."
              >
                No models (click to refresh)
              </button>
            ) : currentModel === 'Downloading...' ? (
              <span className="app-model-downloading text-blue-600 flex items-center gap-1.5 text-[11px] font-semibold">
                <span className="inline-block w-3 h-3 border-2 border-blue-400 border-t-transparent rounded-full animate-spin"></span>
                Downloading model...
              </span>
            ) : cachedModels.length > 0 ? (
              <div className="app-model-selector flex items-center gap-1.5">
                <select 
                  value={currentModel} 
                  onChange={(e) => {
                    const newModel = e.target.value;
                    if (newModel !== currentModel) {
                      // Load the new model into VRAM (shows status bar)
                      loadModel(newModel);
                    }
                  }} 
                  className="rounded-md border border-gray-300 bg-white px-2 py-1 text-[11px] font-semibold text-gray-700 focus:border-gray-500 focus:outline-none max-w-[240px]" 
                  title="Select a cached model"
                  disabled={operationStatus?.type === 'loading' || operationStatus?.type === 'downloading'}
                >
                  {cachedModels.map((model) => {
                    return (
                      <option key={model.model_id} value={model.model_id}>
                        {model.alias}{currentModel === model.model_id ? ' ✓' : ''}
                      </option>
                    );
                  })}
                </select>
                {hasToolCalling && (
                  <button 
                    onClick={() => {
                      useSettingsStore.getState().setActiveTab('tools');
                      useSettingsStore.getState().openSettings();
                    }}
                    className="inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-bold bg-green-100 text-green-700 border border-green-200 hover:bg-green-200 transition-colors cursor-pointer" 
                    title="This model supports native tool calling. Click to configure tools."
                  >
                    🔧 Tools
                  </button>
                )}
              </div>
            ) : currentModel === 'Loading...' ? (
              <span className="text-gray-500">Loading...</span>
            ) : (
              <button 
                onClick={handleRefreshModels}
                className="text-amber-600 hover:text-amber-800 text-[11px] font-semibold underline underline-offset-2 transition-colors"
                title="No models cached. Click to check for newly installed models."
              >
                No models (click to refresh)
              </button>
            )}
            {hasReasoning && supportsReasoningEffort && (
              <>
                <span style={{ marginLeft: '24px' }}>Reasoning:</span>
                <select value={reasoningEffort} onChange={(e) => setReasoningEffort(e.target.value as ReasoningEffort)} className="rounded-md border border-gray-300 bg-white px-2 py-1 text-[11px] font-semibold text-gray-700 focus:border-gray-500 focus:outline-none">
                  {effortOptions.map((option) => (
                    <option key={option} value={option}>{option.charAt(0).toUpperCase() + option.slice(1)}</option>
                  ))}
                </select>
              </>
            )}
            {hasReasoning && !supportsReasoningEffort && (
              <span 
                className="inline-flex items-center px-1.5 py-0.5 rounded text-[9px] font-bold bg-yellow-100 text-yellow-700 border border-yellow-200"
                title="This model has built-in reasoning capabilities"
              >
                🧠 Reasoning
              </span>
            )}
          </div>
        </div>
        {/* Main Content */}
        <div className="app-main-region flex-1 flex overflow-hidden min-h-0" style={{ gap: '12px' }}>
          <div className="app-sidebar-container flex-[1] min-w-[260px] overflow-hidden" style={{ backgroundColor: '#e5e7eb', borderRadius: '12px' }}>
            <Sidebar className="h-full sidebar-panel" />
          </div>
          <div className="chat-pane relative flex-[2] min-w-0 flex flex-col overflow-hidden h-full bg-white">
            <ErrorBanner />
            <StartupFailureCard />
            <PreviousLaunchNotice />
            <ChatArea />
          </div>
        </div>
      </div>
    </div>
    </>
  );
}

export default App;
