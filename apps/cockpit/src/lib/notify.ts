import type { CoreEvent, Session } from "@/bindings";
import { isUnreadVisible, sessionTitle } from "@/lib/sidebar";
import { sessKey, type SessionRef, type UiSession } from "@/lib/session-key";

/** Total items needing the user: unread sessions + pending approvals. The
 *  focused session is never counted (isUnreadVisible excludes it). */
export function attentionCount(
  sessions: UiSession[],
  readAt: Record<string, number>,
  focusedSession: SessionRef | null,
  pendingApprovalCount: number,
): number {
  const unread = sessions.filter((s) => isUnreadVisible(s, readAt, focusedSession)).length;
  return unread + pendingApprovalCount;
}

export type NotifyIntent = {
  runnerId: string;
  sessionPk: string;
  kind: "finished" | "approval" | "error";
  settle: boolean;
  detail?: string;
} | null;

/** What (if anything) to notify for a CoreEvent. Suppressed entirely while the
 *  window is focused (the in-app unread dot already signals it). `runnerId` is
 *  the runner that produced the event. */
export function notifyIntentForEvent(event: CoreEvent, runnerId: string, windowFocused: boolean): NotifyIntent {
  if (windowFocused) return null;
  switch (event.kind) {
    case "result":
      return { runnerId, sessionPk: event.session_pk, kind: "finished", settle: true };
    case "approvalRequested":
      return { runnerId, sessionPk: event.session_pk, kind: "approval", settle: false, detail: event.tool };
    case "error":
      return { runnerId, sessionPk: event.session_pk, kind: "error", settle: false };
    default:
      return null;
  }
}

export type JobNotifyIntent = { title: string; body: string; level: "success" | "error" } | null;

/** What (if anything) to raise for a `jobRunChanged` event. The engine has
 *  already applied the job's own notify-on-success / notify-on-failure
 *  switches and the run's `[SILENT]` opt-out — `event.notify` is that whole
 *  decision, so this function only formats it. Unlike per-turn intents this
 *  is NOT suppressed while the window is focused: the caller turns a focused
 *  one into an in-app toast instead. */
export function jobNotifyIntentForEvent(event: CoreEvent): JobNotifyIntent {
  if (event.kind !== "jobRunChanged") return null;
  if (!event.notify) return null;
  if (event.status !== "success" && event.status !== "failed") return null;
  // `job_name`/`notify`/`detail` are `#[serde(default)]` on the Rust side (a
  // remote daemon on an older version omits them), which specta renders as
  // optional here — so every read of them must tolerate `undefined`. A missing
  // `notify` is already handled above: falsy means "do not notify", the safe
  // fallback for an engine too old to have made the decision.
  const title = event.job_name?.trim() || "Scheduled job";
  if (event.status === "failed") {
    return { title, body: `Run failed — ${event.detail ?? "no error reported"}`, level: "error" };
  }
  return { title, body: `Run finished${event.detail ? ` — ${event.detail}` : ""}`, level: "success" };
}

/** A session the scheduler started. Its per-turn `result`/`error` events must
 *  NOT raise the generic "Turn finished" notification — the matching
 *  `jobRunChanged` event notifies instead, and that one knows the job's name,
 *  its notify switches and the real run outcome. Without this guard every
 *  scheduled run notifies twice. */
export function isSchedulerSession(session: Session | undefined): boolean {
  return session?.startedBy === "scheduler";
}

export const SETTLE_MS = 3000;

export type NotifierDeps = {
  sendNotification: (o: { title: string; body: string }) => void;
  setBadgeCount: (n: number | undefined) => void;
  ensurePermission: () => Promise<boolean>;
  isEnabled: () => boolean;
  /** Schedule `fn` after `ms`; returns a cancel function. */
  schedule: (fn: () => void, ms: number) => () => void;
};

export type Notifier = {
  /** Send a notification immediately, bypassing session keying and settles —
   *  for events that are not tied to one session's turn (scheduled job runs). */
  notifyNow(text: { title: string; body: string }): void;
  handle(intent: NonNullable<NotifyIntent>, session: Session | undefined): void;
  cancelSettle(runnerId: string, sessionPk: string): void;
  cancelAllSettles(): void;
  updateBadge(count: number): void;
};

/** Title/body for a notification. Title is the session title; body states the
 *  kind. */
export function notificationText(intent: NonNullable<NotifyIntent>, session: Session | undefined): { title: string; body: string } {
  const title = session ? sessionTitle(session) : "Session";
  const body =
    intent.kind === "approval"
      ? `Needs approval: ${intent.detail ?? "a tool"}`
      : intent.kind === "error"
        ? "Turn errored"
        : "Turn finished";
  return { title, body };
}

export function createNotifier(deps: NotifierDeps): Notifier {
  // Keyed by `sessKey(runnerId, sessionPk)` — settles must not collide across
  // runners that share a session pk.
  const settles = new Map<string, () => void>();
  // Real counts are >= 0, so -1 guarantees the first updateBadge call fires.
  let lastBadge = -1;

  const cancelSettle = (runnerId: string, sessionPk: string) => {
    const key = sessKey(runnerId, sessionPk);
    const cancel = settles.get(key);
    if (cancel) {
      cancel();
      settles.delete(key);
    }
  };

  const send = (intent: NonNullable<NotifyIntent>, session: Session | undefined) => {
    if (!deps.isEnabled()) return;
    void deps.ensurePermission().then((ok) => {
      if (ok) deps.sendNotification(notificationText(intent, session));
    });
  };

  return {
    notifyNow(text) {
      if (!deps.isEnabled()) return;
      void deps.ensurePermission().then((ok) => {
        if (ok) deps.sendNotification(text);
      });
    },
    handle(intent, session) {
      const key = sessKey(intent.runnerId, intent.sessionPk);
      // Any new event for a session supersedes its pending "finished" settle.
      cancelSettle(intent.runnerId, intent.sessionPk);
      if (!deps.isEnabled()) return;
      if (intent.settle) {
        const cancel = deps.schedule(() => {
          settles.delete(key);
          send(intent, session);
        }, SETTLE_MS);
        settles.set(key, cancel);
      } else {
        send(intent, session);
      }
    },
    cancelSettle,
    cancelAllSettles() {
      for (const cancel of settles.values()) cancel();
      settles.clear();
    },
    updateBadge(count) {
      if (count === lastBadge) return;
      lastBadge = count;
      deps.setBadgeCount(count || undefined);
    },
  };
}

import { getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { useUi } from "@/store-ui";
import { useStore } from "@/store";

/** Convenience: the badge number for the current store slices. */
export function badgeCountFor(
  sessions: UiSession[],
  readAt: Record<string, number>,
  focusedSession: SessionRef | null,
  pendingApprovalCount: number,
): number {
  return attentionCount(sessions, readAt, focusedSession, pendingApprovalCount);
}

let permissionChecked = false;
let cachedGranted = false;
export async function ensurePermission(): Promise<boolean> {
  if (permissionChecked) return cachedGranted;
  permissionChecked = true;
  try {
    cachedGranted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
  } catch {
    cachedGranted = false;
  }
  return cachedGranted;
}

/** The app-wide notifier, backed by the Tauri plugin + window. Badge writes are
 *  wrapped so an unsupported platform (Windows) is a silent no-op. */
export const notifier = createNotifier({
  sendNotification: (o) => {
    try {
      sendNotification(o);
    } catch {
      /* notification unavailable — ignore */
    }
  },
  setBadgeCount: (n) => {
    try {
      void getCurrentWindow()
        .setBadgeCount(n)
        .catch(() => {});
    } catch {
      /* badge unsupported (e.g. Windows) — no-op */
    }
  },
  ensurePermission,
  isEnabled: () => useUi.getState().notificationsEnabled,
  schedule: (fn, ms) => {
    const id = setTimeout(fn, ms);
    return () => clearTimeout(id);
  },
});

/** Whether the OS window is focused right now (updated by onFocusChanged). */
let windowFocused = true;
export function isWindowFocused(): boolean {
  return windowFocused;
}

let inited = false;
/** Idempotent: track window focus and keep the dock badge in sync with
 *  attention. Call once at startup. */
export function initNotifications(): void {
  if (inited) return;
  inited = true;

  void getCurrentWindow()
    .onFocusChanged(({ payload: focused }) => {
      windowFocused = focused;
      if (focused) notifier.cancelAllSettles(); // back at the app → drop pending
    })
    .catch(() => {});

  const recomputeBadge = () => {
    const st = useStore.getState();
    notifier.updateBadge(badgeCountFor(st.sessions, useUi.getState().readAt, st.focusedSession, st.pendingApprovals.length));
  };
  useStore.subscribe(recomputeBadge);
  useUi.subscribe(recomputeBadge);
  recomputeBadge();
}
