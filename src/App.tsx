import { Sidebar } from "./components/Sidebar";
import { ChatArea } from "./components/ChatArea";
import { CodeEditor } from "./components/CodeEditor";
import { useChatStore } from "./store/chat-store";

function App() {
  console.log("App component rendering...");

  return (
    <div className="fixed inset-0 flex bg-white text-gray-900 overflow-hidden">
      <Sidebar />
      <div className="flex-1 flex min-w-0">
          <ChatArea />
      </div>
    </div>
  );
}

export default App;
