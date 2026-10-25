// Protocole de l'API WebSocket du daemon, miroir de `crates/argos-core/src/protocol.rs`.
// Chaque message est un objet JSON sur une trame texte. Le client envoie des requêtes ;
// le daemon répond par `reply` (même `id`) et pousse en continu des `event`.
// Toute évolution du protocole côté Rust doit se refléter ici.

/** Version du protocole que cette GUI comprend (`argos_core::PROTOCOL_VERSION`). */
export const PROTOCOL_VERSION = 1;

export type AgentId = string;
export type Level = "assistant" | "agent" | "worker";
export type EngineKind = "claude" | "codex";
export type AgentState = "active" | "parked" | "retired";

/** Fiche d'identité d'un Agent (D20). */
export interface Charter {
  name: string;
  scope: string;
}

/** Un agent tel que le registre le connaît (`AgentRecord`). */
export interface AgentRecord {
  id: AgentId;
  level: Level;
  /** `null` pour l'Assistant seulement. */
  parent: AgentId | null;
  engine: EngineKind;
  model: string | null;
  workdir: string;
  /** Présente pour les Agents seulement. */
  charter: Charter | null;
  state: AgentState;
  session_id: string | null;
  /** Secondes depuis l'époque Unix. */
  created_at: number;
}

/** `AgentStatus` : le record aplati (`serde(flatten)`) plus l'état d'exécution. */
export interface AgentStatus extends AgentRecord {
  /** Son process tourne. */
  running: boolean;
  /** Un tour est en cours. */
  busy: boolean;
}

/** Issue d'un tour (`TurnOutcome`, tag `status`). */
export type TurnOutcome =
  | { status: "completed" }
  | { status: "interrupted" }
  | { status: "failed"; message: string };

/** Consommation d'un tour ; `input_tokens` inclut les tokens servis par le cache. */
export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number | null;
}

/** Ce que l'utilisateur peut demander (`Command`, tag `command`). */
export type Command =
  | { command: "tree" }
  | { command: "inspect"; agent: AgentId }
  | { command: "send"; agent: AgentId; text: string }
  | { command: "interrupt"; agent: AgentId }
  | { command: "park"; agent: AgentId }
  | { command: "stop"; agent: AgentId }
  | { command: "retire"; agent: AgentId }
  | { command: "panic_stop" };

/** Requête complète : `Command` aplatie avec l'`id` choisi par le client. */
export type Request = { id: number } & Command;

/** Réponse à une requête (`Reply`, tag `kind`). */
export type Reply =
  | { kind: "tree"; agents: AgentStatus[] }
  | { kind: "agent"; agent: AgentStatus }
  | { kind: "done"; affected: AgentId[] }
  | { kind: "error"; message: string };

/** Événement poussé par le daemon (`DaemonEvent`, tag `type`). */
export type DaemonEvent =
  | { type: "message"; agent: AgentId; text: string }
  | { type: "tool"; agent: AgentId; name: string }
  | {
      type: "turn_completed";
      agent: AgentId;
      outcome: TurnOutcome;
      usage: Usage | null;
    }
  /** Le process de l'agent s'est terminé (parking, stop ou panne). */
  | { type: "exited"; agent: AgentId; code: number | null }
  /** L'agent a changé dans le registre ou en exécution. */
  | { type: "agent_updated"; agent: AgentStatus };

/** Message du daemon (`ServerMessage`, tag `type`). */
export type ServerMessage =
  | { type: "hello"; protocol: number }
  | { type: "reply"; id: number; reply: Reply }
  | { type: "event"; event: DaemonEvent };

/** Sérialise une requête : un objet plat, comme `serde(flatten)` côté daemon. */
export function encodeRequest(id: number, command: Command): string {
  const request: Request = { id, ...command };
  return JSON.stringify(request);
}

/** Message reçu illisible ou hors protocole. */
export class ProtocolError extends Error {}

/**
 * Lit une trame du daemon. Les étiquettes (`type`, `kind`, `status`) et les champs dont
 * la GUI dépend sont vérifiés ; une étiquette inconnue lève une `ProtocolError`, que
 * l'appelant ignore plutôt que de corrompre son état.
 */
export function parseServerMessage(frame: string): ServerMessage {
  let value: unknown;
  try {
    value = JSON.parse(frame);
  } catch {
    throw new ProtocolError("trame qui n'est pas du JSON");
  }
  const message = object(value, "message");
  switch (message.type) {
    case "hello":
      number(message.protocol, "hello.protocol");
      break;
    case "reply":
      number(message.id, "reply.id");
      checkReply(object(message.reply, "reply.reply"));
      break;
    case "event":
      checkEvent(object(message.event, "event.event"));
      break;
    default:
      throw new ProtocolError(`type de message inconnu : ${String(message.type)}`);
  }
  return message as unknown as ServerMessage;
}

function checkReply(reply: Record<string, unknown>) {
  switch (reply.kind) {
    case "tree":
      array(reply.agents, "reply.agents").forEach((agent) => checkAgent(agent));
      break;
    case "agent":
      checkAgent(reply.agent);
      break;
    case "done":
      array(reply.affected, "reply.affected");
      break;
    case "error":
      string(reply.message, "reply.message");
      break;
    default:
      throw new ProtocolError(`réponse inconnue : ${String(reply.kind)}`);
  }
}

function checkEvent(event: Record<string, unknown>) {
  if (event.type === "agent_updated") {
    checkAgent(event.agent);
    return;
  }
  string(event.agent, "event.agent");
  switch (event.type) {
    case "message":
      string(event.text, "event.text");
      break;
    case "tool":
      string(event.name, "event.name");
      break;
    case "turn_completed": {
      const outcome = object(event.outcome, "event.outcome");
      if (!["completed", "interrupted", "failed"].includes(outcome.status as string)) {
        throw new ProtocolError(`issue de tour inconnue : ${String(outcome.status)}`);
      }
      break;
    }
    case "exited":
      break;
    default:
      throw new ProtocolError(`événement inconnu : ${String(event.type)}`);
  }
}

function checkAgent(value: unknown) {
  const agent = object(value, "agent");
  string(agent.id, "agent.id");
  if (!["assistant", "agent", "worker"].includes(agent.level as string)) {
    throw new ProtocolError(`niveau inconnu : ${String(agent.level)}`);
  }
  if (typeof agent.running !== "boolean" || typeof agent.busy !== "boolean") {
    throw new ProtocolError("agent sans état d'exécution (running, busy)");
  }
}

function object(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ProtocolError(`${what} : objet attendu`);
  }
  return value as Record<string, unknown>;
}

function array(value: unknown, what: string): unknown[] {
  if (!Array.isArray(value)) throw new ProtocolError(`${what} : liste attendue`);
  return value;
}

function string(value: unknown, what: string) {
  if (typeof value !== "string") throw new ProtocolError(`${what} : texte attendu`);
}

function number(value: unknown, what: string) {
  if (typeof value !== "number") throw new ProtocolError(`${what} : nombre attendu`);
}
