import React from "react";
import ReactDOM from "react-dom/client";
import { Orb } from "./Orb";

ReactDOM.createRoot(document.getElementById("orb-root") as HTMLElement).render(
  <React.StrictMode>
    <Orb />
  </React.StrictMode>,
);
