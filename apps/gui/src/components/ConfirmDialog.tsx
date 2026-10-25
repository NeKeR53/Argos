// Confirmation modale légère (`<dialog>` natif : Échap annule, le focus reste piégé).
// Le bouton de confirmation a le focus : Entrée confirme.

import { useEffect, useRef, type ReactNode } from "react";

interface Props {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  /** Action destructive : bouton rouge. */
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({ title, children, confirmLabel, danger, onConfirm, onCancel }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const element = dialog.current;
    if (element && !element.open) element.showModal();
    // `showModal` donne le focus au premier bouton (Annuler) : on le rend à la
    // confirmation après coup, `autoFocus` passerait avant l'ouverture.
    confirm.current?.focus();
    return () => element?.close();
  }, []);

  return (
    <dialog
      ref={dialog}
      className="dialog"
      aria-labelledby="dialog-title"
      onCancel={(event) => {
        event.preventDefault();
        onCancel();
      }}
    >
      <h2 id="dialog-title">{title}</h2>
      <div className="dialog-body">{children}</div>
      <div className="dialog-actions">
        <button type="button" className="button" onClick={onCancel}>
          Annuler
        </button>
        <button
          type="button"
          className={danger ? "button button-danger" : "button button-primary"}
          onClick={onConfirm}
          ref={confirm}
        >
          {confirmLabel}
        </button>
      </div>
    </dialog>
  );
}
