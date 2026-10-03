// A stand-in for the desktop shell's two analytics commands, for the Privacy
// page and first-launch notice tests. It models the part of the shell those
// tests depend on: the preference is remembered across calls, `source` flips
// from `default` to `setting` once anybody chooses, and every call is recorded.

import type { AnalyticsPreference } from "@/api/transport/desktop";

export interface Call {
  command: string;
  args?: Record<string, unknown>;
}

export const basePreference: AnalyticsPreference = {
  enabled: true,
  source: "default",
  restart_required: false,
  status: {
    decision: "reporting",
    reason: "reporting",
    deployment: "desktop",
    endpoint: "https://panel.tinyhumans.ai/api/track",
    in_build: true,
    consent: true,
    last_send: "accepted",
    last_status: 200,
    last_at: "2026-10-01T10:00:00Z",
    accepted: 7,
    dropped: 1,
  },
};

export function installShell(initial: Partial<AnalyticsPreference> = {}) {
  const calls: Call[] = [];
  let state: AnalyticsPreference = { ...basePreference, ...initial };
  (window as unknown as { __TAURI__: unknown }).__TAURI__ = {
    core: {
      Channel: class {},
      async invoke(command: string, args?: Record<string, unknown>) {
        calls.push({ command, args });
        if (command === "oc_set_analytics_preference") {
          const enabled = args?.enabled as boolean;
          state = { ...state, enabled, source: "setting" };
        }
        return state;
      },
    },
  };
  return calls;
}

export function removeShell(): void {
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
}
