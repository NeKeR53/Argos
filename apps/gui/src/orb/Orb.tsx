// Fenêtre de l'orbe (D38) : une pastille discrète, toujours au premier plan, que l'on
// déplace. Elle montre l'état de l'Assistant (repos, réflexion, escalade) et celui de la
// voix (ondes à l'écoute, bouche qui articule à la parole), ouvre ou cache la console au
// clic, et porte le panic stop global (deux appuis du raccourci).
//
// C'est aussi elle qui mène la voix côté front (D37, D3), puisqu'elle est toujours là :
// la transcription d'un appui part à l'Assistant, et ses réponses à une demande faite à
// la voix sont lues. Un appui pendant un tour de l'Assistant l'interrompt (barge-in).

import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
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
import type { Utterance } from "./Face";
import { orbMode, pressPanic, PANIC_WINDOW_MS, readVoiceState, SILENT, type VoiceState } from "./logic";
import { Pill, PILL_MARGIN, pillSize } from "./Pill";
import "./orb.css";

/** Au-delà de ce déplacement (px), un appui devient un glisser de la fenêtre. */
const DRAG_THRESHOLD = 4;
/** Fenêtre pour reconnaître un double-clic (qui remet l'orbe à sa place). */
const DOUBLE_CLICK_MS = 260;
/** Durée d'affichage d'une légende (transcription envoyée, voix indisponible…). */
const CAPTION_MS = 2500;
/** Durée de la transition de taille de la pastille (doit suivre orb.css). */
const RESIZE_MS = 240;

/** Le menu (clic droit) remplace la pastille le temps d'un choix. */
const MENU_SIZE = { width: 330, height: 36 };

const isMac = navigator.platform.toLowerCase().startsWith("mac");

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

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

  const { state, dispatch, send } = useDaemon(onMessage);
  const connected = state.connection.status === "connected";
  const assistant = assistantOf(state);
  assistantId.current = assistant?.id ?? null;
  const busy = useRef(false);
  busy.current = assistant?.busy ?? false;

  const [voice, setVoice] = useState<VoiceState>(SILENT);
  const [status, setStatus] = useState<VoiceInfo | null>(null);
  const [utterance, setUtterance] = useState<Utterance | null>(null);
  const [caption, setCaption] = useState<string | null>(null);
  const [armed, setArmed] = useState(false);
  const [stopped, setStopped] = useState(false);
  const [menu, setMenu] = useState(false);
  const armedAt = useRef<number | null>(null);
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const captionTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const mode = orbMode(connected, assistant?.busy ?? false, voice);

  /** Légende ; `null` en durée la garde jusqu'au prochain changement. */
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
      setUtterance(null);
      if (busy.current && assistantId.current) send({ command: "interrupt", agent: assistantId.current });
      showCaption(null);
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
      showCaption(null);
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
        const payload = event.payload;
        if (payload.type === "partial") showCaption(payload.text || null, null);
        // Chaque phrase lancée : la bouche la suit à partir de maintenant.
        if (payload.type === "utterance") setUtterance({ text: payload.text, startedAt: performance.now() });
        if (payload.type === "speech_finished") setUtterance(null);
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

  // La fenêtre suit la pastille : elle grandit tout de suite, et ne rétrécit qu'une fois
  // la transition finie, pour ne pas couper l'animation.
  const escalation = state.escalations.length > 0;
  useEffect(() => {
    if (!isTauri() || !isMac) return;
    void invoke("orb_dock_state", {
      view: { mode, level: voice.level, armed, stopped, escalation, still: reducedMotion() },
    });
  }, [mode, voice.level, armed, stopped, escalation]);
  const mute = status && status.status !== "ready" ? (voiceReason(status) ?? voiceStatusLabel(status)) : null;
  const size = menu ? MENU_SIZE : pillSize({ mode, armed, stopped, caption, escalation, mute });
  const windowSize = useRef({ width: 0, height: 0 });
  useEffect(() => {
    if (!isTauri()) return;
    const target = { width: size.width + 2 * PILL_MARGIN, height: size.height + 2 * PILL_MARGIN };
    const current = windowSize.current;
    const grown = { width: Math.max(current.width, target.width), height: Math.max(current.height, target.height) };
    if (grown.width !== current.width || grown.height !== current.height) {
      windowSize.current = grown;
      void invoke("orb_resize", grown);
    }
    const timer = setTimeout(() => {
      windowSize.current = target;
      void invoke("orb_resize", target);
    }, RESIZE_MS + 40);
    return () => clearTimeout(timer);
  }, [size.width, size.height]);

  // Clic : console ; double-clic : retour à la place par défaut ; glisser : déplacement.
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
    if (!start || start.dragging || !isTauri()) return;
    if (clickTimer.current) {
      clearTimeout(clickTimer.current);
      clickTimer.current = null;
      void invoke("orb_reset_position");
      return;
    }
    clickTimer.current = setTimeout(() => {
      clickTimer.current = null;
      // Ouvrir la console, c'est voir l'escalade : la marque s'efface.
      state.escalations.forEach((e) => dispatch({ type: "dismiss_escalation", escalation: e.id }));
      void invoke("toggle_console");
    }, DOUBLE_CLICK_MS);
  };

  return (
    <div
      className="orb-root"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onContextMenu={(event) => {
        event.preventDefault();
        setMenu((open) => !open);
      }}
    >
      {menu ? (
        <div className="orb-menu" onPointerDown={(event) => event.stopPropagation()}>
          <button type="button" onClick={() => isTauri() && void invoke("orb_reset_position").then(() => setMenu(false))}>
            Remettre en place
          </button>
          {isMac && <button type="button" onClick={() => {
            if (!isTauri()) return;
            void invoke("set_orb_placement", { placement: "dock" })
              .then(() => setMenu(false))
              .catch((reason: unknown) => { setMenu(false); showCaption(String(reason), 5000); });
          }}>Placer dans le Dock</button>}
          <button type="button" onClick={() => isTauri() && void invoke("quit_app")}>
            Quitter
          </button>
          <button type="button" onClick={() => setMenu(false)} aria-label="Fermer le menu">
            ×
          </button>
        </div>
      ) : (
        <Pill
          mode={mode}
          level={voice.level}
          utterance={utterance}
          caption={caption}
          armed={armed}
          stopped={stopped}
          mute={mute}
          escalation={escalation}
          still={reducedMotion()}
        />
      )}
    </div>
  );
}

// Les appels IPC doivent arriver dans l'ordre des événements du daemon.
let speechCommands: Promise<unknown> = Promise.resolve();
function perform(action: SpeechAction) {
  speechCommands = speechCommands.catch(() => undefined).then(() => {
    if (action.type === "speak") return invoke("voice_speak_delta", { text: action.text });
    if (action.type === "stop") return invoke("voice_stop_speech");
    return invoke("voice_finish_speech");
  });
}
