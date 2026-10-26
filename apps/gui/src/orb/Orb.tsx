// Fenêtre de l'orbe (D38) : petite, transparente, toujours au premier plan. Elle montre
// l'état de l'Assistant (repos, réflexion) et celui de la voix (écoute, parole), ouvre ou
// cache la console au clic, et porte le panic stop global (deux appuis du raccourci).

import { useCallback, useEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { assistantOf } from "../state";
import { useDaemon } from "../useDaemon";
import { orbMode, pressPanic, PANIC_WINDOW_MS, readVoiceState, SILENT, type VoiceState } from "./logic";
import "./orb.css";

const label = {
  idle: "Argos au repos",
  listening: "Argos écoute",
  thinking: "Argos réfléchit",
  speaking: "Argos parle",
  offline: "Daemon injoignable",
} as const;

/** Au-delà de ce déplacement (px), un appui devient un glisser de la fenêtre. */
const DRAG_THRESHOLD = 4;

export function Orb() {
  const { state, send } = useDaemon();
  const connected = state.connection.status === "connected";
  const assistant = assistantOf(state);
  const [voice, setVoice] = useState<VoiceState>(() => previewVoice(window.location.search));
  const [armed, setArmed] = useState(false);
  const [stopped, setStopped] = useState(false);
  const [menu, setMenu] = useState(false);
  const armedAt = useRef<number | null>(null);
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);

  const mode = orbMode(connected, assistant?.busy ?? false, voice);

  // Premier appui : confirmation visible ; second dans les 2 s : arrêt de tout l'arbre.
  const onPanicShortcut = useCallback(() => {
    const result = pressPanic(armedAt.current, Date.now());
    armedAt.current = result.armedAt;
    setArmed(result.armedAt !== null);
    if (result.fire) {
      send({ command: "panic_stop" });
      setStopped(true);
      setTimeout(() => setStopped(false), 1500);
    }
  }, [send]);

  // Fin de la fenêtre de confirmation.
  useEffect(() => {
    if (!armed) return;
    const timer = setTimeout(() => {
      armedAt.current = null;
      setArmed(false);
    }, PANIC_WINDOW_MS);
    return () => clearTimeout(timer);
  }, [armed]);

  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = [
      listen("voice-state", (event) => setVoice(readVoiceState(event.payload))),
      listen("panic-shortcut", () => onPanicShortcut()),
    ];
    return () => unlisten.forEach((pending) => void pending.then((stop) => stop()));
  }, [onPanicShortcut]);

  // Un clic montre ou cache la console ; un appui qui bouge déplace la fenêtre.
  const onPointerDown = (event: PointerEvent) => {
    if (event.button !== 0) return;
    press.current = { x: event.screenX, y: event.screenY, dragging: false };
  };
  const onPointerMove = (event: PointerEvent) => {
    const start = press.current;
    if (!start || start.dragging) return;
    if (Math.hypot(event.screenX - start.x, event.screenY - start.y) > DRAG_THRESHOLD) {
      start.dragging = true;
      if (isTauri()) void getCurrentWindow().startDragging();
    }
  };
  const onPointerUp = () => {
    const start = press.current;
    press.current = null;
    if (start && !start.dragging && isTauri()) void invoke("toggle_console");
  };

  const style = { "--level": voice.level } as CSSProperties;
  return (
    <div className="orb-root" data-mode={mode} data-armed={armed || undefined} data-stopped={stopped || undefined}>
      <div
        className="orb"
        role="button"
        aria-label={`${label[mode]} : clic pour la console`}
        title={label[mode]}
        style={style}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu((open) => !open);
        }}
      >
        <div className="orb-halo" />
        <div className="orb-core" />
      </div>
      {armed && <div className="orb-hint">Encore une fois : tout arrêter</div>}
      {stopped && <div className="orb-hint">Arbre arrêté</div>}
      {menu && (
        <div className="orb-menu">
          <button type="button" onClick={() => isTauri() && void invoke("quit_app")}>
            Quitter Argos
          </button>
          <button type="button" onClick={() => setMenu(false)}>
            Annuler
          </button>
        </div>
      )}
    </div>
  );
}

/** Aperçu dans un navigateur : `?voice=listening&level=0.6`. */
function previewVoice(search: string): VoiceState {
  const params = new URLSearchParams(search);
  if (!params.has("voice")) return SILENT;
  return readVoiceState({ state: params.get("voice"), level: Number(params.get("level") ?? 0.5) });
}
