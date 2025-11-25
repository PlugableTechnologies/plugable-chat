import { useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatArea } from "./components/ChatArea";
import { useChatStore } from "./store/chat-store";
import { AlertTriangle, X } from "lucide-react";

function ErrorBanner() {
  const { backendError, clearError } = useChatStore();

  if (!backendError) return null;

  return (
    <div className="absolute top-20 left-4 right-4 z-50 flex items-center justify-between bg-red-500/10 border border-red-500/50 text-red-200 px-4 py-3 rounded-xl backdrop-blur-md shadow-lg animate-in fade-in slide-in-from-top-2">
      <div className="flex items-center gap-3">
        <AlertTriangle className="text-red-400" size={20} />
        <span className="font-medium text-sm">{backendError}</span>
      </div>
      <button
        onClick={clearError}
        className="p-1 hover:bg-red-500/20 rounded-lg transition-colors text-red-400 hover:text-red-200"
      >
        <X size={16} />
      </button>
    </div>
  );
}

function App() {
  const { currentModel } = useChatStore();
  console.log("App component rendering...");

  // Debug layout utility function
  const debugLayout = () => {
    console.log('\n=== 🔍 LAYOUT DEBUG INFO ===');
    console.log('💡 TIP: Open DevTools (Cmd+Option+I on Mac, Ctrl+Shift+I on Windows) to see full output\n');

    console.log('=== WINDOW INFO ===');
    console.log({
      innerWidth: window.innerWidth,
      innerHeight: window.innerHeight,
      outerWidth: window.outerWidth,
      outerHeight: window.outerHeight,
      devicePixelRatio: window.devicePixelRatio,
      screenWidth: window.screen.width,
      screenHeight: window.screen.height,
    });

    console.log('\n=== DOCUMENT INFO ===');
    console.log({
      scrollWidth: document.documentElement.scrollWidth,
      scrollHeight: document.documentElement.scrollHeight,
      clientWidth: document.documentElement.clientWidth,
      clientHeight: document.documentElement.clientHeight,
      offsetWidth: document.documentElement.offsetWidth,
      offsetHeight: document.documentElement.offsetHeight,
    });

    console.log('\n=== KEY ELEMENTS ===');
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

    selectors.forEach(selector => {
      try {
        const el = document.querySelector(selector);
        if (el) {
          const rect = el.getBoundingClientRect();
          const styles = window.getComputedStyle(el);
          console.log(`${selector}:`, {
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
          console.log(`${selector}: NOT FOUND`);
        }
      } catch (error) {
        console.log(`${selector}: ERROR -`, error);
      }
    });

    console.log('\n=== ALL VISIBLE ELEMENTS (with dimensions > 0) ===');
    const allElements = document.querySelectorAll('*');
    let count = 0;
    allElements.forEach((el) => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        const styles = window.getComputedStyle(el);
        const identifier = `${el.tagName.toLowerCase()}${el.id ? '#' + el.id : ''}${el.className ? '.' + String(el.className).split(' ').filter(c => c).join('.') : ''}`;
        console.log(`[${count}] ${identifier}`, {
          size: { w: Math.round(rect.width), h: Math.round(rect.height) },
          pos: { x: Math.round(rect.left), y: Math.round(rect.top) },
          display: styles.display,
          position: styles.position,
        });
        count++;
      }
    });
    console.log(`\nTotal visible elements: ${count}`);
    console.log('\n=== END LAYOUT DEBUG ===\n');
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
    <div className="fixed inset-0 flex flex-col bg-[#0f1419] text-slate-200 overflow-hidden font-sans antialiased selection:bg-cyan-500/30">
      {/* Top Header Bar */}
      <div className="h-14 bg-[#0d1117] border-b border-transparent flex items-center justify-between px-4 sm:px-6 shrink-0 rounded-2xl shadow-[0_0_35px_rgba(2,6,23,0.7)]">
        <div className="flex items-center gap-3">
          <img src="/plugable-logo.png" alt="Plugable" className="h-6 max-w-[120px] w-auto object-contain brightness-110 opacity-90" />
          <span className="font-semibold text-sm">Plugable Chat</span>
        </div>
        <div className="flex items-center gap-4 text-sm">
          <span className="text-slate-400">Local</span>
          <span className="text-slate-200">Model: {currentModel}</span>
        </div>
      </div>

      {/* Main Content Area */}
      <div className="flex-1 flex overflow-hidden min-h-0 w-full max-w-none min-w-0 mt-3">
        <div className="flex-[1] min-w-[260px]">
          <Sidebar className="h-full rounded-2xl" />
        </div>
        <div className="flex-[2] min-w-0 flex flex-col relative overflow-hidden h-full">
          <ErrorBanner />
          <ChatArea />
        </div>
      </div>
    </div>
  );
}

export default App;
