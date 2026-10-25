// Connexion WebSocket au daemon : URL relue à chaque tentative, reconnexion avec backoff,
// resynchronisation par `tree` à chaque ouverture. Sans React : le hook `useDaemon` la
// branche sur le réducteur.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { encodeRequest, parseServerMessage, ProtocolError, type Command, type ServerMessage } from "./protocol";
import type { Connection } from "./state";

/** Réponse du daemon à une poignée de main (commande Tauri `probe_daemon`). */
export type Probe = "accepted" | "refused" | "unreachable";

/** Où trouver le daemon. */
export interface Endpoint {
  /** URL complète (`ws://…/?token=…`), redemandée à chaque tentative. */
  resolve(): Promise<string>;
  /** Pourquoi une connexion a échoué. Absente dans un navigateur, qui ne le sait pas. */
  probe?(url: string): Promise<Probe>;
}

/**
 * Dans l'app Tauri, l'URL vient du cœur Rust (`$ARGOS_HOME`). Dans un navigateur
 * (`npm run dev`), elle se passe dans l'adresse de la page : `?daemon=<url encodée>`.
 * Rend `null` si aucune des deux n'est disponible.
 */
export function endpointFromEnvironment(search: string = window.location.search): Endpoint | null {
  if (isTauri()) {
    return {
      resolve: () => invoke<string>("daemon_url"),
      probe: (url) => invoke<Probe>("probe_daemon", { url }),
    };
  }
  const url = new URLSearchParams(search).get("daemon");
  return url ? { resolve: async () => url } : null;
}

/** Délai avant la tentative n° `attempt` (1, 2…) : 0,5 s doublé à chaque échec, 10 s au plus. */
export function backoffDelay(attempt: number): number {
  return Math.min(500 * 2 ** Math.max(0, attempt - 1), MAX_DELAY_MS);
}

const MAX_DELAY_MS = 10_000;

export interface ConnectionHandlers {
  onMessage(message: ServerMessage): void;
  onConnection(connection: Connection): void;
  /** Une requête part avec cet id ; appelé avant l'envoi. */
  onRequest(id: number, command: Command): void;
}

export class DaemonConnection {
  private socket: WebSocket | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private attempt = 0;
  private stopped = false;
  /** 0 est réservé aux réponses du daemon à une requête illisible. */
  private nextId = 1;

  constructor(
    private readonly endpoint: Endpoint,
    private readonly handlers: ConnectionHandlers,
  ) {}

  start() {
    this.handlers.onConnection({ status: "connecting" });
    void this.connect();
  }

  /** Ferme la connexion pour de bon (démontage). */
  stop() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.socket?.close();
    this.socket = null;
  }

  /** Envoie une commande ; rend son id, ou `null` si la connexion n'est pas ouverte. */
  send(command: Command): number | null {
    if (this.socket?.readyState !== WebSocket.OPEN) return null;
    const id = this.nextId++;
    this.handlers.onRequest(id, command);
    this.socket.send(encodeRequest(id, command));
    return id;
  }

  private async connect() {
    let url: string;
    try {
      url = await this.endpoint.resolve();
    } catch (error) {
      // Typiquement : pas encore de jeton, le daemon n'a jamais démarré.
      this.retry(String(error));
      return;
    }
    if (this.stopped) return;

    const socket = new WebSocket(url);
    this.socket = socket;
    let opened = false;

    socket.onopen = () => {
      opened = true;
      this.attempt = 0;
      this.handlers.onConnection({ status: "connected" });
      // Le daemon ne rejoue rien : l'arbre se relit à chaque (re)connexion.
      this.send({ command: "tree" });
    };
    socket.onmessage = (frame) => {
      if (typeof frame.data !== "string") return;
      try {
        this.handlers.onMessage(parseServerMessage(frame.data));
      } catch (error) {
        if (!(error instanceof ProtocolError)) throw error;
        console.warn("message du daemon ignoré :", error.message);
      }
    };
    socket.onclose = () => {
      if (this.stopped || this.socket !== socket) return;
      this.socket = null;
      if (opened) {
        this.retry("connexion perdue");
      } else {
        void this.diagnose(url);
      }
    };
  }

  /** La connexion n'a jamais abouti : daemon absent ou jeton refusé ? */
  private async diagnose(url: string) {
    const probe = this.endpoint.probe;
    if (!probe) {
      this.retry("daemon injoignable ou jeton refusé");
      return;
    }
    const verdict = await probe(url).catch((): Probe => "unreachable");
    if (this.stopped) return;
    if (verdict === "refused") {
      // Le jeton ne changera pas tout seul : inutile d'insister vite.
      this.schedule(MAX_DELAY_MS, { status: "unauthorized", retryInMs: MAX_DELAY_MS });
    } else {
      this.retry(verdict === "accepted" ? "connexion interrompue" : "daemon injoignable");
    }
  }

  private retry(reason: string) {
    this.attempt += 1;
    const retryInMs = backoffDelay(this.attempt);
    this.schedule(retryInMs, { status: "reconnecting", attempt: this.attempt, retryInMs, reason });
  }

  private schedule(delay: number, connection: Connection) {
    if (this.stopped) return;
    this.handlers.onConnection(connection);
    this.timer = setTimeout(() => void this.connect(), delay);
  }
}
