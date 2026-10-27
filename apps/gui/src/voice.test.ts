import { describe, expect, it } from "vitest";
import type { DaemonEvent } from "./protocol";
import { initialState, reduce, type Action, type AppState } from "./state";
import { bargeIn, IDLE_PLAN, onAssistantEvent, voiceSent, type SpeechAction, type SpeechPlan } from "./voice";

const user = { kind: "user" } as const;
const input = (text: string): DaemonEvent => ({ type: "input", agent: "a0", from: user, text });
const delta = (text: string): DaemonEvent => ({ type: "message_delta", agent: "a0", text });
const message = (text: string): DaemonEvent => ({ type: "message", agent: "a0", text });
const done: DaemonEvent = { type: "turn_completed", agent: "a0", outcome: { status: "completed" }, usage: null };

/** Rejoue des événements de l'Assistant ; rend le plan et ce qui a été demandé à la voix. */
function play(plan: SpeechPlan, events: DaemonEvent[], speakAll = false) {
  const actions: SpeechAction[] = [];
  for (const event of events) {
    const result = onAssistantEvent(plan, event, speakAll);
    plan = result.plan;
    actions.push(...result.actions);
  }
  return { plan, actions };
}

describe("lecture des réponses", () => {
  it("lit la réponse à une demande faite à la voix, fragment par fragment", () => {
    const sent = voiceSent(IDLE_PLAN, "Dis-moi bonjour");
    const { plan, actions } = play(sent, [input("Dis-moi bonjour"), delta("Bonjour, "), delta("Alexandre."), message("Bonjour, Alexandre."), done]);
    expect(actions).toEqual([
      { type: "speak", text: "Bonjour, " },
      { type: "speak", text: "Alexandre." },
      { type: "finish" },
    ]);
    expect(plan).toEqual(IDLE_PLAN);
  });

  it("ne lit pas la réponse à une demande tapée", () => {
    expect(play(IDLE_PLAN, [input("tapé"), delta("Réponse"), message("Réponse"), done]).actions).toEqual([]);
  });

  it("lit tout avec speak_typed_replies", () => {
    const { actions } = play(IDLE_PLAN, [input("tapé"), delta("Réponse"), message("Réponse"), done], true);
    expect(actions).toEqual([{ type: "speak", text: "Réponse" }, { type: "finish" }]);
  });

  it("lit d'un bloc un message arrivé sans fragments", () => {
    const { actions } = play(voiceSent(IDLE_PLAN, "salut"), [input("salut"), message("Salut !"), done]);
    expect(actions).toEqual([{ type: "speak", text: "Salut !" }, { type: "finish" }]);
  });

  it("ne lit pas le tour d'une autre demande arrivée avant celle faite à la voix", () => {
    const sent = voiceSent(IDLE_PLAN, "à la voix");
    const { plan, actions } = play(sent, [input("tapé avant"), delta("pour l'autre"), done]);
    expect(actions).toEqual([]);
    expect(plan.awaiting).toEqual(["à la voix"]);
  });

  it("arrête de lire le tour en cours sur un nouvel appui (barge-in)", () => {
    const reading = play(voiceSent(IDLE_PLAN, "longue"), [input("longue"), delta("Il était ")]).plan;
    const { actions } = play(bargeIn(reading), [delta("une fois"), done]);
    expect(actions).toEqual([]);
  });
});

describe("la voix dans la console", () => {
  const assistant = {
    id: "a0",
    level: "assistant" as const,
    parent: null,
    engine: "claude" as const,
    model: null,
    profile: null,
    node: null,
    workdir: "/tmp/a0",
    charter: null,
    state: "active" as const,
    session_id: null,
    created_at: 1,
    running: true,
    busy: false,
  };
  const tree: Action = {
    type: "server",
    message: { type: "reply", id: 1, reply: { kind: "tree", agents: [assistant] } },
  };
  const event = (event: DaemonEvent): Action => ({ type: "server", message: { type: "event", event } });
  const run = (...actions: Action[]): AppState => actions.reduce(reduce, initialState);

  it("montre la dictée puis marque la demande reçue « à la voix », sans doublon", () => {
    let state = run(tree, { type: "voice_partial", text: "Dis-moi" });
    expect(state.voice.dictation).toBe("Dis-moi");
    state = [
      { type: "voice_transcript", text: " Dis-moi bonjour " } as Action,
      event(input("Dis-moi bonjour")),
    ].reduce(reduce, state);
    expect(state.voice.dictation).toBeNull();
    expect(state.voice.spoken).toEqual([]);
    expect(state.threads.a0.map(({ seq: _, ...entry }) => entry)).toEqual([
      { kind: "input", text: "Dis-moi bonjour", from: user, console: null, voice: true },
    ]);
  });

  it("ignore une transcription vide", () => {
    expect(run(tree, { type: "voice_transcript", text: "  " }).voice.spoken).toEqual([]);
  });

  it("dit pourquoi il n'y a pas de voix, une fois", () => {
    const disabled: Action = {
      type: "voice_status",
      status: { status: "disabled", reason: "pas de section [voice] dans config.toml : voix désactivée" },
    };
    const state = run(disabled, disabled);
    expect(state.notices).toEqual([
      {
        id: expect.any(Number),
        tone: "info",
        text: "Voix indisponible : pas de section [voice] dans config.toml : voix désactivée",
      },
    ]);
    expect(run({ type: "voice_status", status: { status: "ready" } }).notices).toEqual([]);
    expect(run({ type: "voice_status", status: { status: "failed", reason: "whisper-server absent" } }).notices[0].tone).toBe(
      "warning",
    );
  });

  it("met les erreurs de la voix dans les avis", () => {
    expect(run({ type: "voice_error", message: "micro indisponible" }).notices[0]).toMatchObject({
      tone: "error",
      text: "Voix : micro indisponible",
    });
  });
});
