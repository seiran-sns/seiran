import { useCallback, useSyncExternalStore } from "react";
import type { Note } from "../api/client";

export type PollState = {
  poll: NonNullable<Note["poll"]>;
  votedByMe: number[];
};

const states = new Map<string, PollState>();
const listeners = new Map<string, Set<() => void>>();

export function getPollState(noteId: string): PollState | undefined {
  return states.get(noteId);
}

export function setPollState(noteId: string, state: PollState): void {
  states.set(noteId, state);
  listeners.get(noteId)?.forEach((listener) => listener());
}

export function subscribePollState(noteId: string, listener: () => void): () => void {
  let noteListeners = listeners.get(noteId);
  if (!noteListeners) {
    noteListeners = new Set();
    listeners.set(noteId, noteListeners);
  }
  noteListeners.add(listener);
  return () => {
    noteListeners!.delete(listener);
    if (noteListeners!.size === 0) listeners.delete(noteId);
  };
}

export function usePollState(noteId: string, initialPoll: Note["poll"]): PollState | undefined {
  const existing = states.get(noteId);
  if (initialPoll && (!existing || ((initialPoll.votedByMe?.length ?? 0) > 0 && existing.votedByMe.length === 0))) {
    states.set(noteId, {
      poll: initialPoll,
      votedByMe: initialPoll.votedByMe ?? [],
    });
  }
  const subscribeNote = useCallback((listener: () => void) => subscribePollState(noteId, listener), [noteId]);
  const getSnapshot = useCallback(() => states.get(noteId), [noteId]);
  return useSyncExternalStore(subscribeNote, getSnapshot, getSnapshot);
}
