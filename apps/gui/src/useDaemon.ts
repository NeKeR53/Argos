// Branche la connexion au daemon sur le réducteur de l'état de la console.

import { useCallback, useEffect, useReducer, useRef } from "react";
import { DaemonConnection, endpointFromEnvironment } from "./connection";
import type { Command } from "./protocol";
import { initialState, reduce, type Action, type AppState } from "./state";

export interface Daemon {
  state: AppState;
  dispatch: (action: Action) => void;
  /** Envoie une commande ; `false` si la connexion n'est pas ouverte. */
  send: (command: Command) => boolean;
}

export function useDaemon(): Daemon {
  const [state, dispatch] = useReducer(reduce, initialState);
  const connection = useRef<DaemonConnection | null>(null);

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
      onMessage: (message) => dispatch({ type: "server", message }),
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
