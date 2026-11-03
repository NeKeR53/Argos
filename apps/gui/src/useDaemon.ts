// Branche la connexion au daemon sur le réducteur de l'état de la console.

import { useCallback, useEffect, useReducer, useRef } from "react";
import { endpointFromEnvironment } from "./connection";
import { MeshConnection, meshNodesFromEnvironment } from "./mesh";
import type { Command, Reply, ServerMessage } from "./protocol";
import { initialState, reduce, type Action, type AppState } from "./state";

export interface Daemon {
  state: AppState;
  dispatch: (action: Action) => void;
  /** Envoie une commande ; `false` si la connexion n'est pas ouverte. */
  send: (command: Command) => boolean;
  request: (command: Command) => Promise<Reply>;
}

/**
 * `onMessage` voit passer chaque message du daemon, après le réducteur : pour les effets
 * qui n'ont pas leur place dans l'état (la voix lit les réponses de l'Assistant).
 */
export function useDaemon(onMessage?: (message: ServerMessage) => void): Daemon {
  const [state, dispatch] = useReducer(reduce, initialState);
  const connection = useRef<MeshConnection | null>(null);
  const observer = useRef(onMessage);
  observer.current = onMessage;

  useEffect(() => {
    const endpoint = endpointFromEnvironment();
    if (!endpoint) {
      dispatch({
        type: "connection",
        connection: {
          status: "unconfigured",
          reason:
            "Hors de l'app Tauri, indiquez le daemon dans l'adresse de la page : ?daemon=<URL WebSocket encodée>.",
        },
      });
      return;
    }
    // Le nœud local d'abord ; les autres nœuds du maillage (D48) s'y ajoutent dès que
    // l'app les connaît.
    const live = new MeshConnection(endpoint, [], {
      onMessage: (message) => {
        dispatch({ type: "server", message });
        observer.current?.(message);
      },
      onConnection: (connection) => dispatch({ type: "connection", connection }),
      onRequest: (id, command) => dispatch({ type: "requested", id, command }),
    });
    connection.current = live;
    live.start();
    let stopped = false;
    void meshNodesFromEnvironment().then((nodes) => {
      if (!stopped) for (const node of nodes) live.join(node);
    });
    return () => {
      stopped = true;
      live.stop();
      connection.current = null;
    };
  }, []);

  const send = useCallback(
    (command: Command) => connection.current?.send(command) != null,
    [],
  );
  const request = useCallback(
    (command: Command) => connection.current?.request(command) ?? Promise.reject(new Error("Daemon hors connexion.")),
    [],
  );

  return { state, dispatch, send, request };
}
