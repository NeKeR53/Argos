// État de la console : un réducteur pur, nourri par les messages du daemon et par les
// quelques actions locales (connexion, requête envoyée, sélection). Le daemon ne garde
// pas d'historique : les fils de messages commencent à la connexion (phase 1).

import { classifyInput, escalationSource } from "./inputs";
import {
  PROTOCOL_VERSION,
  type AgentId,
  type AgentStatus,
  type Board,
  type Command,
  type DaemonEvent,
  type Origin,
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
  | { seq: number; kind: "input"; text: string; from: Origin | null; console: ConsoleSend | null }
  | { seq: number; kind: "agent"; text: string }
  | { seq: number; kind: "tool"; name: string }
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

/** Un Agent qui a besoin de l'utilisateur (D12), remonté par l'Assistant. */
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
  | { type: "dismiss_escalation"; escalation: number };

/** Au-delà, les lignes les plus anciennes d'un fil sont oubliées. */
export const THREAD_LIMIT = 500;
const NOTICE_LIMIT = 5;
const ESCALATION_LIMIT = 5;

export const initialState: AppState = {
  connection: { status: "connecting" },
  protocol: null,
  agents: {},
  order: [],
  threads: {},
  spent: {},
  boards: {},
  boardRevisions: {},
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
    case "input":
      return onInput(state, event.agent, event.from, event.text);
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
    case "notice":
      return notify(state, "info", event.text, event.agent);
  }
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
    next = append(state, agent, { kind: "input", text, from, console: null });
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
  if (event.type === "notice") return state;
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
    return {
      ...state,
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
  }
}
