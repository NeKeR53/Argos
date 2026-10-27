import React from "react";
import ReactDOM from "react-dom/client";
import { Orb } from "./Orb";
import { OrbDemo } from "./OrbDemo";

// `?demo=…` : les états de l'orbe sur données simulées, pour les voir dans un navigateur.
const params = new URLSearchParams(window.location.search);
const demo = params.get("demo");
const frozen = params.get("t");

ReactDOM.createRoot(document.getElementById("orb-root") as HTMLElement).render(
  <React.StrictMode>
    {demo ? <OrbDemo demo={demo} frozenAt={frozen === null ? undefined : Number(frozen)} /> : <Orb />}
  </React.StrictMode>,
);
