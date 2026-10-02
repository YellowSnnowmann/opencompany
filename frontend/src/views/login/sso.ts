/**
 * The `#/sso?token=…` auto-login fragment (WS-A design, Part 2).
 *
 * The dashboard opens a company at `https://<slug>.opencompany.work/#/sso?token=<jwt>`.
 * The token rides the URL *fragment* on purpose: a browser never sends a
 * fragment to a server, so it survives the cold-wake auto-refresh the manager's
 * holding page performs and never lands in a server log. `Login.tsx` reads it,
 * POSTs it to `/api/v1/…/sso/redeem`, and — on success — drops the owner
 * straight into the app (the setup wizard for a fresh company) with no email or
 * password field ever shown.
 *
 * This module is the *pure* half: turning a raw `window.location.hash` into the
 * token, if the fragment is the SSO route. It is a pure function so it can be
 * unit-tested without a document; the redeem call and the "Signed in as …"
 * dialog are the React half in `Login.tsx`.
 */

/**
 * The token in a `#/sso?token=…` fragment, or `null` when this is not the SSO
 * route.
 *
 * Parsed defensively: the hash may or may not carry the leading `#`, the route
 * segment may or may not carry a leading `/`, and the query may carry other
 * params in any order. Only a non-empty `token` on the `sso` route returns a
 * value; anything else — a different route, a missing or blank token — is
 * `null`, so a caller can treat "there is an SSO token to redeem" as a single
 * yes/no question.
 *
 * `hash` is taken as an argument rather than read from `window` so the parse is
 * testable and the caller owns when it reads the live location.
 */
export function ssoTokenFromHash(hash: string): string | null {
  // Strip a leading `#`, then split the route from its query. `#/sso?token=x`
  // and `#sso?token=x` both parse; the route is everything before the first
  // `?`.
  const withoutHash = hash.startsWith("#") ? hash.slice(1) : hash;
  const [rawRoute, ...queryParts] = withoutHash.split("?");
  const route = rawRoute.replace(/^\/+/, "").replace(/\/+$/, "");
  if (route !== "sso") return null;

  // Rejoin on `?` in case a value contained one; `URLSearchParams` handles the
  // rest of the decoding. A JWT is `[A-Za-z0-9_-].` segments, so it needs no
  // encoding, but a defensive decode costs nothing and tolerates one.
  const query = queryParts.join("?");
  const token = new URLSearchParams(query).get("token")?.trim();
  return token ? token : null;
}
