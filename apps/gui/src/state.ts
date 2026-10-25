// État de la console : un réducteur pur, nourri par les messages du daemon et par les
// quelques actions locales (connexion, requête envoyée, sélection). Le daemon ne garde
// pas d'historique : les fils de messages commencent à la connexion (phase 1).

import {
  PROTOCOL_VERSION,
  type AgentId,
  type AgentStatus,
  type Command,
  type DaemonEvent,
  type Reply,
  type ServerMessage,
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

/** Sort d'un message envoyé par l'utilisateur. */
export type Delivery = "pending" | "delivered" | "failed" | "lost";

/** Une ligne du fil d'un agent. `seq` sert de clé stable à l'affichage. */
export type ThreadEntry =
  | { seq: number; kind: "user"; text: string; request: number; delivery: Delivery }
  | { seq: number; kind: "agent"; text: string }
  | { seq: number; kind: "tool"; name: string }
  | { seq: number; kind: "turn"; outcome: TurnOutcome; usage: Usage | null }
  | { seq: number; kind: "exited"; code: number | null };

export interface Notice {
  id: number;
  tone: "error" | "warning";
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
  /** Requêtes sans réponse, par id. */
  pending: Record<number, Command>;
  notices: Notice[];
  /** Agent affiché ; `null` = l'Assistant. */
  selected: AgentId | null;
  /** Compteur des clés de lignes et d'avis. */
  seq: number;
}

export type Action =
  | { type: "server"; message: ServerMessage }
  | { type: "connection"; connection: Connection }
  /** Une requête vient de partir avec cet id. */
  | { type: "requested"; id: number; command: Command }
  | { type: "select"; agent: AgentId }
  | { type: "dismiss"; notice: number };

/** Au-delà, les lignes les plus anciennes d'un fil sont oubliées. */
export const THREAD_LIMIT = 500;
const NOTICE_LIMIT = 5;

export const initialState: AppState = {
  connection: { status: "connecting" },
  protocol: null,
  agents: {},
  order: [],
  threads: {},
  spent: {},
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
      if (command.command !== "send") return next;
      return append(next, command.agent, {
        kind: "user",
        text: command.text,
        request: action.id,
        delivery: "pending",
      });
    }
    case "select":
      return { ...state, selected: action.agent };
    case "dismiss":
      return { ...state, notices: state.notices.filter((notice) => notice.id !== action.notice) };
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
      return onEvent(state, message.event);
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
    case "done":
      return command?.command === "send" ? deliver(next, command.agent, id, "delivered") : next;
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
    case "message":
      return append(state, event.agent, { kind: "agent", text: event.text });
    case "tool":
      return append(state, event.agent, { kind: "tool", name: event.name });
    case "turn_completed": {
      const next = append(state, event.agent, {
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
      return append(state, event.agent, { kind: "exited", code: event.code });
    case "agent_updated":
      return upsert(state, event.agent);
  }
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
    return {
      ...state,
      agents,
      threads,
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

function setDelivery(thread: ThreadEntry[], request: number, delivery: Delivery): ThreadEntry[] {
  return thread.map((entry) =>
    entry.kind === "user" && entry.request === request ? { ...entry, delivery } : entry,
  );
}

function notify(state: AppState, tone: Notice["tone"], text: string): AppState {
  const seq = state.seq + 1;
  return { ...state, seq, notices: [...state.notices, { id: seq, tone, text }].slice(-NOTICE_LIMIT) };
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
  }
}
