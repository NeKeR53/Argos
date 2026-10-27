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
  /**
   * Profil de la politique de modèles dont viennent moteur et modèle (D22 : `code`,
   * `standard`, `léger`…). Absent pour l'Assistant, choisi par l'utilisateur.
   */
  profile: string | null;
  /**
   * Nœud où tourne sa session (D32) ; `null` pour le nœud du hub. Un Worker tourne sur le
   * nœud de son Agent.
   */
  node: string | null;
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

/**
 * Consommation d'un tour. `input_tokens` inclut les tokens servis par le cache : c'est la
 * taille du contexte envoyé au modèle pour ce tour.
 */
export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cost_usd: number | null;
  /** Taille de la fenêtre de contexte du modèle, si le moteur la donne. */
  context_window: number | null;
}

/** État d'une tâche du board d'un Agent (D17). */
export type TaskState = "pending" | "in_progress" | "blocked" | "done" | "cancelled" | "interrupted";
export type MissionState = "open" | "done" | "cancelled";

/** Demande de haut niveau confiée à un Agent par l'Assistant (D19). */
export interface Mission {
  id: string;
  agent: AgentId;
  text: string;
  state: MissionState;
  created_at: number;
}

/** Unité de travail du board d'un Agent, exécutée par un Worker. */
export interface Task {
  id: string;
  agent: AgentId;
  mission: string | null;
  title: string;
  detail: string | null;
  state: TaskState;
  /** Le Worker qui s'en charge ou s'en est chargé. */
  worker: AgentId | null;
  /** Dernier compte rendu du Worker. */
  result: string | null;
  created_at: number;
  updated_at: number;
}

/**
 * Le board d'un Agent, dans l'ordre de priorité : missions ouvertes, puis tâches de ces
 * missions et tâches hors mission non terminées.
 */
export interface Board {
  agent: AgentId;
  missions: Mission[];
  tasks: Task[];
}

/** Nature d'une surface (D26) : vivante et bidirectionnelle, ou snapshot autonome. */
export type SurfaceKind = "cockpit" | "artifact";

/** Une page HTML produite par un agent et hébergée par l'app (D26–D28). */
export interface SurfaceMeta {
  id: string;
  agent: AgentId;
  kind: SurfaceKind;
  title: string;
  /** Incrémentée à chaque nouveau code : la page se recharge. */
  version: number;
  created_at: number;
  updated_at: number;
}

/** Une surface avec son code (`SurfaceDoc` : la meta aplatie plus `html`). */
export interface SurfaceDoc extends SurfaceMeta {
  html: string;
}

/** Valeur JSON quelconque (`serde_json::Value`). */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/**
 * Nature d'un déclencheur (D29–D30) : intervalle (`spec` en secondes), cron (`spec` :
 * expression à 5 champs), interrupteur, dossier surveillé (`spec` : chemin), webhook.
 */
export type TriggerKind = "every" | "cron" | "switch" | "watch" | "webhook";

/** Source d'événements qui stimule un agent : sa consigne est remise à la cible. */
export interface Trigger {
  id: string;
  /** L'agent qui l'a créé. */
  owner: AgentId;
  /** L'agent qui reçoit les événements. */
  target: AgentId;
  name: string;
  kind: TriggerKind;
  /** Paramètre selon la nature ; vide pour un interrupteur ou un webhook. */
  spec: string;
  /** Consigne remise à l'agent à chaque déclenchement. */
  prompt: string;
  /** Secret d'un webhook, à mettre dans son URL. */
  secret: string | null;
  enabled: boolean;
  created_at: number;
  /**
   * Dernière (ré)activation : un périodique compte ses échéances à partir d'ici ou de son
   * dernier déclenchement, le plus récent des deux.
   */
  armed_at: number;
  last_fired: number | null;
}

/** Ce que l'utilisateur peut demander (`Command`, tag `command`). */
export type Command =
  | { command: "tree" }
  | { command: "inspect"; agent: AgentId }
  /** Le board d'un Agent (lecture seule ; erreur pour un autre niveau). */
  | { command: "board"; agent: AgentId }
  | { command: "send"; agent: AgentId; text: string }
  | { command: "interrupt"; agent: AgentId }
  | { command: "park"; agent: AgentId }
  | { command: "stop"; agent: AgentId }
  | { command: "retire"; agent: AgentId }
  | { command: "panic_stop" }
  /** Les surfaces d'un agent (D26). */
  | { command: "surfaces"; agent: AgentId }
  /** Une surface avec son code, pour l'afficher. */
  | { command: "surface"; surface: string }
  /** Texte envoyé depuis une surface (`argos.send`) : remis à son agent, préfixé. */
  | { command: "surface_send"; surface: string; text: string }
  /** Événement structuré émis par une surface (`argos.emit`) : remis à son agent. */
  | { command: "surface_emit"; surface: string; name: string; payload: Json }
  /** Appel direct d'une fonction publiée par le propriétaire de la surface. */
  | { command: "surface_call"; surface: string; function: string; arguments: Json }
  /** L'utilisateur ferme définitivement une surface. */
  | { command: "surface_delete"; surface: string }
  /** Les déclencheurs dont un agent est propriétaire ou cible (D29). */
  | { command: "triggers"; agent: AgentId }
  /** Actionne un déclencheur (l'interrupteur, ou n'importe lequel pour l'essayer). */
  | { command: "fire_trigger"; trigger: string }
  /** Active ou suspend un déclencheur. */
  | { command: "set_trigger"; trigger: string; enabled: boolean }
  | { command: "delete_trigger"; trigger: string }
  /** Les nœuds déclarés et l'état de leur connexion (D32). */
  | { command: "nodes" }
  /** Les profils de la politique de modèles et leurs modèles. */
  | { command: "models" }
  /** Override de l'utilisateur : nouveau moteur et modèle d'un agent (session parkée). */
  | { command: "set_model"; agent: AgentId; engine: EngineKind; model: string };

/** Un profil de la politique de modèles (`ProfileInfo`) : sa chaîne, premier choix d'abord. */
export interface ProfileInfo {
  name: string;
  description: string;
  models: { engine: EngineKind; model: string }[];
}

/** Un nœud déclaré dans la configuration du hub (`NodeStatus`). */
export interface NodeStatus {
  name: string;
  connected: boolean;
  /** Dernière erreur de connexion, tant que le nœud est injoignable. */
  error: string | null;
}

/** Requête complète : `Command` aplatie avec l'`id` choisi par le client. */
export type Request = { id: number } & Command;

/** Réponse à une requête (`Reply`, tag `kind`). */
export type Reply =
  | { kind: "tree"; agents: AgentStatus[] }
  | { kind: "agent"; agent: AgentStatus }
  | { kind: "board"; board: Board }
  | { kind: "surfaces"; surfaces: SurfaceMeta[] }
  | { kind: "surface"; surface: SurfaceDoc }
  | { kind: "function_result"; result: Json }
  | { kind: "triggers"; triggers: Trigger[] }
  | { kind: "nodes"; nodes: NodeStatus[] }
  | { kind: "models"; profiles: ProfileInfo[] }
  | { kind: "done"; affected: AgentId[] }
  | { kind: "error"; message: string };

/** Expéditeur d'un message remis à un agent (`Origin`, tag `kind`). */
export type Origin = { kind: "user" } | { kind: "agent"; agent: AgentId } | { kind: "argos" };

/** Événement poussé par le daemon (`DaemonEvent`, tag `type`). */
export type DaemonEvent =
  /**
   * Ce que l'agent vient de recevoir : message de l'utilisateur, mission de l'Assistant,
   * rapport d'un enfant, question d'un pair, consigne d'Argos. Publié pour chaque message
   * remis, avec son expéditeur.
   */
  | { type: "input"; agent: AgentId; from: Origin; text: string }
  /**
   * Fragment du message que l'agent est en train d'écrire (D37) ; le message complet
   * suit avec `message`.
   */
  | { type: "message_delta"; agent: AgentId; text: string }
  /** Ce que l'agent a dit. */
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
  | { type: "agent_updated"; agent: AgentStatus }
  /** Information courte du daemon pour l'utilisateur (repli de modèle, plafond…). */
  | { type: "notice"; agent: AgentId | null; text: string }
  /** Une surface a été créée, mise à jour (nouvelle `version`) ou supprimée. */
  | { type: "surface_updated"; surface: SurfaceMeta; deleted: boolean }
  /** Données poussées par l'agent vers son cockpit (`argos.on('data')`). */
  | { type: "surface_data"; surface: string; data: Json };

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
    case "board": {
      const board = object(reply.board, "reply.board");
      string(board.agent, "board.agent");
      array(board.missions, "board.missions").forEach((mission) => string(object(mission, "mission").id, "mission.id"));
      array(board.tasks, "board.tasks").forEach((value) => {
        const task = object(value, "task");
        string(task.id, "task.id");
        if (!TASK_STATES.includes(task.state as TaskState)) {
          throw new ProtocolError(`état de tâche inconnu : ${String(task.state)}`);
        }
      });
      break;
    }
    case "surfaces":
      array(reply.surfaces, "reply.surfaces").forEach((surface) => checkSurface(surface));
      break;
    case "surface":
      string(checkSurface(reply.surface).html, "surface.html");
      break;
    case "function_result":
      if (!("result" in reply)) throw new ProtocolError("function_result sans result");
      break;
    case "nodes":
      array(reply.nodes, "reply.nodes").forEach((value) => {
        const node = object(value, "node");
        string(node.name, "node.name");
        if (typeof node.connected !== "boolean") throw new ProtocolError("node.connected : booléen attendu");
      });
      break;
    case "triggers":
      array(reply.triggers, "reply.triggers").forEach((value) => {
        const trigger = object(value, "trigger");
        string(trigger.id, "trigger.id");
        string(trigger.target, "trigger.target");
        if (!TRIGGER_KINDS.includes(trigger.kind as TriggerKind)) {
          throw new ProtocolError(`source de déclencheur inconnue : ${String(trigger.kind)}`);
        }
        if (typeof trigger.enabled !== "boolean") throw new ProtocolError("trigger.enabled : booléen attendu");
      });
      break;
    case "models":
      array(reply.profiles, "reply.profiles").forEach((value) => {
        const profile = object(value, "profile");
        string(profile.name, "profile.name");
        string(profile.description, "profile.description");
        array(profile.models, "profile.models").forEach((choice) => {
          const model = object(choice, "model");
          string(model.model, "model.model");
          if (model.engine !== "claude" && model.engine !== "codex") {
            throw new ProtocolError(`moteur inconnu : ${String(model.engine)}`);
          }
        });
      });
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
  if (event.type === "notice") {
    if (event.agent !== null) string(event.agent, "event.agent");
    string(event.text, "event.text");
    return;
  }
  if (event.type === "surface_updated") {
    checkSurface(event.surface);
    if (typeof event.deleted !== "boolean") throw new ProtocolError("surface_updated.deleted : booléen attendu");
    return;
  }
  if (event.type === "surface_data") {
    string(event.surface, "event.surface");
    if (!("data" in event)) throw new ProtocolError("surface_data sans data");
    return;
  }
  string(event.agent, "event.agent");
  switch (event.type) {
    case "input": {
      string(event.text, "event.text");
      const from = object(event.from, "event.from");
      if (!["user", "agent", "argos"].includes(from.kind as string)) {
        throw new ProtocolError(`expéditeur inconnu : ${String(from.kind)}`);
      }
      if (from.kind === "agent") string(from.agent, "event.from.agent");
      break;
    }
    case "message_delta":
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

const TASK_STATES: TaskState[] = ["pending", "in_progress", "blocked", "done", "cancelled", "interrupted"];

const TRIGGER_KINDS: TriggerKind[] = ["every", "cron", "switch", "watch", "webhook"];

function checkSurface(value: unknown): Record<string, unknown> {
  const surface = object(value, "surface");
  string(surface.id, "surface.id");
  string(surface.agent, "surface.agent");
  string(surface.title, "surface.title");
  number(surface.version, "surface.version");
  if (surface.kind !== "cockpit" && surface.kind !== "artifact") {
    throw new ProtocolError(`nature de surface inconnue : ${String(surface.kind)}`);
  }
  return surface;
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
