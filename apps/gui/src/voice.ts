// Voix côté front (D37, D3) : types des événements du cœur Rust (`voice.rs`) et logique
// pure de lecture des réponses. La voix est réservée à l'Assistant : on lui envoie la
// transcription, on ne lit que ses réponses.

import type { DaemonEvent } from "./protocol";

/** `voice-status` et `voice_status` : la voix est-elle là, et sinon pourquoi. */
export type VoiceStatus =
  | { status: "disabled"; reason: string }
  | { status: "starting" }
  | { status: "ready" }
  | { status: "failed"; reason: string };

/** Réponse de la commande `voice_status`. */
export type VoiceInfo = VoiceStatus & { speak_typed_replies: boolean };

/** `voice-event` : un `argos_voice::VoiceEvent`. */
export type VoiceEvent =
  | { type: "level"; rms: number }
  | { type: "partial"; text: string }
  | { type: "final"; text: string }
  | { type: "speech_started" }
  /** Phrase lancée par `say`, texte exact : la bouche de l'orbe la suit. */
  | { type: "utterance"; text: string }
  | { type: "speech_finished"; interrupted: boolean }
  | { type: "error"; message: string };

/** `voice-transcript` : ce qu'un appui (ou `voice_simulate`) a donné. */
export interface VoiceTranscript {
  text: string;
  simulated: boolean;
}

export function voiceStatusLabel(status: VoiceStatus): string {
  switch (status.status) {
    case "ready":
      return "voix prête";
    case "starting":
      return "voix en démarrage";
    case "disabled":
      return "voix désactivée";
    case "failed":
      return "voix en panne";
  }
}

export function voiceReason(status: VoiceStatus): string | null {
  return status.status === "disabled" || status.status === "failed" ? status.reason : null;
}

// --- Lecture des réponses ------------------------------------------------------------

/**
 * Où en est la lecture des réponses de l'Assistant. Par défaut, on lit la réponse à une
 * demande faite à la voix : le tour qu'ouvre son `input` est lu, jusqu'à `turn_completed`.
 */
export interface SpeechPlan {
  /** Transcriptions envoyées, en attente de leur `input` chez l'Assistant. */
  awaiting: string[];
  /** Le tour en cours est lu. */
  speaking: boolean;
  /** Le message en cours est arrivé en fragments (son texte complet est déjà lu). */
  streamed: boolean;
}

export const IDLE_PLAN: SpeechPlan = { awaiting: [], speaking: false, streamed: false };

export type SpeechAction = { type: "speak"; text: string } | { type: "finish" } | { type: "stop" };

/**
 * Une transcription vient de partir vers l'Assistant : sa réponse sera lue. Le tour est
 * marqué tout de suite, sans attendre l'écho `input` : si cet écho n'arrive pas (connexion
 * de l'orbe pas encore établie au premier usage), la première réponse serait muette.
 */
export function voiceSent(plan: SpeechPlan, text: string): SpeechPlan {
  return { ...plan, awaiting: [...plan.awaiting, text], speaking: true, streamed: false };
}

/** Nouvel appui du push-to-talk : la lecture en cours est coupée, la suite du tour aussi. */
export function bargeIn(plan: SpeechPlan): SpeechPlan {
  return { ...plan, speaking: false, streamed: false };
}

/**
 * Un événement de l'Assistant : que faut-il dire ? `speakAll` (`speak_typed_replies`)
 * fait lire toutes ses réponses, pas seulement celles aux demandes faites à la voix.
 */
export function onAssistantEvent(
  plan: SpeechPlan,
  event: DaemonEvent,
  speakAll: boolean,
): { plan: SpeechPlan; actions: SpeechAction[] } {
  switch (event.type) {
    case "input": {
      // Un retour d’agent ouvre un nouveau tour de l’Assistant. Sa réponse doit être
      // audible, même si le tour de la demande vocale initiale est déjà terminé.
      if (event.from.kind === "agent") {
        return { plan: { ...plan, speaking: true, streamed: false }, actions: [] };
      }
      const index = event.from.kind === "user" ? plan.awaiting.indexOf(event.text) : -1;
      if (index < 0) return speakAll
        ? { plan: { ...plan, speaking: true, streamed: false }, actions: [{ type: "stop" }] }
        // Une demande tapée : son tour n'est pas lu, même si une demande vocale attend.
        : { plan: { ...plan, speaking: false, streamed: false }, actions: [] };
      const awaiting = plan.awaiting.filter((_, at) => at !== index);
      return { plan: { ...plan, awaiting, speaking: true, streamed: false }, actions: [{ type: "stop" }] };
    }
    case "message_delta":
      if (!plan.speaking) return { plan, actions: [] };
      return { plan: { ...plan, streamed: true }, actions: [{ type: "speak", text: event.text }] };
    case "message":
      if (!plan.speaking) return { plan, actions: [] };
      // Sans fragments (moteur qui ne les donne pas), le message complet est lu d'un bloc.
      return {
        plan: { ...plan, streamed: false },
        actions: plan.streamed ? [] : [{ type: "speak", text: event.text }],
      };
    case "turn_completed":
    case "exited":
      if (!plan.speaking) return { plan, actions: [] };
      return { plan: { ...plan, speaking: false, streamed: false }, actions: [{ type: "finish" }] };
    default:
      return { plan, actions: [] };
  }
}
