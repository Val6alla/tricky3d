// First: moves settings saved under the app's old name before any module reads them.
import "./settingsRename";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { useStore } from "./store";
import { useUi } from "./ui";
import { viewport } from "./viewport/api";
import "./theme.css";

// Development builds only: a handle for driving the app from devtools or a CDP test script.
if (import.meta.env.DEV) Object.assign(window, { __tricky3d: { useStore, useUi, viewport } });

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
