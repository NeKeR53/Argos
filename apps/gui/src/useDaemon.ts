// Branche la connexion au daemon sur le réducteur de l'état de la console.

import { useCallback, useEffect, useReducer, useRef } from "react";
import { DaemonConnection, endpointFromEnvironment } from "./connection";
import type { Command, ServerMessage } from "./protocol";
import { initialState, reduce, type Action, type AppState } from "./state";

export interface Daemon {
  state: AppState;
  dispatch: (action: Action) => void;
  /** Envoie une commande ; `false` si la connexion n'est pas ouverte. */
  send: (command: Command) => boolean;
}

/**
 * `onMessage` voit passer chaque message du daemon, après le réducteur : pour les effets
 * qui n'ont pas leur place dans l'état (la voix lit les réponses de l'Assistant).
 */
export function useDaemon(onMessage?: (message: ServerMessage) => void): Daemon {
  const [state, dispatch] = useReducer(reduce, initialState);
  const connection = useRef<DaemonConnection | null>(null);
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
    const live = new DaemonConnection(endpoint, {
      onMessage: (message) => {
        dispatch({ type: "server", message });
        observer.current?.(message);
      },
      onConnection: (connection) => dispatch({ type: "connection", connection }),
      onRequest: (id, command) => dispatch({ type: "requested", id, command }),
    });
    connection.current = live;
    live.start();
    return () => {
      live.stop();
      connection.current = null;
    };
  }, []);

  const send = useCallback(
    (command: Command) => connection.current?.send(command) != null,
    [],
  );

  return { state, dispatch, send };
}
