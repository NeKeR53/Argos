import { describe, expect, it } from "vitest";
import type { ConnectionHandlers, Endpoint } from "./connection";
import { MeshConnection, type LinkFactory, type NodeLink } from "./mesh";
import type { AgentStatus, Command, Reply, ServerMessage } from "./protocol";

/** Un faux lien : ouvert à la demande, il note ce qu'on lui envoie. */
class FakeLink implements NodeLink {
  open = false;
  sent: Command[] = [];
  constructor(
    readonly name: string,
    readonly handlers: ConnectionHandlers,
    private readonly ids: () => number,
  ) {}
  start() {}
  stop() {}
  send(command: Command) {
    if (!this.open) return null;
    const id = this.ids();
    this.sent.push(command);
    this.handlers.onRequest(id, command);
    return id;
  }
  request(command: Command): Promise<Reply> {
    this.send(command);
    return Promise.resolve({ kind: "done", affected: [] });
  }
  connect(node: string) {
    this.open = true;
    this.handlers.onConnection({ status: "connected" });
    this.handlers.onMessage({ type: "hello", protocol: 1, node });
  }
  reply(reply: Reply) {
    this.handlers.onMessage({ type: "reply", id: 99, reply });
  }
}

function agent(id: string, node: string | null, level: AgentStatus["level"] = "agent"): AgentStatus {
  return {
    id,
    level,
    parent: level === "assistant" ? null : "a0",
    engine: "claude",
    model: null,
    profile: null,
    node,
    workdir: `/tmp/${id}`,
    charter: level === "agent" ? { name: id, scope: "essais" } : null,
    state: "parked",
    session_id: null,
    created_at: 1,
    running: false,
    busy: false,
  };
}

function mesh() {
  const links = new Map<string, FakeLink>();
  const factory: LinkFactory = (endpoint, handlers, ids) => {
    const link = new FakeLink(endpoint === local ? "local" : "pi", handlers, ids);
    links.set(link.name, link);
    return link;
  };
  const local: Endpoint = { resolve: async () => "ws://local" };
  const received: ServerMessage[] = [];
  const connection = new MeshConnection(
    local,
    [{ name: "pi", endpoint: { resolve: async () => "ws://pi" } }],
    {
      onMessage: (message) => received.push(message),
      onConnection: () => {},
      onRequest: () => {},
    },
    factory,
  );
  connection.start();
  return { connection, links, received };
}

function lastTree(received: ServerMessage[]): AgentStatus[] {
  const trees = received.flatMap((message) =>
    message.type === "reply" && message.reply.kind === "tree" ? [message.reply.agents] : [],
  );
  return trees[trees.length - 1] ?? [];
}

describe("maillage", () => {
  it("assemble les agents détenus par chaque nœud et ignore les copies d'annuaire", () => {
    const { links, received } = mesh();
    links.get("local")!.connect("mac");
    links.get("pi")!.connect("pi");
    links.get("local")!.reply({ kind: "tree", agents: [agent("a0", null, "assistant"), agent("maison", "pi")] });
    links.get("pi")!.reply({ kind: "tree", agents: [agent("a0", "mac", "assistant"), agent("maison", null)] });
    const tree = lastTree(received);
    expect(tree.map((a) => [a.id, a.node])).toEqual([
      ["a0", null],
      ["maison", "pi"],
    ]);
  });

  it("envoie une commande au nœud qui détient l'agent, et l'arbre partout", () => {
    const { connection, links } = mesh();
    const [local, pi] = [links.get("local")!, links.get("pi")!];
    local.connect("mac");
    pi.connect("pi");
    pi.reply({ kind: "tree", agents: [agent("maison", null)] });
    local.reply({ kind: "tree", agents: [agent("a0", null, "assistant")] });
    connection.send({ command: "send", agent: "maison", text: "salut" });
    connection.send({ command: "send", agent: "a0", text: "bonjour" });
    connection.send({ command: "tree" });
    expect(pi.sent.map((c) => c.command)).toEqual(["send", "tree"]);
    expect(local.sent.map((c) => c.command)).toEqual(["send", "tree"]);
  });

  it("retire de la vue les agents d'un nœud perdu", () => {
    const { links, received } = mesh();
    const [local, pi] = [links.get("local")!, links.get("pi")!];
    local.connect("mac");
    pi.connect("pi");
    pi.reply({ kind: "tree", agents: [agent("maison", null)] });
    local.reply({ kind: "tree", agents: [agent("a0", null, "assistant")] });
    expect(lastTree(received)).toHaveLength(2);
    pi.open = false;
    pi.handlers.onConnection({ status: "reconnecting", attempt: 1, retryInMs: 500, reason: "perdu" });
    expect(lastTree(received).map((a) => a.id)).toEqual(["a0"]);
  });

  it("ne garde que l'agent mis à jour par son détenteur", () => {
    const { links, received } = mesh();
    const [local, pi] = [links.get("local")!, links.get("pi")!];
    local.connect("mac");
    pi.connect("pi");
    local.handlers.onMessage({ type: "event", event: { type: "agent_updated", agent: agent("maison", "pi") } });
    pi.handlers.onMessage({ type: "event", event: { type: "agent_updated", agent: agent("maison", null) } });
    const updates = received.flatMap((m) =>
      m.type === "event" && m.event.type === "agent_updated" ? [m.event.agent] : [],
    );
    expect(updates.map((a) => a.node)).toEqual(["pi"]);
  });
});
