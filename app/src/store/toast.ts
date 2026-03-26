import { create } from "zustand";

export type ToastKind = "info" | "warn" | "error" | "success";

export interface Toast {
  id: string;
  message: string;
  kind: ToastKind;
}

interface ToastStore {
  toasts: Toast[];
  push: (message: string, kind?: ToastKind) => void;
  dismiss: (id: string) => void;
}

export const useToastStore = create<ToastStore>((set) => ({
  toasts: [],
  push(message, kind = "info") {
    const id = `${Date.now()}-${Math.random()}`;
    set((s) => ({ toasts: [...s.toasts, { id, message, kind }] }));
    setTimeout(() => {
      set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
    }, 4000);
  },
  dismiss(id) {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },
}));

/** Convenience — call outside React (e.g. in event handlers). */
export function toast(message: string, kind: ToastKind = "info") {
  useToastStore.getState().push(message, kind);
}
