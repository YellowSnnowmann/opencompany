// Paired devices: the machines signed in as you, and the codes that enrol them.
//
// The host has served these routes since device pairing landed; this module is
// what the console calls them with. Until it existed the desktop's pairing
// prompt sent people to a page nobody had built (issue #1476).
//
// The direction of the flow is the reverse of OAuth's device flow, and that is
// deliberate — see `src/server/users/devices.rs`. The consequence for a caller
// here: `startPairing` is the *authenticated* half. A signed-in human mints a
// code in this console and carries it to the machine being enrolled, which
// redeems it over `…/devices/claim`. That redemption is not in this file and
// must not be: it is the desktop's own call, made through the Tauri bridge, so
// the session token it returns reaches the OS keychain without ever passing
// through the webview.

import type { OpenCompanyClient } from "./client";

/**
 * A freshly minted pairing code.
 *
 * The plaintext comes back exactly once — only its hash is stored — so a caller
 * that drops it has to mint another. Nothing here can re-read it.
 */
export interface PairingCode {
  code: string;
  expiresAtMillis: number;
}

/** A paired device as its owner sees it. Carries no secret. */
export interface Device {
  id: string;
  label?: string;
  createdAtMillis: number;
  expiresAtMillis: number;
  /**
   * Whether this is the credential making the request — true only when the
   * console is itself running in a paired desktop. A browser session is never a
   * device, so in a browser this is false for every row.
   */
  current: boolean;
}

/** The signed-in user's paired devices, most recently paired first. */
export async function listDevices(
  client: OpenCompanyClient,
  company: string | null,
): Promise<Device[]> {
  const devices = await client.get<Device[]>(`${client.scopeFor(company)}/devices`);
  return [...devices].sort((a, b) => b.createdAtMillis - a.createdAtMillis);
}

/**
 * Mints a pairing code for the signed-in user.
 *
 * Available to any member, not just admins: pairing enrols a machine as
 * *yourself* and grants nothing you do not already have. It is refused for a
 * caller that is itself a paired device — otherwise one compromised desktop
 * could enrol further machines that survive revoking it. Surface the host's
 * message rather than rewording it; it names the remedy (sign in on the web
 * console).
 */
export async function startPairing(
  client: OpenCompanyClient,
  company: string | null,
): Promise<PairingCode> {
  return client.post<PairingCode>(`${client.scopeFor(company)}/devices`, {});
}

/**
 * Revokes one paired device.
 *
 * Scoped server-side to the caller's own devices, so an id belonging to someone
 * else is simply not found. Revocation is immediate — the record is the session,
 * so deleting it is what signs that machine out.
 */
export async function revokeDevice(
  client: OpenCompanyClient,
  company: string | null,
  deviceId: string,
): Promise<{ revoked: boolean }> {
  return client.del<{ revoked: boolean }>(
    `${client.scopeFor(company)}/devices/${encodeURIComponent(deviceId)}`,
  );
}
