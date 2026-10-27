import { describe, expect, it } from "vitest";
import type { AgentStatus, Origin, ServerMessage, Trigger } from "./protocol";
import { parseServerMessage } from "./protocol";
import {
  initialState,
  reduce,
  selectedAgent,
  treeOf,
  THREAD_LIMIT,
  type Action,
  type AppState,
} from "./state";

function agent(id: string, level: AgentStatus["level"], parent: string | null, extra: Partial<AgentStatus> = {}) {
  return {
    id,
    level,
    parent,
    engine: "claude",
    model: null,
    profile: null,
    node: null,
    workdir: `/tmp/${id}`,
    charter: level === "agent" ? { name: id.toUpperCase(), scope: "essais" } : null,
    state: "parked",
    session_id: null,
    created_at: 1,
    running: false,
    busy: false,
    ...extra,
  } satisfies AgentStatus;
}

const assistant = agent("a0", "assistant", null);
const domotique = agent("dom", "agent", "a0");
const worker = agent("w1", "worker", "dom");

/** Passe par le parsing, comme une vraie trame du daemon. */
function server(message: ServerMessage): Action {
  return { type: "server", message: parseServerMessage(JSON.stringify(message)) };
}

function run(...actions: Action[]): AppState {
  return actions.reduce(reduce, initialState);
}

const connected: Action = { type: "connection", connection: { status: "connected" } };
const tree = server({ type: "reply", id: 1, reply: { kind: "tree", agents: [assistant, domotique, worker] } });

describe("arbre", () => {
  it("se construit depuis tree, chacun sous son parent", () => {
    const state = run(connected, tree);
    const roots = treeOf(state);
    expect(roots.map((node) => node.agent.id)).toEqual(["a0"]);
    expect(roots[0].children.map((node) => node.agent.id)).toEqual(["dom"]);
    expect(roots[0].children[0].children.map((node) => node.agent.id)).toEqual(["w1"]);
  });

  it("suit agent_updated : mise à jour, apparition, retrait", () => {
    const busy = { ...assistant, state: "active" as const, running: true, busy: true };
    const compta = agent("compta", "agent", "a0");
    let state = run(
      tree,
      server({ type: "event", event: { type: "agent_updated", agent: busy } }),
      server({ type: "event", event: { type: "agent_updated", agent: compta } }),
    );
    expect(state.agents.a0.busy).toBe(true);
    expect(state.order).toEqual(["a0", "dom", "w1", "compta"]);

    state = reduce(state, { type: "select", agent: "dom" });
    for (const retired of [worker, domotique]) {
      state = reduce(
        state,
        server({ type: "event", event: { type: "agent_updated", agent: { ...retired, state: "retired" } } }),
      );
    }
    expect(state.order).toEqual(["a0", "compta"]);
    // L'agent affiché a disparu : retour à l'Assistant.
    expect(selectedAgent(state)?.id).toBe("a0");
  });

  it("garde visible un agent dont le parent est inconnu", () => {
    const state = run(server({ type: "event", event: { type: "agent_updated", agent: worker } }));
    expect(treeOf(state).map((node) => node.agent.id)).toEqual(["w1"]);
  });

  it("affiche l'Assistant par défaut", () => {
    expect(selectedAgent(run(tree))?.id).toBe("a0");
    expect(selectedAgent(run(tree, { type: "select", agent: "w1" }))?.id).toBe("w1");
  });
});

const user = { kind: "user" } as const;
const input = (agent: string, text: string, from: Origin = user): Action =>
  server({ type: "event", event: { type: "input", agent, from, text } });
const send = (id: number, agent: string, text: string): Action => ({
  type: "requested",
  id,
  command: { command: "send", agent, text },
});
const done = (id: number, agent: string): Action =>
  server({ type: "reply", id, reply: { kind: "done", affected: [agent] } });

/** Le fil sans les numéros de ligne. */
function thread(state: AppState, agent: string) {
  return (state.threads[agent] ?? []).map(({ seq: _, ...entry }) => entry);
}

describe("fil des messages", () => {
  it("assemble message reçu, réponse, outil et fin de tour", () => {
    const usage = { input_tokens: 13, output_tokens: 2, cost_usd: 0.001, context_window: 200000 };
    const state = run(
      connected,
      tree,
      send(2, "a0", "bonjour"),
      input("a0", "bonjour"),
      done(2, "a0"),
      server({ type: "event", event: { type: "tool", agent: "a0", name: "Bash" } }),
      server({ type: "event", event: { type: "message", agent: "a0", text: "reply 1" } }),
      server({ type: "event", event: { type: "turn_completed", agent: "a0", outcome: { status: "completed" }, usage } }),
      server({ type: "event", event: { type: "exited", agent: "a0", code: 0 } }),
    );
    expect(thread(state, "a0")).toEqual([
      {
        kind: "input",
        text: "bonjour",
        from: user,
        console: { request: 2, delivery: "delivered", awaitingEcho: false },
      },
      { kind: "tool", name: "Bash" },
      { kind: "agent", text: "reply 1" },
      { kind: "turn", outcome: { status: "completed" }, usage },
      { kind: "exited", code: 0 },
    ]);
    expect(state.spent.a0).toBeCloseTo(0.001);
    expect(state.pending).toEqual({});
  });

  it("ne duplique pas un message de la console, que l'input arrive avant ou après la réponse", () => {
    const before = run(tree, send(2, "a0", "salut"), input("a0", "salut"), done(2, "a0"));
    const after = run(tree, send(2, "a0", "salut"), done(2, "a0"), input("a0", "salut"));
    for (const state of [before, after]) {
      expect(thread(state, "a0")).toEqual([
        { kind: "input", text: "salut", from: user, console: { request: 2, delivery: "delivered", awaitingEcho: false } },
      ]);
    }
  });

  it("confirme dans l'ordre deux envois identiques", () => {
    const state = run(tree, send(2, "a0", "ok"), send(3, "a0", "ok"), input("a0", "ok"));
    expect(thread(state, "a0").map((entry) => entry.kind === "input" && entry.console)).toEqual([
      { request: 2, delivery: "delivered", awaitingEcho: false },
      { request: 3, delivery: "pending", awaitingEcho: true },
    ]);
  });

  it("ajoute ce que l'agent reçoit d'ailleurs : mission, rapport, autre console", () => {
    const mission = "[Mission m-1a2b3c4d de l'Assistant]\nÉclaire le salon";
    const state = run(tree, input("dom", mission), input("dom", "bonjour"));
    expect(thread(state, "dom")).toEqual([
      { kind: "input", text: mission, from: user, console: null },
      { kind: "input", text: "bonjour", from: user, console: null },
    ]);
  });

  it("garde « non remis » sans attendre d'input après une erreur", () => {
    const state = run(
      tree,
      send(5, "a0", "salut"),
      server({ type: "reply", id: 5, reply: { kind: "error", message: "réveil impossible" } }),
      input("a0", "salut"),
    );
    // L'input tardif (d'une autre console, par exemple) ne relève pas le message refusé.
    expect(thread(state, "a0")).toEqual([
      { kind: "input", text: "salut", from: null, console: { request: 5, delivery: "failed", awaitingEcho: false } },
      { kind: "input", text: "salut", from: user, console: null },
    ]);
  });

  it("cumule le coût des tours et ignore un coût absent", () => {
    const turn = (cost: number | null): Action =>
      server({
        type: "event",
        event: {
          type: "turn_completed",
          agent: "a0",
          outcome: { status: "failed", message: "quota" },
          usage: cost == null ? null : { input_tokens: 1, output_tokens: 1, cost_usd: cost, context_window: null },
        },
      });
    const state = run(turn(0.5), turn(null), turn(0.25));
    expect(state.spent.a0).toBeCloseTo(0.75);
  });

  it("borne la longueur d'un fil", () => {
    const message: Action = server({ type: "event", event: { type: "message", agent: "a0", text: "x" } });
    const state = run(...Array.from({ length: THREAD_LIMIT + 10 }, () => message));
    expect(state.threads.a0).toHaveLength(THREAD_LIMIT);
    expect(state.threads.a0[THREAD_LIMIT - 1].seq).toBe(THREAD_LIMIT + 10);
  });

  it("marque perdus les messages sans réponse quand la connexion tombe", () => {
    const state = run(
      connected,
      { type: "requested", id: 3, command: { command: "send", agent: "a0", text: "tu es là ?" } },
      { type: "connection", connection: { status: "reconnecting", attempt: 1, retryInMs: 500, reason: "connexion perdue" } },
    );
    expect(state.threads.a0[0]).toMatchObject({ kind: "input", console: { delivery: "lost", awaitingEcho: false } });
    expect(state.pending).toEqual({});
  });
});

describe("escalades", () => {
  const escalation = "[Escalade pour l'utilisateur de l'Agent « DOM » (dom)]\nFaut-il couper le chauffage ?";

  it("se signalent quand l'utilisateur regarde un autre agent, jusqu'à consulter l'Assistant", () => {
    let state = run(tree, { type: "select", agent: "dom" }, input("a0", escalation));
    expect(state.escalations).toEqual([
      { id: expect.any(Number), name: "DOM", agent: "dom", text: "Faut-il couper le chauffage ?" },
    ]);
    expect(state.unseenEscalations).toBe(1);

    state = reduce(state, { type: "select", agent: "a0" });
    expect(state.unseenEscalations).toBe(0);
    // Le bandeau reste jusqu'à ce qu'on le ferme ou qu'on ouvre l'Agent.
    state = reduce(state, { type: "dismiss_escalation", escalation: state.escalations[0].id });
    expect(state.escalations).toEqual([]);
  });

  it("sont vues d'emblée si l'Assistant est affiché", () => {
    const state = run(tree, input("a0", escalation));
    expect(state.escalations).toHaveLength(1);
    expect(state.unseenEscalations).toBe(0);
  });

  it("ne viennent que des inputs de l'Assistant", () => {
    expect(run(tree, input("dom", escalation)).escalations).toEqual([]);
    expect(run(tree, input("a0", "[Rapport de l'Agent « DOM » (dom)]\nfini")).escalations).toEqual([]);
  });
});

describe("board", () => {
  const board = {
    agent: "dom",
    missions: [{ id: "m-1", agent: "dom", text: "éclairage", state: "open" as const, created_at: 1 }],
    tasks: [],
  };

  it("garde le dernier board reçu", () => {
    const state = run(tree, server({ type: "reply", id: 9, reply: { kind: "board", board } }));
    expect(state.boards.dom).toEqual(board);
  });

  it("est à relire après un événement de l'Agent ou de l'un de ses Workers", () => {
    const state = run(
      tree,
      input("dom", "bonjour"),
      server({ type: "event", event: { type: "tool", agent: "w1", name: "report_to_agent" } }),
      server({ type: "event", event: { type: "agent_updated", agent: agent("w2", "worker", "dom") } }),
      input("a0", "rien à voir"),
    );
    expect(state.boardRevisions).toEqual({ dom: 3 });
  });
});

describe("erreurs du daemon", () => {
  it("deviennent des avis lisibles, avec la commande en contexte", () => {
    const state = run(
      tree,
      { type: "requested", id: 4, command: { command: "park", agent: "dom" } },
      server({ type: "reply", id: 4, reply: { kind: "error", message: "agent inconnu" } }),
    );
    expect(state.notices).toEqual([{ id: expect.any(Number), tone: "error", text: "Parking de DOM : agent inconnu" }]);
  });

  it("font échouer le message refusé", () => {
    const state = run(
      tree,
      { type: "requested", id: 5, command: { command: "send", agent: "w1", text: "salut" } },
      server({
        type: "reply",
        id: 5,
        reply: { kind: "error", message: "w1 est un Worker : l'utilisateur passe par son Agent" },
      }),
    );
    expect(state.threads.w1[0]).toMatchObject({ console: { delivery: "failed" } });
    expect(state.notices[0].text).toBe("Message à w1 : w1 est un Worker : l'utilisateur passe par son Agent");
  });

  it("restent lisibles pour une requête illisible (id 0)", () => {
    const state = run(server({ type: "reply", id: 0, reply: { kind: "error", message: "requête illisible : x" } }));
    expect(state.notices[0].text).toBe("Le daemon signale : requête illisible : x");
  });

  it("se ferment", () => {
    const state = run(server({ type: "reply", id: 0, reply: { kind: "error", message: "x" } }));
    expect(reduce(state, { type: "dismiss", notice: state.notices[0].id }).notices).toEqual([]);
  });

  it("côtoient les informations du daemon, liées à leur agent", () => {
    const state = run(
      tree,
      server({ type: "event", event: { type: "notice", agent: "dom", text: "quota épuisé : bascule sur gpt-6-luna" } }),
      server({ type: "event", event: { type: "notice", agent: null, text: "plafond atteint" } }),
    );
    expect(state.notices).toEqual([
      { id: expect.any(Number), tone: "info", text: "quota épuisé : bascule sur gpt-6-luna", agent: "dom" },
      { id: expect.any(Number), tone: "info", text: "plafond atteint" },
    ]);
  });

  it("préviennent d'une version de protocole différente", () => {
    expect(run(server({ type: "hello", protocol: 1 })).notices).toEqual([]);
    expect(run(server({ type: "hello", protocol: 2 })).notices[0].tone).toBe("warning");
  });
});

describe("expéditeur", () => {
  it("garde qui a envoyé chaque message reçu", () => {
    const from = { kind: "agent", agent: "a0" } as const;
    const state = run(tree, input("dom", "[Question de l'Agent « Mail » (m1)]\nUn courriel ?", from));
    expect(thread(state, "dom")).toEqual([
      { kind: "input", text: "[Question de l'Agent « Mail » (m1)]\nUn courriel ?", from, console: null },
    ]);
  });
});

describe("surfaces", () => {
  const meta = (id: string, agent: string, version = 1, created_at = 1) => ({
    id,
    agent,
    kind: "cockpit" as const,
    title: `Surface ${id}`,
    version,
    created_at,
    updated_at: created_at,
  });
  const listed = (id: number, agent: string, surfaces: ReturnType<typeof meta>[]): Action[] => [
    { type: "requested", id, command: { command: "surfaces", agent } },
    server({ type: "reply", id, reply: { kind: "surfaces", surfaces } }),
  ];
  const updated = (surface: ReturnType<typeof meta>, deleted = false) =>
    server({ type: "event", event: { type: "surface_updated", surface, deleted } });
  /** Le flux d'une surface sans les numéros de ligne. */
  const feedOf = (state: AppState, surface: string) =>
    (state.surfaceFeeds[surface] ?? []).map(({ seq: _, ...item }) => item);

  it("tient la liste d'un agent : lecture, création, nouvelle version, suppression", () => {
    let state = run(tree, ...listed(2, "dom", [meta("s-b", "dom", 1, 20), meta("s-a", "dom", 1, 10)]));
    expect(state.surfaces.dom.map((s) => s.id)).toEqual(["s-a", "s-b"]);

    state = reduce(reduce(state, updated(meta("s-c", "dom", 1, 30))), updated(meta("s-a", "dom", 2, 10)));
    expect(state.surfaces.dom.map((s) => [s.id, s.version])).toEqual([
      ["s-a", 2],
      ["s-b", 1],
      ["s-c", 1],
    ]);

    state = reduce(state, updated(meta("s-b", "dom"), true));
    expect(state.surfaces.dom.map((s) => s.id)).toEqual(["s-a", "s-c"]);
  });

  it("vide la liste d'un agent sans surface, d'après la requête", () => {
    const state = run(tree, ...listed(2, "dom", [meta("s-a", "dom")]), ...listed(3, "dom", []));
    expect(state.surfaces.dom).toEqual([]);
  });

  it("garde le code lu et oublie tout d'une surface supprimée", () => {
    const doc = { ...meta("s-a", "dom", 2), html: "<p>salut</p>" };
    let state = run(
      tree,
      server({ type: "reply", id: 4, reply: { kind: "surface", surface: doc } }),
      server({ type: "event", event: { type: "surface_data", surface: "s-a", data: { lampes: 4 } } }),
    );
    expect(state.surfaceDocs["s-a"]).toEqual(doc);
    expect(state.surfaces.dom.map((s) => s.version)).toEqual([2]);
    expect(feedOf(state, "s-a")).toEqual([{ type: "data", data: { lampes: 4 } }]);

    state = reduce(state, updated(meta("s-a", "dom", 2), true));
    expect(state.surfaces.dom).toEqual([]);
    expect(state.surfaceDocs["s-a"]).toBeUndefined();
    expect(state.surfaceFeeds["s-a"]).toBeUndefined();
  });

  it("remet aux surfaces d'un agent ses envois, ses messages et l'état du tour", () => {
    const state = run(
      tree,
      ...listed(2, "dom", [meta("s-a", "dom"), meta("s-b", "dom", 1, 2)]),
      { type: "requested", id: 3, command: { command: "surface_send", surface: "s-a", text: "allume" } },
      input("dom", "[Depuis la surface « Surface s-a » (s-a)]\nallume"),
      server({ type: "event", event: { type: "message", agent: "dom", text: "reply 1" } }),
      server({ type: "event", event: { type: "message", agent: "a0", text: "pas pour elles" } }),
      server({
        type: "event",
        event: { type: "turn_completed", agent: "dom", outcome: { status: "completed" }, usage: null },
      }),
    );
    expect(feedOf(state, "s-a")).toEqual([
      { type: "sent", text: "allume" },
      { type: "busy", busy: true },
      { type: "message", text: "reply 1" },
      { type: "busy", busy: false },
    ]);
    expect(feedOf(state, "s-b")).toEqual([
      { type: "busy", busy: true },
      { type: "message", text: "reply 1" },
      { type: "busy", busy: false },
    ]);
  });

  it("relit la liste après un changement de l'agent, et la retire avec lui", () => {
    let state = run(tree, ...listed(2, "dom", [meta("s-a", "dom")]));
    state = reduce(state, server({ type: "event", event: { type: "agent_updated", agent: { ...domotique, busy: true } } }));
    expect(state.surfaceRevisions.dom).toBe(1);
    state = reduce(
      state,
      server({ type: "event", event: { type: "agent_updated", agent: { ...domotique, state: "retired" } } }),
    );
    expect(state.surfaces.dom).toBeUndefined();
  });
});

describe("texte en streaming", () => {
  const delta = (agent: string, text: string): Action =>
    server({ type: "event", event: { type: "message_delta", agent, text } });
  const message = (agent: string, text: string): Action =>
    server({ type: "event", event: { type: "message", agent, text } });

  it("s'écrit au fil des fragments puis laisse place au message complet", () => {
    let state = run(tree, input("a0", "bonjour"), delta("a0", "Bon"), delta("a0", "jour à "));
    expect(thread(state, "a0").slice(1)).toEqual([{ kind: "streaming", text: "Bonjour à " }]);

    state = reduce(state, message("a0", "Bonjour à toi."));
    expect(thread(state, "a0").slice(1)).toEqual([{ kind: "agent", text: "Bonjour à toi." }]);
    // Un outil entre deux messages : le second message s'écrit à part.
    state = [
      server({ type: "event", event: { type: "tool", agent: "a0", name: "Bash" } }),
      delta("a0", "Fini"),
      message("a0", "Fini."),
    ].reduce(reduce, state);
    expect(thread(state, "a0").slice(1)).toEqual([
      { kind: "agent", text: "Bonjour à toi." },
      { kind: "tool", name: "Bash" },
      { kind: "agent", text: "Fini." },
    ]);
  });

  it("garde le texte reçu d'un tour interrompu", () => {
    const state = run(
      tree,
      delta("a0", "J'allais dire"),
      server({
        type: "event",
        event: { type: "turn_completed", agent: "a0", outcome: { status: "interrupted" }, usage: null },
      }),
    );
    expect(thread(state, "a0")).toEqual([
      { kind: "agent", text: "J'allais dire" },
      { kind: "turn", outcome: { status: "interrupted" }, usage: null },
    ]);
  });

  it("ne fait pas relire le board à chaque fragment", () => {
    const state = run(tree, delta("dom", "a"), delta("dom", "b"));
    expect(state.boardRevisions).toEqual({});
  });
});

describe("déclencheurs", () => {
  const trigger = (id: string, owner: string, target: string, extra: Partial<Trigger> = {}): Trigger => ({
    id,
    owner,
    target,
    name: `déclencheur ${id}`,
    kind: "switch",
    spec: "",
    prompt: "Fais le point.",
    secret: null,
    enabled: true,
    created_at: 1,
    armed_at: 1,
    last_fired: null,
    ...extra,
  });
  const listed = (id: number, agent: string, triggers: Trigger[]): Action[] => [
    { type: "requested", id, command: { command: "triggers", agent } },
    server({ type: "reply", id, reply: { kind: "triggers", triggers } }),
  ];

  it("garde la liste de chaque agent", () => {
    const state = run(tree, ...listed(2, "dom", [trigger("tr-1", "dom", "dom")]), ...listed(3, "a0", []));
    expect(state.triggers.dom.map((t) => t.id)).toEqual(["tr-1"]);
    expect(state.triggers.a0).toEqual([]);
  });

  it("relit la cible et le propriétaire quand un déclencheur part", () => {
    const state = run(
      tree,
      ...listed(2, "a0", [trigger("tr-1", "a0", "dom")]),
      ...listed(3, "dom", [trigger("tr-1", "a0", "dom")]),
      input("dom", "[Déclencheur « pause » (tr-1, switch)]\nFais le point.", { kind: "argos" }),
      input("dom", "un message ordinaire"),
    );
    expect(state.triggerRevisions).toEqual({ dom: 1, a0: 1 });
  });

  it("relit les listes concernées après une action", () => {
    const state = run(
      tree,
      ...listed(2, "dom", [trigger("tr-1", "dom", "dom")]),
      ...listed(3, "a0", [trigger("tr-9", "a0", "a0")]),
      { type: "requested", id: 4, command: { command: "set_trigger", trigger: "tr-1", enabled: false } },
      server({ type: "reply", id: 4, reply: { kind: "done", affected: [] } }),
    );
    expect(state.triggerRevisions).toEqual({ dom: 1 });
  });

  it("disparaissent avec leur agent retiré", () => {
    let state = run(tree, ...listed(2, "dom", [trigger("tr-1", "dom", "dom")]));
    state = reduce(
      state,
      server({ type: "event", event: { type: "agent_updated", agent: { ...domotique, state: "retired" } } }),
    );
    expect(state.triggers.dom).toBeUndefined();
  });
});

describe("nœuds", () => {
  it("garde le dernier état des nœuds, et le nœud de chaque agent", () => {
    const remote = agent("pi-agent", "agent", "a0", { node: "pi" });
    const state = run(
      server({ type: "reply", id: 1, reply: { kind: "tree", agents: [assistant, remote] } }),
      server({
        type: "reply",
        id: 2,
        reply: { kind: "nodes", nodes: [{ name: "pi", connected: false, error: "ssh : délai dépassé" }] },
      }),
      server({ type: "reply", id: 3, reply: { kind: "nodes", nodes: [{ name: "pi", connected: true, error: null }] } }),
    );
    expect(state.nodes).toEqual([{ name: "pi", connected: true, error: null }]);
    expect(state.agents["pi-agent"].node).toBe("pi");
    expect(state.agents.a0.node).toBeNull();
  });

  it("restent inconnus tant que le daemon n'a rien dit", () => {
    expect(initialState.nodes).toBeNull();
  });
});
