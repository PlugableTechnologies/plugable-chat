import { Sidebar } from "./components/Sidebar";
import { ChatArea } from "./components/ChatArea";
import { CodeEditor } from "./components/CodeEditor";
import { useChatStore } from "./store/chat-store";

function App() {
  console.log("App component rendering...");
  const { isCodeEditorOpen } = useChatStore();

  return (
    <div className="flex h-screen bg-white text-gray-900 overflow-hidden">
      <Sidebar />
      <div className="flex-1 flex min-w-0">
          <ChatArea />
          {isCodeEditorOpen && (
              <div className="w-1/2 min-w-[400px] border-l border-gray-200 shadow-xl z-10">
                  <CodeEditor />
              </div>
          )}
      </div>
    </div>
  );
}

export default App;
