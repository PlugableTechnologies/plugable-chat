import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { installProductionWebviewGuards } from "./lib/webview-guards";

if (import.meta.env.PROD) {
  installProductionWebviewGuards();
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
