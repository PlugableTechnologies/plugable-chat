import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";
import type { ReasoningEffort } from "./store/chat-store";
import { Sidebar } from "./components/Sidebar";
import { ChatArea } from "./components/ChatArea";
import { useChatStore } from "./store/chat-store";
import { AlertTriangle, X } from "lucide-react";
import plugableLogo from "../docs/plugable_logo_color_3x2_1800x1200_on_transparent.png";

function ErrorBanner() {
  const { backendError, clearError } = useChatStore();

  if (!backendError) return null;

  return (
    <div className="absolute top-4 left-4 right-4 z-50 flex items-center justify-between bg-red-50 border border-red-300 text-red-800 px-4 py-3 rounded-lg shadow-md">
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
  const { currentModel, reasoningEffort, setReasoningEffort } = useChatStore();
  const effortOptions: ReasoningEffort[] = ['low', 'medium', 'high'];
  console.log("App component rendering...");


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

  // Log layout info after initial render
  useEffect(() => {
    // Use setTimeout to ensure DOM is fully rendered and styled
    const timer = setTimeout(() => {
      console.log('📊 Initial layout debug (after first render):');
      debugLayout();
    }, 100);

    return () => clearTimeout(timer);
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
    <div className="h-screen w-screen fixed inset-0 bg-white text-gray-800 overflow-hidden font-sans antialiased">
      <div className="h-full w-full">
        <div className="h-full w-full px-4 sm:px-6 py-4 sm:py-6">
          <div className="mx-auto h-full w-full max-w-[1400px] flex flex-col gap-4">
            {/* Top Header Bar */}
            <div className="h-14 bg-white border border-transparent rounded-2xl border-b border-gray-200 flex items-center relative px-4 sm:px-6">
              <div className="flex items-center gap-3">
                <img src={plugableLogo} alt="Plugable" className="h-6 max-w-[120px] w-auto object-contain" />
                <span className="font-semibold text-sm text-gray-900">Chat for Microsoft Foundry</span>
              </div>
              <div className="flex-1" />
              <div className="flex items-center gap-6 text-sm text-gray-500">
              <span>Reasoning: </span>
                  <select
                    value={reasoningEffort}
                    onChange={(e) => setReasoningEffort(e.target.value as ReasoningEffort)}
                    className="rounded-md border border-gray-300 bg-white px-2 py-1 text-[11px] font-semibold text-gray-700 focus:border-gray-500 focus:outline-none"
                  >
                    {effortOptions.map((option) => (
                      <option key={option} value={option}>
                        {option.charAt(0).toUpperCase() + option.slice(1)}
                      </option>
                    ))}
                  </select>
                <span> Local Model: </span>
                <span className="text-gray-700">{currentModel}</span>
              </div>
            </div>

            {/* Main Content Area */}
            <div className="flex-1 flex overflow-hidden min-h-0 w-full max-w-none min-w-0">
              <div className="flex-[1] min-w-[260px]">
                <Sidebar className="h-full" />
              </div>
              <div className="flex-[2] min-w-0 flex flex-col relative overflow-hidden h-full">
                <ErrorBanner />
                <ChatArea />
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default App;
