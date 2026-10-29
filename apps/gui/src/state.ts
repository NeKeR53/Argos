// État de la console : un réducteur pur, nourri par les messages du daemon et par les
// quelques actions locales (connexion, requête envoyée, sélection). Le daemon ne garde
// pas d'historique : les fils de messages commencent à la connexion (phase 1).

import { classifyInput, escalationSource } from "./inputs";
import { voiceReason, type VoiceStatus } from "./voice";
import {
  PROTOCOL_VERSION,
  type AgentId,
  type AgentStatus,
  type Board,
  type Command,
  type DaemonEvent,
  type Json,
  type Origin,
  type Reply,
  type ServerMessage,
  type SurfaceDoc,
  type SurfaceMeta,
  type NodeStatus,
  type ProfileInfo,
  type Trigger,
  type TurnOutcome,
  type Usage,
} from "./protocol";

/** État de la connexion au daemon. */
export type Connection =
  /** Première tentative en cours. */
  | { status: "connecting" }
  | { status: "connected" }
  /** Connexion perdue ou impossible ; nouvel essai après `retryInMs`. */
  | { status: "reconnecting"; attempt: number; retryInMs: number; reason: string }
  /** Le daemon écoute mais refuse le jeton. */
  | { status: "unauthorized"; retryInMs: number }
  /** Aucune URL de daemon : ni API Tauri, ni paramètre `?daemon=`. */
  | { status: "unconfigured"; reason: string };

/** Sort d'un message envoyé depuis cette console. */
export type Delivery = "pending" | "delivered" | "failed" | "lost";

/** Suivi d'un message parti de cette console, en attendant son `input`. */
export interface ConsoleSend {
  request: number;
  delivery: Delivery;
  /** Son `input` n'est pas encore arrivé : le prochain `input` de même texte le confirme. */
  awaitingEcho: boolean;
}

/** Une ligne du fil d'un agent. `seq` sert de clé stable à l'affichage. */
export type ThreadEntry =
  /**
   * Ce que l'agent reçoit ; `console` est présent pour un message parti d'ici, `from` une
   * fois l'expéditeur connu par l'événement du daemon.
   */
  | {
      seq: number;
      kind: "input";
      text: string;
      from: Origin | null;
      console: ConsoleSend | null;
      /** Demande faite à la voix (transcription envoyée par l'orbe). */
      voice?: boolean;
    }
  | { seq: number; kind: "agent"; text: string }
  /** Message en cours d'écriture (`message_delta`), remplacé par le message complet. */
  | { seq: number; kind: "streaming"; text: string }
  | { seq: number; kind: "tool"; name: string; id?: string; input?: string | null; output?: string | null; finished?: boolean }
  | { seq: number; kind: "turn"; outcome: TurnOutcome; usage: Usage | null }
  | { seq: number; kind: "exited"; code: number | null };

export interface Notice {
  id: number;
  /** `info` : information du daemon (événement `notice`). */
  tone: "error" | "warning" | "info";
  text: string;
  /** Agent concerné, pour y mener l'utilisateur. */
  agent?: AgentId | null;
}

/**
 * Une ligne du flux d'une surface, rejoué en entier à chaque chargement de la page puis
 * remis au fil de l'eau par le pont : textes envoyés depuis la surface, messages de son
 * agent, état du tour, données poussées par l'agent.
 */
export type FeedItem =
  | { seq: number; type: "sent"; text: string }
  | { seq: number; type: "message"; text: string }
  | { seq: number; type: "busy"; busy: boolean }
  | { seq: number; type: "data"; data: Json };

/** Un Agent qui a besoin de l'utilisateur (D12), remonté par l'Assistant. */
export interface VoiceView {
  /** `null` hors de l'app Tauri : pas de voix dans un navigateur. */
  status: VoiceStatus | null;
  /** Transcription partielle de l'appui en cours, montrée dans le fil de l'Assistant. */
  dictation: string | null;
  /** Transcriptions envoyées à l'Assistant, en attente de leur `input`. */
  spoken: string[];
}

export interface Escalation {
  id: number;
  /** Nom de l'Agent tel qu'écrit dans le préfixe. */
  name: string;
  /** Son id, quand le préfixe le donne. */
  agent: AgentId | null;
  text: string;
}

export interface AppState {
  connection: Connection;
  /** Version annoncée par le daemon dans `hello`. */
  protocol: number | null;
  agents: Record<AgentId, AgentStatus>;
  /** Ordre du registre (création), pour un arbre stable. */
  order: AgentId[];
  threads: Record<AgentId, ThreadEntry[]>;
  /** Coût cumulé par agent depuis la connexion de la GUI (USD). */
  spent: Record<AgentId, number>;
  /** Dernier board reçu, par Agent. */
  boards: Record<AgentId, Board>;
  /**
   * Compteur incrémenté à chaque événement qui concerne un Agent ou l'un de ses Workers :
   * son board affiché est à relire quand il change.
   */
  boardRevisions: Record<AgentId, number>;
  /** Surfaces de chaque agent, par ordre de création. */
  surfaces: Record<AgentId, SurfaceMeta[]>;
  /** Code des surfaces déjà lues, par id de surface. */
  surfaceDocs: Record<string, SurfaceDoc>;
  /** Ce que le pont doit remettre à chaque surface, par id de surface (voir `FeedItem`). */
  surfaceFeeds: Record<string, FeedItem[]>;
  /**
   * Compteur incrémenté à chaque `agent_updated` d'un agent : sa liste de surfaces affichée
   * est relue (rattrape une surface créée ou modifiée sans événement).
   */
  surfaceRevisions: Record<AgentId, number>;
  /** Déclencheurs dont l'agent est propriétaire ou cible, par agent. */
  triggers: Record<AgentId, Trigger[]>;
  /**
   * Compteur incrémenté quand un déclencheur d'un agent part ou change : la liste
   * affichée est relue.
   */
  triggerRevisions: Record<AgentId, number>;
  /** La voix vue de la console : état, dictée en cours, transcriptions envoyées. */
  voice: VoiceView;
  /** Nœuds déclarés (D32), relus régulièrement ; `null` avant la première lecture. */
  nodes: NodeStatus[] | null;
  /** Profils de la politique de modèles, pour choisir le modèle d'un agent. */
  profiles: ProfileInfo[] | null;
  /** Escalades encore affichées dans le bandeau. */
  escalations: Escalation[];
  /** Escalades arrivées depuis la dernière consultation de l'Assistant. */
  unseenEscalations: number;
  /** Requêtes sans réponse, par id. */
  pending: Record<number, Command>;
  notices: Notice[];
  /** Agent affiché ; `null` = l'Assistant. */
  selected: AgentId | null;
  /** Compteur des clés de lignes, d'avis et d'escalades. */
  seq: number;
}

export type Action =
  | { type: "server"; message: ServerMessage }
  | { type: "connection"; connection: Connection }
  /** Une requête vient de partir avec cet id. */
  | { type: "requested"; id: number; command: Command }
  | { type: "select"; agent: AgentId }
  | { type: "dismiss"; notice: number }
  | { type: "dismiss_escalation"; escalation: number }
  /** Voix (événements du cœur Rust, voir `voice.ts`). */
  | { type: "voice_status"; status: VoiceStatus }
  | { type: "voice_partial"; text: string }
  | { type: "voice_transcript"; text: string }
  | { type: "voice_error"; message: string };

/** Au-delà, les lignes les plus anciennes d'un fil sont oubliées. */
export const THREAD_LIMIT = 500;
/** Au-delà, les lignes les plus anciennes du flux d'une surface sont oubliées. */
export const FEED_LIMIT = 500;
const NOTICE_LIMIT = 5;
const ESCALATION_LIMIT = 5;
/** Transcriptions gardées en attente de leur `input` (au-delà, les plus vieilles partent). */
const SPOKEN_LIMIT = 10;

export const initialState: AppState = {
  connection: { status: "connecting" },
  protocol: null,
  agents: {},
  order: [],
  threads: {},
  spent: {},
  boards: {},
  boardRevisions: {},
  surfaces: {},
  surfaceDocs: {},
  surfaceFeeds: {},
  surfaceRevisions: {},
  triggers: {},
  triggerRevisions: {},
  voice: { status: null, dictation: null, spoken: [] },
  nodes: null,
  profiles: null,
  escalations: [],
  unseenEscalations: 0,
  pending: {},
  notices: [],
  selected: null,
  seq: 0,
};

export function reduce(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "server":
      return onServerMessage(state, action.message);
    case "connection":
      return onConnection(state, action.connection);
    case "requested": {
      const next = { ...state, pending: { ...state.pending, [action.id]: action.command } };
      const command = action.command;
      // Le chat de la surface montre ce qui en est parti, y compris après un rechargement.
      if (command.command === "surface_send") return feed(next, command.surface, { type: "sent", text: command.text });
      if (command.command !== "send") return next;
      // Affiché tout de suite ; l'`input` du daemon le confirmera sans le dupliquer.
      return append(next, command.agent, {
        kind: "input",
        text: command.text,
        from: null,
        console: { request: action.id, delivery: "pending", awaitingEcho: true },
      });
    }
    case "select": {
      const next = { ...state, selected: action.agent };
      // Consulter l'Assistant, c'est voir les escalades qu'il a reçues.
      return state.agents[action.agent]?.level === "assistant" ? { ...next, unseenEscalations: 0 } : next;
    }
    case "dismiss":
      return { ...state, notices: state.notices.filter((notice) => notice.id !== action.notice) };
    case "dismiss_escalation":
      return { ...state, escalations: state.escalations.filter((e) => e.id !== action.escalation) };
    case "voice_status": {
      const previous = state.voice.status;
      const next = { ...state, voice: { ...state.voice, status: action.status } };
      const reason = voiceReason(action.status);
      // Pas de voix : on le dit une fois, avec la raison.
      if (!reason || (previous && voiceReason(previous) === reason)) return next;
      return notify(next, action.status.status === "failed" ? "warning" : "info", `Voix indisponible : ${reason}`);
    }
    case "voice_partial":
      return { ...state, voice: { ...state.voice, dictation: action.text } };
    case "voice_transcript": {
      const text = action.text.trim();
      const spoken = text ? [...state.voice.spoken, text].slice(-SPOKEN_LIMIT) : state.voice.spoken;
      return { ...state, voice: { ...state.voice, dictation: null, spoken } };
    }
    case "voice_error":
      return notify(state, "error", `Voix : ${action.message}`);
  }
}

function onServerMessage(state: AppState, message: ServerMessage): AppState {
  switch (message.type) {
    case "hello":
      if (message.protocol === PROTOCOL_VERSION) return { ...state, protocol: message.protocol };
      return notify(
        { ...state, protocol: message.protocol },
        "warning",
        `Le daemon parle le protocole v${message.protocol}, la GUI attend v${PROTOCOL_VERSION} : mettez-les à jour ensemble.`,
      );
    case "reply":
      return onReply(state, message.id, message.reply);
    case "event":
      return onEvent(touchBoard(state, message.event), message.event);
  }
}

function onReply(state: AppState, id: number, reply: Reply): AppState {
  const command = state.pending[id];
  const pending = { ...state.pending };
  delete pending[id];
  let next: AppState = { ...state, pending };

  switch (reply.kind) {
    case "tree":
      return withTree(next, reply.agents);
    case "agent":
      return upsert(next, reply.agent);
    case "board":
      return { ...next, boards: { ...next.boards, [reply.board.agent]: reply.board } };
    case "surfaces": {
      // La réponse ne dit pas de quel agent il s'agit quand la liste est vide : la requête le dit.
      const agent = command?.command === "surfaces" ? command.agent : reply.surfaces[0]?.agent;
      if (!agent) return next;
      return { ...next, surfaces: { ...next.surfaces, [agent]: sortedSurfaces(reply.surfaces) } };
    }
    case "surface": {
      const { html: _, ...meta } = reply.surface;
      const withDoc = { ...next, surfaceDocs: { ...next.surfaceDocs, [meta.id]: reply.surface } };
      return withSurface(withDoc, meta);
    }
    case "triggers":
      if (command?.command !== "triggers") return next;
      return { ...next, triggers: { ...next.triggers, [command.agent]: reply.triggers } };
    case "nodes":
      return { ...next, nodes: reply.nodes };
    case "models":
      return { ...next, profiles: reply.profiles };
    case "capabilities":
      return next;
    case "function_result":
      // Le résultat revient à la surface appelante, sans alimenter le chat de l'agent.
      return next;
    case "done":
      if (command?.command === "send") return deliver(next, command.agent, id, "delivered");
      // Un déclencheur actionné, suspendu ou supprimé : les listes qui le montrent sont relues.
      if (
        command?.command === "fire_trigger" ||
        command?.command === "set_trigger" ||
        command?.command === "delete_trigger"
      ) {
        return touchTriggersOf(next, command.trigger);
      }
      return next;
    case "error": {
      // Id 0 ou inconnu : requête illisible, ou réponse d'une connexion précédente.
      const context = command ? describeCommand(command, next.agents) : "Le daemon signale";
      if (command?.command === "send") next = deliver(next, command.agent, id, "failed");
      return notify(next, "error", `${context} : ${reply.message}`);
    }
  }
}

function onEvent(state: AppState, event: DaemonEvent): AppState {
  switch (event.type) {
    case "input": {
      // L'agent se met au travail : ses surfaces le montrent (`argos.on('busy')`).
      let next = feedAgent(onInput(state, event.agent, event.from, event.text), event.agent, {
        type: "busy",
        busy: true,
      });
      // Un déclencheur vient de partir : sa date de dernier déclenchement a changé.
      if (classifyInput(event.text).category === "trigger") next = touchTriggersAiming(next, event.agent);
      return next;
    }
    case "message_delta":
      return withDelta(state, event.agent, event.text);
    case "message": {
      const next = withMessage(state, event.agent, event.text);
      return feedAgent(next, event.agent, { type: "message", text: event.text });
    }
    case "tool":
      return append(state, event.agent, { kind: "tool", name: event.name });
    case "tool_detail": {
      const thread = state.threads[event.agent] ?? [];
      const index = thread.findIndex((entry) => entry.kind === "tool" && entry.id === event.id);
      if (index < 0) return append(state, event.agent, { kind: "tool", id: event.id, name: event.name || "Outil", input: event.input, output: event.output, finished: event.finished });
      const updated = thread.map((entry, i) => i === index && entry.kind === "tool" ? { ...entry, name: event.name || entry.name, input: event.input ?? entry.input, output: event.output ?? entry.output, finished: event.finished } : entry);
      return { ...state, threads: { ...state.threads, [event.agent]: updated } };
    }
    case "turn_completed": {
      const settled = settleStreaming(state, event.agent);
      const next = append(feedAgent(settled, event.agent, { type: "busy", busy: false }), event.agent, {
        kind: "turn",
        outcome: event.outcome,
        usage: event.usage,
      });
      const cost = event.usage?.cost_usd;
      if (cost == null) return next;
      return {
        ...next,
        spent: { ...next.spent, [event.agent]: (next.spent[event.agent] ?? 0) + cost },
      };
    }
    case "exited":
      return append(settleStreaming(state, event.agent), event.agent, { kind: "exited", code: event.code });
    case "agent_updated": {
      const id = event.agent.id;
      const next = upsert(state, event.agent);
      return { ...next, surfaceRevisions: { ...next.surfaceRevisions, [id]: (next.surfaceRevisions[id] ?? 0) + 1 } };
    }
    case "notice":
      return notify(state, "info", event.text, event.agent);
    case "surface_updated":
      return event.deleted ? withoutSurface(state, event.surface.id) : withSurface(state, event.surface);
    case "surface_data":
      return feed(state, event.surface, { type: "data", data: event.data });
  }
}

// --- Texte en streaming -----------------------------------------------------------

/** Index de la ligne en cours d'écriture d'un fil, ou -1. */
function streamingIndex(thread: ThreadEntry[]): number {
  for (let index = thread.length - 1; index >= 0; index--) {
    if (thread[index].kind === "streaming") return index;
  }
  return -1;
}

/** Un fragment s'ajoute au message en cours, ou en ouvre un. */
function withDelta(state: AppState, agent: AgentId, text: string): AppState {
  const thread = state.threads[agent] ?? [];
  const index = streamingIndex(thread);
  if (index < 0) return append(state, agent, { kind: "streaming", text });
  const updated = thread.map((entry, at) =>
    at === index && entry.kind === "streaming" ? { ...entry, text: entry.text + text } : entry,
  );
  return { ...state, threads: { ...state.threads, [agent]: updated } };
}

/** Le message complet prend la place du texte en cours, sans doublon. */
function withMessage(state: AppState, agent: AgentId, text: string): AppState {
  const thread = state.threads[agent] ?? [];
  const index = streamingIndex(thread);
  if (index < 0) return append(state, agent, { kind: "agent", text });
  const updated = thread.map((entry, at): ThreadEntry => (at === index ? { seq: entry.seq, kind: "agent", text } : entry));
  return { ...state, threads: { ...state.threads, [agent]: updated } };
}

/** Tour fini sans message complet (interruption, panne) : le texte reçu reste tel quel. */
function settleStreaming(state: AppState, agent: AgentId): AppState {
  const thread = state.threads[agent] ?? [];
  const index = streamingIndex(thread);
  if (index < 0) return state;
  const entry = thread[index];
  return withMessage(state, agent, entry.kind === "streaming" ? entry.text : "");
}

// --- Déclencheurs -----------------------------------------------------------------

function touchTriggers(state: AppState, agent: AgentId): AppState {
  return {
    ...state,
    triggerRevisions: { ...state.triggerRevisions, [agent]: (state.triggerRevisions[agent] ?? 0) + 1 },
  };
}

/** Un déclencheur a visé `target` : sa liste, et celles de leurs propriétaires, sont relues. */
function touchTriggersAiming(state: AppState, target: AgentId): AppState {
  const owners = Object.entries(state.triggers)
    .filter(([agent, list]) => agent !== target && list.some((trigger) => trigger.target === target))
    .map(([agent]) => agent);
  return [target, ...owners].reduce(touchTriggers, state);
}

/** Relit les listes qui montrent ce déclencheur. */
function touchTriggersOf(state: AppState, trigger: string): AppState {
  return Object.entries(state.triggers)
    .filter(([, list]) => list.some((candidate) => candidate.id === trigger))
    .reduce((next, [agent]) => touchTriggers(next, agent), state);
}

// --- Surfaces ---------------------------------------------------------------------

function sortedSurfaces(surfaces: SurfaceMeta[]): SurfaceMeta[] {
  return [...surfaces].sort((a, b) => a.created_at - b.created_at || a.id.localeCompare(b.id));
}

/** Ajoute ou met à jour une surface dans la liste de son agent. */
function withSurface(state: AppState, meta: SurfaceMeta): AppState {
  const list = state.surfaces[meta.agent] ?? [];
  const others = list.filter((surface) => surface.id !== meta.id);
  return { ...state, surfaces: { ...state.surfaces, [meta.agent]: sortedSurfaces([...others, meta]) } };
}

/** Une surface supprimée disparaît partout : liste, code, flux. */
function withoutSurface(state: AppState, id: string): AppState {
  const surfaces: Record<AgentId, SurfaceMeta[]> = {};
  for (const [agent, list] of Object.entries(state.surfaces)) {
    surfaces[agent] = list.filter((surface) => surface.id !== id);
  }
  const surfaceDocs = { ...state.surfaceDocs };
  delete surfaceDocs[id];
  const surfaceFeeds = { ...state.surfaceFeeds };
  delete surfaceFeeds[id];
  return { ...state, surfaces, surfaceDocs, surfaceFeeds };
}

type NewFeedItem = FeedItem extends infer I ? (I extends unknown ? Omit<I, "seq"> : never) : never;

/** Ajoute une ligne au flux d'une surface. */
function feed(state: AppState, surface: string, item: NewFeedItem): AppState {
  const seq = state.seq + 1;
  const items = [...(state.surfaceFeeds[surface] ?? []), { ...item, seq } as FeedItem].slice(-FEED_LIMIT);
  return { ...state, seq, surfaceFeeds: { ...state.surfaceFeeds, [surface]: items } };
}

/** Ajoute une ligne au flux de chacune des surfaces connues d'un agent. */
function feedAgent(state: AppState, agent: AgentId, item: NewFeedItem): AppState {
  return (state.surfaces[agent] ?? []).reduce((next, surface) => feed(next, surface.id, item), state);
}

/**
 * Ce que l'agent reçoit. Un message parti de cette console est déjà affiché : son
 * `input` le confirme au lieu de l'ajouter une seconde fois.
 */
function onInput(state: AppState, agent: AgentId, from: Origin, text: string): AppState {
  const thread = state.threads[agent] ?? [];
  const echoed = thread.findIndex(
    (entry) =>
      entry.kind === "input" &&
      entry.console?.awaitingEcho === true &&
      entry.console.delivery !== "failed" &&
      entry.text === text,
  );
  let next: AppState;
  if (echoed >= 0) {
    const updated = thread.map((entry, index) =>
      index === echoed && entry.kind === "input" && entry.console
        ? { ...entry, from, console: { ...entry.console, delivery: "delivered" as const, awaitingEcho: false } }
        : entry,
    );
    next = { ...state, threads: { ...state.threads, [agent]: updated } };
  } else {
    // Une transcription envoyée par l'orbe revient ici comme un message de l'utilisateur.
    const spoken =
      from.kind === "user" && state.agents[agent]?.level === "assistant" ? state.voice.spoken.indexOf(text) : -1;
    const base =
      spoken < 0
        ? state
        : { ...state, voice: { ...state.voice, spoken: state.voice.spoken.filter((_, at) => at !== spoken) } };
    next = append(base, agent, { kind: "input", text, from, console: null, ...(spoken < 0 ? {} : { voice: true }) });
  }
  return state.agents[agent]?.level === "assistant" ? withEscalation(next, text) : next;
}

/** Une escalade reçue par l'Assistant doit se voir où que regarde l'utilisateur. */
function withEscalation(state: AppState, text: string): AppState {
  const { tag, body } = classifyInput(text);
  const source = escalationSource(tag);
  if (!source) return state;
  const seq = state.seq + 1;
  const assistantShown = selectedAgent(state)?.level === "assistant";
  return {
    ...state,
    seq,
    escalations: [...state.escalations, { id: seq, ...source, text: body }].slice(-ESCALATION_LIMIT),
    unseenEscalations: assistantShown ? 0 : state.unseenEscalations + 1,
  };
}

/** Marque à relire le board de l'Agent concerné par l'événement (lui ou son Worker). */
function touchBoard(state: AppState, event: DaemonEvent): AppState {
  // Les fragments de texte ne changent pas le board ; les relire à chaque fragment
  // ferait une requête par mot.
  if (
    event.type === "notice" ||
    event.type === "surface_updated" ||
    event.type === "surface_data" ||
    event.type === "message_delta"
  ) {
    return state;
  }
  const subject = event.type === "agent_updated" ? event.agent : state.agents[event.agent];
  if (!subject) return state;
  const owner =
    subject.level === "agent" ? subject.id : subject.level === "worker" ? subject.parent : null;
  if (!owner) return state;
  return {
    ...state,
    boardRevisions: { ...state.boardRevisions, [owner]: (state.boardRevisions[owner] ?? 0) + 1 },
  };
}

function onConnection(state: AppState, connection: Connection): AppState {
  const next = { ...state, connection };
  if (connection.status === "connected" || Object.keys(state.pending).length === 0) return next;
  // Connexion perdue : les réponses attendues n'arriveront plus. Un message en attente
  // a pu être reçu ou non ; on le dit plutôt que de laisser croire qu'il est parti.
  const threads = { ...state.threads };
  for (const [request, command] of Object.entries(state.pending)) {
    if (command.command !== "send") continue;
    threads[command.agent] = setDelivery(threads[command.agent] ?? [], Number(request), "lost");
  }
  return { ...next, threads, pending: {} };
}

/** Remplace l'arbre (resynchronisation complète) en gardant les fils. */
function withTree(state: AppState, agents: AgentStatus[]): AppState {
  const living = agents.filter((agent) => agent.state !== "retired");
  return {
    ...state,
    agents: Object.fromEntries(living.map((agent) => [agent.id, agent])),
    order: living.map((agent) => agent.id),
    selected: state.selected && living.some((a) => a.id === state.selected) ? state.selected : null,
  };
}

/** Ajoute ou met à jour un agent ; un agent retiré quitte l'arbre. */
function upsert(state: AppState, agent: AgentStatus): AppState {
  if (agent.state === "retired") {
    if (!(agent.id in state.agents)) return state;
    const agents = { ...state.agents };
    delete agents[agent.id];
    const threads = { ...state.threads };
    delete threads[agent.id];
    const boards = { ...state.boards };
    delete boards[agent.id];
    // Ses surfaces partent avec lui.
    const cleared = (state.surfaces[agent.id] ?? []).reduce((next, s) => withoutSurface(next, s.id), state);
    const surfaces = { ...cleared.surfaces };
    delete surfaces[agent.id];
    // Le daemon supprime ses déclencheurs avec lui.
    const triggers = { ...cleared.triggers };
    delete triggers[agent.id];
    return {
      ...cleared,
      surfaces,
      triggers,
      agents,
      threads,
      boards,
      order: state.order.filter((id) => id !== agent.id),
      selected: state.selected === agent.id ? null : state.selected,
    };
  }
  const known = agent.id in state.agents;
  return {
    ...state,
    agents: { ...state.agents, [agent.id]: agent },
    order: known ? state.order : [...state.order, agent.id],
  };
}

/** Une ligne avant numérotation (`Omit` appliqué à chaque variante). */
type NewEntry = ThreadEntry extends infer E ? (E extends unknown ? Omit<E, "seq"> : never) : never;

function append(state: AppState, agent: AgentId, entry: NewEntry): AppState {
  const seq = state.seq + 1;
  const thread = [...(state.threads[agent] ?? []), { ...entry, seq } as ThreadEntry];
  return {
    ...state,
    seq,
    threads: { ...state.threads, [agent]: thread.slice(-THREAD_LIMIT) },
  };
}

function deliver(state: AppState, agent: AgentId, request: number, delivery: Delivery): AppState {
  const thread = state.threads[agent];
  if (!thread) return state;
  return { ...state, threads: { ...state.threads, [agent]: setDelivery(thread, request, delivery) } };
}

/**
 * Sort d'un message de la console. « remis » ne défait pas une confirmation déjà reçue ;
 * « perdu » ou « refusé » arrêtent l'attente de l'`input`.
 */
function setDelivery(thread: ThreadEntry[], request: number, delivery: Delivery): ThreadEntry[] {
  return thread.map((entry) => {
    if (entry.kind !== "input" || entry.console?.request !== request) return entry;
    const awaitingEcho = delivery === "delivered" ? entry.console.awaitingEcho : false;
    return { ...entry, console: { ...entry.console, delivery, awaitingEcho } };
  });
}

function notify(state: AppState, tone: Notice["tone"], text: string, agent: AgentId | null = null): AppState {
  const seq = state.seq + 1;
  const notice: Notice = agent ? { id: seq, tone, text, agent } : { id: seq, tone, text };
  return { ...state, seq, notices: [...state.notices, notice].slice(-NOTICE_LIMIT) };
}

// --- Lectures dérivées ------------------------------------------------------------

export function shortId(id: AgentId): string {
  return id.slice(0, 8);
}

/** Nom affiché : celui de la charte, « Assistant », ou l'id court d'un Worker. */
export function displayName(agent: AgentStatus): string {
  if (agent.charter) return agent.charter.name;
  if (agent.level === "assistant") return "Assistant";
  return shortId(agent.id);
}

/** L'utilisateur s'adresse à l'Assistant et aux Agents, jamais à un Worker (D2). */
export function isAddressable(agent: AgentStatus): boolean {
  return agent.level !== "worker";
}

/** Seuls les Agents se retirent : l'Assistant est unique et permanent. */
export function isRetirable(agent: AgentStatus): boolean {
  return agent.level === "agent";
}

export function assistantOf(state: AppState): AgentStatus | null {
  return Object.values(state.agents).find((agent) => agent.level === "assistant") ?? null;
}

/** L'agent affiché dans le panneau de chat : la sélection, sinon l'Assistant. */
export function selectedAgent(state: AppState): AgentStatus | null {
  return (state.selected && state.agents[state.selected]) || assistantOf(state);
}

export interface TreeNode {
  agent: AgentStatus;
  children: TreeNode[];
}

/**
 * L'arbre Assistant → Agents → Workers, dans l'ordre du registre. Un agent dont le
 * parent est inconnu reste visible à la racine plutôt que de disparaître.
 */
export function treeOf(state: AppState): TreeNode[] {
  const nodes = new Map<AgentId, TreeNode>();
  for (const id of state.order) {
    const agent = state.agents[id];
    if (agent) nodes.set(id, { agent, children: [] });
  }
  const roots: TreeNode[] = [];
  for (const node of nodes.values()) {
    const parent = node.agent.parent ? nodes.get(node.agent.parent) : undefined;
    (parent ? parent.children : roots).push(node);
  }
  return roots;
}

/** Libellé d'une commande pour un message d'erreur : « Parking de Domotique ». */
export function describeCommand(command: Command, agents: Record<AgentId, AgentStatus>): string {
  const target = (id: AgentId) => (agents[id] ? displayName(agents[id]) : shortId(id));
  switch (command.command) {
    case "tree":
      return "Lecture de l'arbre";
    case "inspect":
      return `Inspection de ${target(command.agent)}`;
    case "board":
      return `Lecture du board de ${target(command.agent)}`;
    case "send":
      return `Message à ${target(command.agent)}`;
    case "interrupt":
      return `Interruption de ${target(command.agent)}`;
    case "park":
      return `Parking de ${target(command.agent)}`;
    case "stop":
      return `Arrêt de ${target(command.agent)}`;
    case "retire":
      return `Retrait de ${target(command.agent)}`;
    case "panic_stop":
      return "Panic stop";
    case "surfaces":
      return `Lecture des surfaces de ${target(command.agent)}`;
    case "surface":
      return `Lecture de la surface ${command.surface}`;
    case "surface_send":
      return `Envoi depuis la surface ${command.surface}`;
    case "surface_emit":
      return `Événement « ${command.name} » de la surface ${command.surface}`;
    case "surface_call":
      return `Fonction « ${command.function} » de la surface ${command.surface}`;
    case "surface_delete":
      return `Suppression de la surface ${command.surface}`;
    case "triggers":
      return `Lecture des déclencheurs de ${target(command.agent)}`;
    case "fire_trigger":
      return `Déclencheur ${command.trigger}`;
    case "set_trigger":
      return `${command.enabled ? "Activation" : "Suspension"} du déclencheur ${command.trigger}`;
    case "delete_trigger":
      return `Suppression du déclencheur ${command.trigger}`;
    case "nodes":
      return "Lecture des nœuds";
    case "models":
      return "Lecture des modèles";
    case "capabilities":
      return `Lecture des capacités de ${target(command.agent)}`;
    case "set_model":
      return `Changement de modèle (${command.model})`;
    case "set_workspace":
      return `Rattachement du workspace de ${target(command.agent)}`;
  }
}
