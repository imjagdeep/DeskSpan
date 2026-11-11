import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "@fontsource-variable/plus-jakarta-sans";
import "./styles/theme.css";
import "./styles/shell.css";
import "./styles/span.css";
import "./styles/account.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
