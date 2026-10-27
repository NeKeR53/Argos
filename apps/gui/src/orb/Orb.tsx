// Fenêtre de l'orbe (D38) : petite, transparente, toujours au premier plan. Elle montre
// l'état de l'Assistant (repos, réflexion) et celui de la voix (écoute, parole), ouvre ou
// cache la console au clic, et porte le panic stop global (deux appuis du raccourci).
//
// C'est aussi elle qui mène la voix côté front (D37, D3), puisqu'elle est toujours là :
// la transcription d'un appui part à l'Assistant, et ses réponses à une demande faite à
// la voix sont lues. Un appui pendant un tour de l'Assistant l'interrompt (barge-in).

import { useCallback, useEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ServerMessage } from "../protocol";
import { assistantOf } from "../state";
import { useDaemon } from "../useDaemon";
import {
  bargeIn,
  IDLE_PLAN,
  onAssistantEvent,
  voiceReason,
  voiceSent,
  voiceStatusLabel,
  type SpeechAction,
  type VoiceEvent,
  type VoiceInfo,
  type VoiceTranscript,
} from "../voice";
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
/** Durée d'affichage d'une légende (transcription envoyée, voix indisponible…). */
const CAPTION_MS = 2500;

export function Orb() {
  const plan = useRef(IDLE_PLAN);
  const voiceInfo = useRef<VoiceInfo | null>(null);
  const assistantId = useRef<string | null>(null);

  // Réponses de l'Assistant à lire : les fragments partent à `say` au fil de l'eau.
  const onMessage = useCallback((message: ServerMessage) => {
    if (message.type !== "event" || !isTauri()) return;
    const event = message.event;
    if (!("agent" in event) || typeof event.agent !== "string" || event.agent !== assistantId.current) return;
    const speakAll = voiceInfo.current?.speak_typed_replies ?? false;
    const result = onAssistantEvent(plan.current, event, speakAll);
    plan.current = result.plan;
    result.actions.forEach(perform);
  }, []);

  const { state, send } = useDaemon(onMessage);
  const connected = state.connection.status === "connected";
  const assistant = assistantOf(state);
  assistantId.current = assistant?.id ?? null;
  const busy = useRef(false);
  busy.current = assistant?.busy ?? false;

  const [voice, setVoice] = useState<VoiceState>(() => previewVoice(window.location.search));
  const [status, setStatus] = useState<VoiceInfo | null>(null);
  const [caption, setCaption] = useState<string | null>(null);
  const [armed, setArmed] = useState(false);
  const [stopped, setStopped] = useState(false);
  const [menu, setMenu] = useState(false);
  const armedAt = useRef<number | null>(null);
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);
  const captionTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const mode = orbMode(connected, assistant?.busy ?? false, voice);

  /** Légende sous l'orbe ; `null` la garde jusqu'au prochain changement. */
  const showCaption = useCallback((text: string | null, ms: number | null = CAPTION_MS) => {
    if (captionTimer.current) clearTimeout(captionTimer.current);
    setCaption(text);
    if (text && ms) captionTimer.current = setTimeout(() => setCaption(null), ms);
  }, []);

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

  const onPushToTalk = useCallback(
    (pressed: boolean) => {
      const info = voiceInfo.current;
      if (pressed && info && info.status !== "ready") {
        showCaption(`Voix indisponible : ${voiceReason(info) ?? voiceStatusLabel(info)}`, 4000);
        return;
      }
      if (!pressed) return;
      // Barge-in : la lecture s'arrête (le moteur coupe `say`), l'Assistant aussi.
      plan.current = bargeIn(plan.current);
      if (busy.current && assistantId.current) send({ command: "interrupt", agent: assistantId.current });
      showCaption("…", null);
    },
    [send, showCaption],
  );

  // La transcription part à l'Assistant, et seulement à lui (D3).
  const onTranscript = useCallback(
    ({ text }: VoiceTranscript) => {
      const message = text.trim();
      const target = assistantId.current;
      if (!message) {
        showCaption("Rien entendu");
        return;
      }
      if (!target || !send({ command: "send", agent: target, text: message })) {
        showCaption("Daemon injoignable : demande perdue", 4000);
        return;
      }
      plan.current = voiceSent(plan.current, message);
      showCaption(message);
    },
    [send, showCaption],
  );

  useEffect(() => {
    if (!isTauri()) return;
    void invoke<VoiceInfo>("voice_status").then((info) => {
      voiceInfo.current = info;
      setStatus(info);
    });
    const unlisten = [
      listen("voice-state", (event) => setVoice(readVoiceState(event.payload))),
      listen("panic-shortcut", () => onPanicShortcut()),
      listen<{ pressed: boolean }>("push-to-talk", (event) => onPushToTalk(event.payload.pressed)),
      listen<VoiceTranscript>("voice-transcript", (event) => onTranscript(event.payload)),
      listen<VoiceEvent>("voice-event", (event) => {
        if (event.payload.type === "partial") showCaption(event.payload.text || "…", null);
      }),
      listen<VoiceInfo>("voice-status", (event) => {
        // L'événement ne porte pas le réglage de lecture : on garde celui déjà lu.
        const info = { ...event.payload, speak_typed_replies: voiceInfo.current?.speak_typed_replies ?? false };
        voiceInfo.current = info;
        setStatus(info);
      }),
    ];
    return () => unlisten.forEach((pending) => void pending.then((stop) => stop()));
  }, [onPanicShortcut, onPushToTalk, onTranscript, showCaption]);

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

  const unavailable = status && status.status !== "ready" ? (voiceReason(status) ?? voiceStatusLabel(status)) : null;
  const style = { "--level": voice.level } as CSSProperties;
  return (
    <div
      className="orb-root"
      data-mode={mode}
      data-armed={armed || undefined}
      data-stopped={stopped || undefined}
      data-mute={unavailable ? "" : undefined}
    >
      <div
        className="orb"
        role="button"
        aria-label={`${label[mode]} : clic pour la console`}
        title={unavailable ? `${label[mode]} · ${unavailable}` : label[mode]}
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
      {!armed && !stopped && caption && <div className="orb-caption">{caption}</div>}
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

function perform(action: SpeechAction) {
  if (action.type === "speak") void invoke("voice_speak_delta", { text: action.text });
  else void invoke("voice_finish_speech");
}

/** Aperçu dans un navigateur : `?voice=listening&level=0.6`. */
function previewVoice(search: string): VoiceState {
  const params = new URLSearchParams(search);
  if (!params.has("voice")) return SILENT;
  return readVoiceState({ state: params.get("voice"), level: Number(params.get("level") ?? 0.5) });
}
