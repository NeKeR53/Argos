// La voix vue de la console : état (et pourquoi elle manque), dictée en cours,
// transcriptions envoyées, erreurs. C'est l'orbe qui mène la voix ; la console montre.

import { useEffect } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Action } from "./state";
import type { VoiceEvent, VoiceInfo, VoiceStatus, VoiceTranscript } from "./voice";

export function useVoiceView(dispatch: (action: Action) => void) {
  useEffect(() => {
    if (!isTauri()) return;
    void invoke<VoiceInfo>("voice_status").then((info) => dispatch({ type: "voice_status", status: info }));
    const unlisten = [
      listen<VoiceStatus>("voice-status", (event) => dispatch({ type: "voice_status", status: event.payload })),
      listen<VoiceTranscript>("voice-transcript", (event) =>
        dispatch({ type: "voice_transcript", text: event.payload.text }),
      ),
      listen<VoiceEvent>("voice-event", (event) => {
        const payload = event.payload;
        if (payload.type === "partial") dispatch({ type: "voice_partial", text: payload.text });
        if (payload.type === "error") dispatch({ type: "voice_error", message: payload.message });
      }),
    ];
    return () => unlisten.forEach((pending) => void pending.then((stop) => stop()));
  }, [dispatch]);
}

/** Mode d'essai sans micro (app de dev seulement) : comme si l'utilisateur avait dit `text`. */
export function simulateVoice(text: string) {
  void invoke("voice_simulate", { text });
}
