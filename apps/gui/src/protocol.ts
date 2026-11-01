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

/** Unité de travail du board, exécutée par l'Agent ou un Worker. */
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
  /** L'agent qui l'a déployée pour `agent`, s'il en est un autre. */
  builder?: AgentId;
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

/** Fichier joint à un message (`Attachment`) ; `data` est le contenu en base64. */
export interface Attachment {
  name: string;
  media_type: string;
  data: string;
}

export type EscalationState = "open" | "answered" | "dismissed";

/**
 * Question d'un Agent à l'utilisateur (D12, D35), gardée jusqu'à sa réponse. La console
 * la montre avec ses choix ; la réponse part directement à l'Agent.
 */
export interface Escalation {
  id: string;
  agent: AgentId;
  question: string;
  /** Réponses courtes proposées en boutons ; vide pour une réponse libre seulement. */
  options: string[];
  state: EscalationState;
  answer: string | null;
  created_at: number;
  resolved_at: number | null;
}

/** Ce que l'utilisateur peut demander (`Command`, tag `command`). */
export type Command =
  | { command: "tree" }
  | { command: "inspect"; agent: AgentId }
  | { command: "capabilities"; agent: AgentId }
  /** Le board d'un Agent (lecture seule ; erreur pour un autre niveau). */
  | { command: "board"; agent: AgentId }
  /** Passe une mission en tête du board ; préempte les Workers moins prioritaires si actif (D14). */
  | { command: "prioritize_mission"; mission: string }
  /** Les escalades ouvertes, puis les dernières résolues. */
  | { command: "escalations" }
  /** Répond à une escalade : la réponse part directement à l'Agent qui attend. */
  | { command: "answer_escalation"; escalation: string; answer: string }
  /** Écarte une escalade ; l'Agent est prévenu de ne plus l'attendre. */
  | { command: "dismiss_escalation"; escalation: string }
  /**
   * Message à un agent, avec d'éventuelles pièces jointes : images en entrée native du
   * moteur, fichiers texte recopiés dans le message, autres fichiers enregistrés sur le hub.
   */
  | { command: "send"; agent: AgentId; text: string; attachments?: Attachment[] }
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
  /** Les canaux de discussion configurés et leur état (D45). */
  | { command: "channels" }
  /** Les profils de la politique de modèles et leurs modèles. */
  | { command: "models" }
  /** Rattache un Agent à un dossier sur son nœud ; ouvre une nouvelle session. */
  | { command: "set_workspace"; agent: AgentId; workdir: string }
  /** Override de l'utilisateur : nouveau moteur et modèle d'un agent (session parkée). */
  | { command: "set_model"; agent: AgentId; engine: EngineKind; model: string }
  /** Les synchros de workspace et leur état (D44). */
  | { command: "syncs" }
  /** Copie locale, sur le hub, du workspace d'un Agent placé sur un nœud. */
  | { command: "open_sync"; agent: AgentId; local: string; excludes: string[] }
  /** Ferme une synchro ; les deux copies restent en place. */
  | { command: "close_sync"; sync: string }
  /** Cherche une version plus récente d'Argos (D46) ; `refresh` ignore le cache d'une heure. */
  | { command: "check_update"; refresh?: boolean }
  /** Installe la dernière version puis relance le daemon ; l'avancement se lit avec `check_update`. */
  | { command: "apply_update" };

export type UpdateStep = "downloading" | "installing" | "restarting" | "done" | "failed";
const UPDATE_STEPS: UpdateStep[] = ["downloading", "installing", "restarting", "done", "failed"];

/** Mises à jour d'Argos vues par le daemon (`UpdateStatus`, D46). */
export interface UpdateStatus {
  /** Version du daemon qui répond. */
  current: string;
  /** Version plus récente publiée pour cette plateforme. */
  available: { version: string; notes: string; url: string } | null;
  /** Pourquoi la recherche a échoué. */
  error: string | null;
  /** La dernière installation lancée : son étape, puis son issue. */
  progress: { version: string; step: UpdateStep; message: string; at: number } | null;
}

export type SyncState = "idle" | "syncing" | "offline" | "error";

/** Une synchro de workspace (`SyncStatus`, D44). */
export interface SyncStatus {
  id: string;
  agent: AgentId;
  node: string;
  /** Copie locale sur le hub. */
  local: string;
  state: SyncState;
  error: string | null;
  /** Fichiers suivis. */
  files: number;
  /** Versions du hub mises de côté lors des conflits (`<nom>.conflit-hub`). */
  conflicts: string[];
  /** Fichiers trop gros pour voyager. */
  held: string[];
  /** Dernière passe réussie, en secondes depuis l'époque. */
  synced_at: number | null;
}

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

/** Ce qu'un type de canal sait faire (`ChannelCapabilities`). */
export interface ChannelCapabilities {
  text: boolean;
  voice_in: boolean;
  voice_out: boolean;
  buttons: boolean;
  attachments: boolean;
}

/** Un canal de discussion configuré, `[channels.<id>]` (`ChannelStatus`, D45). */
export interface ChannelStatus {
  id: string;
  /** Son type : `telegram`… */
  type: string;
  capabilities: ChannelCapabilities;
  /** Démarré et prêt à servir. */
  active: boolean;
  /** Pourquoi il ne sert pas : démarrage en échec, interlocuteur pas encore configuré… */
  unavailable?: string;
  /** Le daemon y annonce lui-même les escalades. */
  escalations: boolean;
  max_per_hour: number;
  /** Envois de l'Assistant dans l'heure glissante. */
  sent_last_hour: number;
}

/** Requête complète : `Command` aplatie avec l'`id` choisi par le client. */
export type Request = { id: number } & Command;

export interface Capabilities {
  skills: string[];
  mcp: ({ name: string; type: "http"; url: string } | { name: string; type: "stdio"; command: string; args: string[] })[];
}

/** Réponse à une requête (`Reply`, tag `kind`). */
export type Reply =
  | { kind: "tree"; agents: AgentStatus[] }
  | { kind: "agent"; agent: AgentStatus }
  | { kind: "capabilities"; config: Capabilities; argos_enabled: boolean }
  | { kind: "board"; board: Board }
  | { kind: "surfaces"; surfaces: SurfaceMeta[] }
  | { kind: "surface"; surface: SurfaceDoc }
  | { kind: "function_result"; result: Json }
  | { kind: "triggers"; triggers: Trigger[] }
  | { kind: "escalations"; escalations: Escalation[] }
  | { kind: "nodes"; nodes: NodeStatus[] }
  | { kind: "channels"; channels: ChannelStatus[] }
  | { kind: "syncs"; syncs: SyncStatus[] }
  | { kind: "sync"; sync: SyncStatus }
  | { kind: "models"; profiles: ProfileInfo[]; models: { engine: EngineKind; model: string }[] }
  | { kind: "update"; update: UpdateStatus }
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
  | { type: "tool_detail"; agent: AgentId; id: string; name: string; input: string | null; output: string | null; finished: boolean }
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
  /** Une synchro de workspace a changé d'état, ou a été fermée. */
  | { type: "sync_updated"; sync: SyncStatus; closed: boolean }
  /** Une escalade a été ouverte, répondue ou écartée. */
  | { type: "escalation_updated"; escalation: Escalation }
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
    case "capabilities": {
      if (typeof reply.argos_enabled !== "boolean") throw new ProtocolError("argos_enabled : booléen attendu");
      const config = object(reply.config, "reply.config");
      array(config.skills, "config.skills").forEach((path) => string(path, "skill"));
      array(config.mcp, "config.mcp").forEach((value) => {
        const server = object(value, "mcp");
        string(server.name, "mcp.name");
        if (server.type === "http") string(server.url, "mcp.url");
        else if (server.type === "stdio") { string(server.command, "mcp.command"); array(server.args, "mcp.args").forEach((arg) => string(arg, "mcp.arg")); }
        else throw new ProtocolError("transport MCP inconnu");
      });
      break;
    }
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
    case "channels":
      array(reply.channels, "reply.channels").forEach((value) => {
        const channel = object(value, "channel");
        string(channel.id, "channel.id");
        string(channel.type, "channel.type");
        object(channel.capabilities, "channel.capabilities");
        if (typeof channel.active !== "boolean") throw new ProtocolError("channel.active : booléen attendu");
      });
      break;
    case "syncs":
      array(reply.syncs, "reply.syncs").forEach((sync) => checkSync(sync));
      break;
    case "sync":
      checkSync(reply.sync);
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
    case "escalations":
      array(reply.escalations, "reply.escalations").forEach((escalation) => checkEscalation(escalation));
      break;
    case "models":
      array(reply.models, "reply.models").forEach((choice) => {
        const model = object(choice, "model");
        string(model.model, "model.model");
        if (model.engine !== "claude" && model.engine !== "codex") throw new ProtocolError(`moteur inconnu : ${String(model.engine)}`);
      });
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
    case "update": {
      const update = object(reply.update, "reply.update");
      string(update.current, "update.current");
      if (update.available !== null) string(object(update.available, "update.available").version, "available.version");
      if (update.progress !== null) {
        const progress = object(update.progress, "update.progress");
        if (!UPDATE_STEPS.includes(progress.step as UpdateStep)) {
          throw new ProtocolError(`étape de mise à jour inconnue : ${String(progress.step)}`);
        }
      }
      break;
    }
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
  if (event.type === "escalation_updated") {
    checkEscalation(event.escalation);
    return;
  }
  if (event.type === "sync_updated") {
    checkSync(event.sync);
    if (typeof event.closed !== "boolean") throw new ProtocolError("sync_updated.closed : booléen attendu");
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
    case "tool_detail":
      string(event.id, "event.id"); string(event.name, "event.name");
      if (event.input !== null) string(event.input, "event.input");
      if (event.output !== null) string(event.output, "event.output");
      if (typeof event.finished !== "boolean") throw new ProtocolError("tool_detail.finished : booléen attendu");
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

const SYNC_STATES: SyncState[] = ["idle", "syncing", "offline", "error"];

function checkSync(value: unknown) {
  const sync = object(value, "sync");
  string(sync.id, "sync.id");
  string(sync.agent, "sync.agent");
  string(sync.local, "sync.local");
  if (!SYNC_STATES.includes(sync.state as SyncState)) throw new ProtocolError(`état de synchro inconnu : ${String(sync.state)}`);
  array(sync.conflicts, "sync.conflicts").forEach((path) => string(path, "sync.conflict"));
}

const TASK_STATES: TaskState[] = ["pending", "in_progress", "blocked", "done", "cancelled", "interrupted"];

const TRIGGER_KINDS: TriggerKind[] = ["every", "cron", "switch", "watch", "webhook"];

const ESCALATION_STATES: EscalationState[] = ["open", "answered", "dismissed"];

function checkEscalation(value: unknown) {
  const escalation = object(value, "escalation");
  string(escalation.id, "escalation.id");
  string(escalation.agent, "escalation.agent");
  string(escalation.question, "escalation.question");
  array(escalation.options, "escalation.options").forEach((option) => string(option, "escalation.option"));
  if (!ESCALATION_STATES.includes(escalation.state as EscalationState)) {
    throw new ProtocolError(`état d'escalade inconnu : ${String(escalation.state)}`);
  }
}

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
