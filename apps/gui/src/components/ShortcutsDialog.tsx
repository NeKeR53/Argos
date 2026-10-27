// Réglage des raccourcis globaux : « Changer », puis la nouvelle combinaison au clavier.
// Elle est active tout de suite et écrite dans `[gui]` de config.toml par le cœur Rust.

import { useEffect, useRef, useState } from "react";
import {
  SHORTCUT_LABELS,
  captureShortcut,
  formatShortcut,
  saveShortcut,
  type ShortcutKey,
  type Shortcuts,
} from "../shortcuts";

interface Props {
  shortcuts: Shortcuts;
  mac: boolean;
  onChange: (shortcuts: Shortcuts) => void;
  onClose: () => void;
}

const KEYS: ShortcutKey[] = ["push_to_talk", "console", "panic"];

export function ShortcutsDialog({ shortcuts, mac, onChange, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [capturing, setCapturing] = useState<ShortcutKey | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const element = dialog.current;
    if (element && !element.open) element.showModal();
    return () => element?.close();
  }, []);

  // Pendant la capture, toute la frappe sert au raccourci (Échap annule).
  useEffect(() => {
    if (!capturing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const capture = captureShortcut(event);
      if (capture.kind === "waiting") return;
      if (capture.kind === "cancel") {
        setCapturing(null);
        setHint(null);
        return;
      }
      if (capture.kind === "needs_modifier") {
        setHint(`« ${capture.key} » seul gênerait la frappe partout : ajoute ⌘, ⌃, ⌥ ou ⇧ (ou une touche F).`);
        return;
      }
      const key = capturing;
      setCapturing(null);
      setHint(null);
      saveShortcut(key, capture.keys)
        .then((updated) => {
          setError(null);
          onChange(updated);
        })
        .catch((reason: unknown) => setError(String(reason)));
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [capturing, onChange]);

  return (
    <dialog
      ref={dialog}
      className="dialog shortcuts-dialog"
      aria-labelledby="shortcuts-title"
      onCancel={(event) => {
        event.preventDefault();
        if (capturing) return;
        onClose();
      }}
    >
      <h2 id="shortcuts-title">Raccourcis</h2>
      <div className="dialog-body">
        <ul className="shortcuts">
          {KEYS.map((key) => (
            <li key={key} className="shortcut-row">
              <span className="shortcut-label">{SHORTCUT_LABELS[key]}</span>
              {capturing === key ? (
                <kbd className="shortcut-capture">Appuie sur la combinaison…</kbd>
              ) : (
                <kbd>{formatShortcut(shortcuts[key], mac)}</kbd>
              )}
              <button
                type="button"
                className="button"
                onClick={() => {
                  setError(null);
                  setHint(null);
                  setCapturing(capturing === key ? null : key);
                }}
              >
                {capturing === key ? "Annuler" : "Changer"}
              </button>
            </li>
          ))}
        </ul>
        {hint && <p className="muted">{hint}</p>}
        {error && <p className="shortcut-error">{error}</p>}
        {shortcuts.errors.map((startup) => (
          <p key={startup} className="shortcut-error">
            {startup}
          </p>
        ))}
        <p className="muted">Actifs tout de suite, et gardés dans config.toml.</p>
      </div>
      <div className="dialog-actions">
        <button type="button" className="button button-primary" onClick={onClose} disabled={capturing !== null}>
          Fermer
        </button>
      </div>
    </dialog>
  );
}
