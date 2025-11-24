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

  return (
    <div className="fixed inset-0 flex flex-col bg-[#0f1419] text-slate-200 overflow-hidden font-sans antialiased selection:bg-cyan-500/30 px-3 sm:px-6">
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
        <div className="flex-[2] min-w-0 flex flex-col relative overflow-hidden">
          <ErrorBanner />
          <ChatArea />
        </div>
      </div>
    </div>
  );
}

export default App;
