import type { TaskView } from "../api";
import type { components } from "./generated";

/** Wire type from `/openapi.json` (`DomainEventDto`). */
export type DomainEvent = components["schemas"]["DomainEventDto"];

export function isTaskView(payload: unknown): payload is TaskView {
  if (!payload || typeof payload !== "object") return false;
  const p = payload as Record<string, unknown>;
  return typeof p.id === "string" && typeof p.status === "string" && typeof p.kind === "string";
}

/**
 * One EventSource for the app. Named SSE events (`event: job.created`) do not
 * fire `onmessage`, so each kind is subscribed explicitly.
 */
export function subscribeEvents(
  onEvent: (event: DomainEvent) => void,
  opts?: { since?: number },
): () => void {
  const since = opts?.since ?? 0;
  const source = new EventSource(`/api/v1/events?since=${since}`);
  const kinds = ["job.created", "job.updated", "match.updated", "task.updated", "profile.updated"];

  const handle = (ev: MessageEvent<string>) => {
    if (!ev.data || ev.data === "ping") return;
    try {
      onEvent(JSON.parse(ev.data) as DomainEvent);
    } catch {
      /* keep-alive or a truncated frame */
    }
  };

  for (const kind of kinds) {
    source.addEventListener(kind, handle as EventListener);
  }
  source.onmessage = handle;
  return () => source.close();
}
