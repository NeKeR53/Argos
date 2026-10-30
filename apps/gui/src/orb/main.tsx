import React from "react";
import ReactDOM from "react-dom/client";
import { Orb } from "./Orb";
import { OctopusDemo } from "../octopus/OctopusDemo";
import { OrbDemo } from "./OrbDemo";

// `?demo=…` : les états de l'orbe sur données simulées, pour les voir dans un navigateur ;
// `?demo=octopus` : la pieuvre dans toutes ses attitudes.
const params = new URLSearchParams(window.location.search);
const demo = params.get("demo");
const frozen = params.get("t");

ReactDOM.createRoot(document.getElementById("orb-root") as HTMLElement).render(
  <React.StrictMode>
    {demo === "octopus" ? (
      <OctopusDemo />
    ) : demo ? (
      <OrbDemo demo={demo} frozenAt={frozen === null ? undefined : Number(frozen)} />
    ) : (
      <Orb />
    )}
  </React.StrictMode>,
);
