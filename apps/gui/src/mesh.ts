// L'application reliée à tout le maillage (D48) : une connexion par nœud, et une vue
// assemblée. Chaque nœud ne donne que ses propres agents (les copies d'annuaire qu'il
// garde des autres sont ignorées) ; une commande part au nœud qui détient ce qu'elle vise.
// Un nœud injoignable disparaît de la vue sans gêner les autres.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { DaemonConnection, type ConnectionHandlers, type Endpoint, type Probe } from "./connection";
import type { AgentId, AgentStatus, ChannelStatus, Command, Escalation, NodeStatus, Reply, ServerMessage } from "./protocol";
import type { Connection } from "./state";

/** Un autre nœud que l'application joint elle-même. */
export interface MeshNode {
  name: string;
  endpoint: Endpoint;
}

/**
 * Les autres nœuds : ceux de `[peers]` dans la config locale (app Tauri) ; dans un
 * navigateur (`npm run dev`), `?peer.<nom>=<URL WebSocket encodée>` dans l'adresse.
 */
export async function meshNodesFromEnvironment(search: string = window.location.search): Promise<MeshNode[]> {
  if (!isTauri()) {
    return [...new URLSearchParams(search).entries()]
      .filter(([key]) => key.startsWith("peer."))
      .map(([key, url]) => ({ name: key.slice("peer.".length), endpoint: { resolve: async () => url } }));
  }
  const names = await invoke<string[]>("mesh_nodes").catch(() => [] as string[]);
  return names.map((name) => ({
    name,
    endpoint: {
      resolve: () => invoke<string>("mesh_endpoint", { name }),
      probe: (url) => invoke<Probe>("probe_daemon", { url }),
    },
  }));
}

/** Commandes qui portent sur tout le maillage : envoyées à chaque nœud, réponses fusionnées. */
const EVERYWHERE = new Set<Command["command"]>(["tree", "escalations", "channels", "panic_stop"]);

/** Nom provisoire du nœud local, tant qu'il ne s'est pas présenté. */
const HOME = "";

/** Ce que le maillage attend d'une connexion à un nœud. */
export interface NodeLink {
  readonly open: boolean;
  start(): void;
  stop(): void;
  send(command: Command): number | null;
  request(command: Command): Promise<Reply>;
}

export type LinkFactory = (endpoint: Endpoint, handlers: ConnectionHandlers, ids: () => number) => NodeLink;

const daemonLink: LinkFactory = (endpoint, handlers, ids) => new DaemonConnection(endpoint, handlers, ids);

interface Peer {
  connection: NodeLink;
  state: Connection;
}

export class MeshConnection {
  private readonly peers = new Map<string, Peer>();
  private home = HOME;
  private nextId = 1;
  /** Ce que chaque nœud détient, pour la vue et le routage. */
  private readonly agents = new Map<string, AgentStatus[]>();
  private readonly escalations = new Map<string, Escalation[]>();
  private readonly channels = new Map<string, ChannelStatus[]>();
  private homeNodes: NodeStatus[] = [];
  private readonly owners = {
    agent: new Map<AgentId, string>(),
    surface: new Map<string, string>(),
    trigger: new Map<string, string>(),
    escalation: new Map<string, string>(),
    mission: new Map<string, string>(),
  };

  constructor(
    private readonly local: Endpoint | null,
    private readonly others: MeshNode[],
    private readonly handlers: ConnectionHandlers,
    private readonly link: LinkFactory = daemonLink,
  ) {}

  start() {
    if (this.local) this.add(HOME, this.local);
    for (const node of this.others) this.add(node.name, node.endpoint);
    this.publishConnection();
  }

  stop() {
    for (const peer of this.peers.values()) peer.connection.stop();
    this.peers.clear();
  }

  /** Ajoute un nœud découvert après le démarrage. */
  join(node: MeshNode) {
    if (this.peers.has(node.name) || node.name === this.home) return;
    this.add(node.name, node.endpoint);
  }

  send(command: Command): number | null {
    let sent: number | null = null;
    for (const name of this.route(command)) {
      sent = this.peers.get(name)?.connection.send(command) ?? sent;
    }
    return sent;
  }

  async request(command: Command): Promise<Reply> {
    const targets = this.route(command);
    if (targets.length === 0) throw new Error("Aucun nœud joignable.");
    if (!EVERYWHERE.has(command.command) || targets.length === 1) {
      return this.peers.get(targets[0])!.connection.request(command);
    }
    const replies = await Promise.allSettled(targets.map((name) => this.peers.get(name)!.connection.request(command)));
    const ok = replies.filter((reply): reply is PromiseFulfilledResult<Reply> => reply.status === "fulfilled");
    if (ok.length === 0) throw new Error("Aucun nœud n'a répondu.");
    return this.merged(ok[ok.length - 1].value);
  }

  private add(name: string, endpoint: Endpoint) {
    const handlers: ConnectionHandlers = {
      onMessage: (message) => this.receive(this.nameOf(handlers), message),
      onConnection: (state) => this.connectionChanged(this.nameOf(handlers), state),
      onRequest: (id, command) => this.handlers.onRequest(id, command),
    };
    const connection = this.link(endpoint, handlers, () => this.nextId++);
    const peer: Peer = { connection, state: { status: "connecting" } };
    this.peers.set(name, peer);
    this.handlerNames.set(handlers, name);
    connection.start();
  }

  /** Le nom courant d'un nœud ; le nœud local change de nom en se présentant. */
  private readonly handlerNames = new Map<ConnectionHandlers, string>();
  private nameOf(handlers: ConnectionHandlers): string {
    return this.handlerNames.get(handlers) ?? HOME;
  }

  /** Où envoyer une commande. */
  private route(command: Command): string[] {
    const open = [...this.peers.entries()].filter(([, peer]) => peer.connection.open).map(([name]) => name);
    if (EVERYWHERE.has(command.command)) return open;
    const owner = this.ownerOf(command);
    if (owner !== undefined && open.includes(owner)) return [owner];
    if (open.includes(this.home)) return [this.home];
    return open.slice(0, 1);
  }

  private ownerOf(command: Command): string | undefined {
    if ("agent" in command && typeof command.agent === "string") return this.owners.agent.get(command.agent);
    if ("surface" in command && typeof command.surface === "string") return this.owners.surface.get(command.surface);
    if ("trigger" in command && typeof command.trigger === "string") return this.owners.trigger.get(command.trigger);
    if ("escalation" in command && typeof command.escalation === "string") return this.owners.escalation.get(command.escalation);
    if ("mission" in command && typeof command.mission === "string") return this.owners.mission.get(command.mission);
    return undefined;
  }

  /** Un agent tel que la vue le montre : `node` vide pour ceux du nœud local. */
  private stamp(name: string, agent: AgentStatus): AgentStatus {
    return name === this.home ? { ...agent, node: null } : { ...agent, node: name };
  }

  private receive(name: string, message: ServerMessage) {
    if (message.type === "hello") {
      if (name === HOME && this.peers.has(HOME)) this.rename(message.node ?? HOME);
      if (name === HOME || name === this.home) this.handlers.onMessage(message);
      return;
    }
    if (message.type === "reply") {
      this.handlers.onMessage({ ...message, reply: this.absorb(name, message.reply) });
      return;
    }
    const event = message.event;
    switch (event.type) {
      case "agent_updated": {
        // Copie d'annuaire d'un agent détenu ailleurs : son détenteur le dira lui-même.
        if (event.agent.node !== null) return;
        const agent = this.stamp(name, event.agent);
        this.owners.agent.set(agent.id, name);
        const list = (this.agents.get(name) ?? []).filter((known) => known.id !== agent.id);
        this.agents.set(name, [...list, agent]);
        this.handlers.onMessage({ type: "event", event: { ...event, agent } });
        return;
      }
      case "surface_updated":
        this.owners.surface.set(event.surface.id, name);
        break;
      case "escalation_updated":
        this.owners.escalation.set(event.escalation.id, name);
        break;
    }
    this.handlers.onMessage(message);
  }

  /** Le nœud local s'est présenté sous son nom. */
  private rename(node: string) {
    if (node === HOME || this.home === node) return;
    const peer = this.peers.get(HOME);
    // Déjà joint sous ce nom (déclaré dans [peers]) : la connexion locale suffit.
    const duplicate = this.peers.get(node);
    if (duplicate) {
      duplicate.connection.stop();
      this.peers.delete(node);
    }
    if (peer) {
      this.peers.delete(HOME);
      this.peers.set(node, peer);
      for (const [handlers, name] of this.handlerNames) if (name === HOME) this.handlerNames.set(handlers, node);
    }
    for (const map of [this.agents, this.escalations, this.channels]) {
      const value = map.get(HOME);
      if (value) {
        map.delete(HOME);
        (map as Map<string, unknown>).set(node, value);
      }
    }
    this.home = node;
  }

  /** Retient ce qu'une réponse apprend du nœud, et la rend fusionnée. */
  private absorb(name: string, reply: Reply): Reply {
    switch (reply.kind) {
      case "tree": {
        const own = reply.agents.filter((agent) => agent.node === null).map((agent) => this.stamp(name, agent));
        this.agents.set(name, own);
        for (const agent of own) this.owners.agent.set(agent.id, name);
        return this.merged(reply);
      }
      case "agent":
        return reply.agent.node === null ? { ...reply, agent: this.stamp(name, reply.agent) } : reply;
      case "escalations":
        this.escalations.set(name, reply.escalations);
        for (const escalation of reply.escalations) this.owners.escalation.set(escalation.id, name);
        return this.merged(reply);
      case "channels":
        this.channels.set(name, reply.channels);
        return this.merged(reply);
      case "nodes":
        if (name === this.home) this.homeNodes = reply.nodes;
        return this.merged(reply);
      case "surfaces":
        for (const surface of reply.surfaces) this.owners.surface.set(surface.id, name);
        return reply;
      case "triggers":
        for (const trigger of reply.triggers) this.owners.trigger.set(trigger.id, name);
        return reply;
      case "board":
        for (const mission of reply.board.missions) this.owners.mission.set(mission.id, name);
        return reply;
      default:
        return reply;
    }
  }

  /** La vue de tout le maillage, pour une réponse de ce genre. */
  private merged(reply: Reply): Reply {
    switch (reply.kind) {
      case "tree":
        return { kind: "tree", agents: [...this.agents.values()].flat() };
      case "escalations":
        return { kind: "escalations", escalations: [...this.escalations.values()].flat() };
      case "channels":
        return { kind: "channels", channels: [...this.channels.values()].flat() };
      case "nodes": {
        // Ce que le nœud local sait de ses pairs, complété par les liens de l'application.
        const nodes = new Map(this.homeNodes.map((node) => [node.name, node]));
        for (const [name, peer] of this.peers) {
          if (name === this.home || name === HOME) continue;
          const known = nodes.get(name);
          const connected = peer.connection.open || (known?.connected ?? false);
          nodes.set(name, { name, connected, error: connected ? null : (known?.error ?? null) });
        }
        return { kind: "nodes", nodes: [...nodes.values()] };
      }
      default:
        return reply;
    }
  }

  private connectionChanged(name: string, state: Connection) {
    const peer = this.peers.get(name);
    if (!peer) return;
    const wasOpen = peer.state.status === "connected";
    peer.state = state;
    // Un nœud perdu disparaît de la vue ; il revient avec son arbre à la reconnexion.
    if (wasOpen && state.status !== "connected") {
      this.agents.delete(name);
      this.escalations.delete(name);
      this.channels.delete(name);
      this.handlers.onMessage({ type: "reply", id: 0, reply: this.merged({ kind: "tree", agents: [] }) });
    }
    this.publishConnection();
  }

  /** L'état montré à l'utilisateur : celui du nœud local, sinon le meilleur des autres. */
  private publishConnection() {
    const local = this.peers.get(this.home) ?? this.peers.get(HOME);
    const states = [...this.peers.values()].map((peer) => peer.state);
    if (states.some((state) => state.status === "connected")) {
      this.handlers.onConnection({ status: "connected" });
      return;
    }
    this.handlers.onConnection(local?.state ?? states[0] ?? { status: "connecting" });
  }
}
