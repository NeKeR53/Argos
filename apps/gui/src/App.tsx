import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type CoreInfo = { version: string; protocol: number };

function App() {
  const [info, setInfo] = useState<CoreInfo | null>(null);

  useEffect(() => {
    invoke<CoreInfo>("core_info").then(setInfo);
  }, []);

  return (
    <main className="container">
      <h1>Argos</h1>
      <p>{info ? `GUI v${info.version} · protocole v${info.protocol}` : "…"}</p>
    </main>
  );
}

export default App;
