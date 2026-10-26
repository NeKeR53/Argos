import { describe, expect, it } from "vitest";
import type { AgentStatus, ServerMessage } from "./protocol";
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

const input = (agent: string, text: string): Action => server({ type: "event", event: { type: "input", agent, text } });
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
        { kind: "input", text: "salut", console: { request: 2, delivery: "delivered", awaitingEcho: false } },
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
      { kind: "input", text: mission, console: null },
      { kind: "input", text: "bonjour", console: null },
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
      { kind: "input", text: "salut", console: { request: 5, delivery: "failed", awaitingEcho: false } },
      { kind: "input", text: "salut", console: null },
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
