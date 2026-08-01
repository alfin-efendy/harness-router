import { PROVIDERS } from "./providers";

/**
 * Worker environment bindings.
 *
 * - `*_CLIENT_ID` come from `wrangler.toml` [vars] (public by design).
 * - `*_CLIENT_SECRET` are Worker secrets, set via `wrangler secret put` — never in the repo.
 * - `OAUTH_RELAY_RATE_LIMITER` is Cloudflare's rate-limiting binding. It is optional here on
 *   purpose: it is absent under `bun test` and can be absent under `wrangler dev`, and the
 *   handler must keep working without it (see the rate-limiting section below).
 *
 * The index signature lets the handler look up `env[provider.clientIdVar]` /
 * `env[provider.clientSecretVar]` dynamically from the provider registry.
 */
export interface Env {
  ATLASSIAN_CLIENT_ID?: string;
  ATLASSIAN_CLIENT_SECRET?: string;
  BITBUCKET_CLIENT_ID?: string;
  BITBUCKET_CLIENT_SECRET?: string;
  OAUTH_RELAY_RATE_LIMITER?: RateLimit;
  [key: string]: string | RateLimit | undefined;
}

/** Loopback redirect_uri shape Cockpit uses for its local OAuth callback server. */
const REDIRECT_URI_RE = /^http:\/\/127\.0\.0\.1:8976\/plugin-oauth\/[a-z0-9][a-z0-9-]*\/profile\/[a-z0-9][a-z0-9-]*\/callback$/;

const MAX_BODY_BYTES = 4096;

function jsonResponse(status: number, body: unknown, extraHeaders?: Record<string, string>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json",
      "cache-control": "no-store",
      ...extraHeaders,
    },
  });
}

function methodNotAllowed(allow: string[]): Response {
  return jsonResponse(405, { error: "method_not_allowed" }, { allow: allow.join(", ") });
}

/** Reads the request body as text, refusing (returns null) anything over `maxBytes`. */
async function readCappedBody(request: Request, maxBytes: number): Promise<string | null> {
  if (!request.body) {
    return "";
  }
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value) {
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  }
  const buffer = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    buffer.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(buffer);
}

function isRateLimiter(value: unknown): value is RateLimit {
  return typeof value === "object" && value !== null && typeof (value as RateLimit).limit === "function";
}

/**
 * Handles the token-relay routes. Exported separately from the `default` Workers export so
 * tests can call it directly with hand-built `Request`s and a stubbed global `fetch` — no
 * Workers runtime (miniflare/wrangler) needed.
 */
export async function handleRequest(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);

  if (url.pathname === "/health") {
    if (request.method !== "GET") {
      return methodNotAllowed(["GET"]);
    }
    return jsonResponse(200, { ok: true });
  }

  const tokenMatch = url.pathname.match(/^\/token\/([^/]+)$/);
  if (!tokenMatch) {
    return jsonResponse(404, { error: "not_found" });
  }

  if (request.method !== "POST") {
    return methodNotAllowed(["POST"]);
  }

  const providerKey = tokenMatch[1] ?? "";
  // `Object.hasOwn` (not `providerKey in PROVIDERS` / plain index access) so prototype-chain
  // keys like `__proto__`, `constructor`, `toString` correctly 404 instead of resolving to an
  // inherited value and falling through to the 503 "unconfigured" branch below.
  const provider = Object.hasOwn(PROVIDERS, providerKey) ? PROVIDERS[providerKey] : undefined;
  if (!provider) {
    return jsonResponse(404, { error: "unknown_provider" });
  }

  const clientId = env[provider.clientIdVar];
  const clientSecret = env[provider.clientSecretVar];
  if (typeof clientId !== "string" || clientId.length === 0 || typeof clientSecret !== "string" || clientSecret.length === 0) {
    // Generic message only — never reveal which specific var is missing.
    console.log(`oauth-proxy: provider "${providerKey}" is not configured`);
    return jsonResponse(503, { error: "provider_unconfigured" });
  }

  const limiter = env.OAUTH_RELAY_RATE_LIMITER;
  if (isRateLimiter(limiter)) {
    const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
    const { success } = await limiter.limit({ key: ip });
    if (!success) {
      console.log(`oauth-proxy: rate limit exceeded for provider "${providerKey}"`);
      return jsonResponse(429, { error: "rate_limited" });
    }
  }

  const contentType = request.headers.get("content-type") ?? "";
  if (!contentType.toLowerCase().startsWith("application/x-www-form-urlencoded")) {
    return jsonResponse(415, { error: "unsupported_content_type" });
  }

  const bodyText = await readCappedBody(request, MAX_BODY_BYTES);
  if (bodyText === null) {
    return jsonResponse(413, { error: "payload_too_large" });
  }

  const incoming = new URLSearchParams(bodyText);
  const grantType = incoming.get("grant_type");
  if (grantType !== "authorization_code" && grantType !== "refresh_token") {
    return jsonResponse(400, { error: "invalid_grant_type" });
  }

  // Allowlisted outgoing form: only these fields are ever copied through, and only the ones
  // relevant to the grant type in play. Anything else the caller sent (client_id, scope,
  // audience, ...) is silently dropped.
  const outgoing = new URLSearchParams();
  outgoing.set("grant_type", grantType);

  if (grantType === "authorization_code") {
    const code = incoming.get("code");
    const redirectUri = incoming.get("redirect_uri");
    // Cockpit always sends PKCE. A missing code_verifier means the caller isn't our client
    // (or is trying to downgrade off PKCE) — reject before the redirect_uri check even runs,
    // and well before any upstream call.
    const codeVerifier = incoming.get("code_verifier");
    if (!code || !redirectUri || !codeVerifier) {
      return jsonResponse(400, { error: "missing_parameters" });
    }
    if (!REDIRECT_URI_RE.test(redirectUri)) {
      return jsonResponse(400, { error: "unsupported_redirect_uri" });
    }
    outgoing.set("code", code);
    outgoing.set("redirect_uri", redirectUri);
    outgoing.set("code_verifier", codeVerifier);
  } else {
    const refreshToken = incoming.get("refresh_token");
    if (!refreshToken) {
      return jsonResponse(400, { error: "missing_parameters" });
    }
    outgoing.set("refresh_token", refreshToken);
  }

  // Always inject the configured credentials, overriding anything the caller sent.
  outgoing.set("client_id", clientId);

  const upstreamHeaders = new Headers({
    accept: "application/json",
    "content-type": "application/x-www-form-urlencoded",
  });

  if (provider.clientAuth === "body") {
    outgoing.set("client_secret", clientSecret);
  } else {
    upstreamHeaders.set("authorization", `Basic ${btoa(`${clientId}:${clientSecret}`)}`);
  }

  let upstream: Response;
  try {
    upstream = await fetch(provider.tokenUrl, {
      method: "POST",
      headers: upstreamHeaders,
      body: outgoing.toString(),
    });
  } catch {
    // Never log the underlying error — it could echo back request details.
    console.log(`oauth-proxy: upstream transport failure for provider "${providerKey}"`);
    return jsonResponse(502, { error: "upstream_unreachable" });
  }

  const upstreamBody = await upstream.text();
  console.log(`oauth-proxy: provider "${providerKey}" token exchange -> ${upstream.status}`);

  return new Response(upstreamBody, {
    status: upstream.status,
    headers: {
      "content-type": "application/json",
      "cache-control": "no-store",
    },
  });
}

export default {
  fetch: handleRequest,
} satisfies ExportedHandler<Env>;
