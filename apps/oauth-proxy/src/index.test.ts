import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { handleRequest, type Env } from "./index";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function stubFetch(impl: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response> | Response) {
  const fn = mock(impl);
  globalThis.fetch = fn as unknown as typeof fetch;
  return fn;
}

const ENV: Env = {
  ATLASSIAN_CLIENT_ID: "atlassian-client-id",
  ATLASSIAN_CLIENT_SECRET: "atlassian-secret",
  BITBUCKET_CLIENT_ID: "bitbucket-client-id",
  BITBUCKET_CLIENT_SECRET: "bitbucket-secret",
};

const VALID_REDIRECT_URI = "http://127.0.0.1:8976/plugin-oauth/atlassian/profile/default/callback";

function postToken(provider: string, body: Record<string, string> | string, headers: Record<string, string> = {}) {
  const bodyStr = typeof body === "string" ? body : new URLSearchParams(body).toString();
  return new Request(`https://relay.test/token/${provider}`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded", ...headers },
    body: bodyStr,
  });
}

async function upstreamFormBody(fetchMock: ReturnType<typeof mock>): Promise<URLSearchParams> {
  const [, init] = fetchMock.mock.calls[0] as [RequestInfo | URL, RequestInit];
  return new URLSearchParams(init.body as string);
}

function upstreamHeaders(fetchMock: ReturnType<typeof mock>): Headers {
  const [, init] = fetchMock.mock.calls[0] as [RequestInfo | URL, RequestInit];
  return new Headers(init.headers as HeadersInit);
}

describe("POST /token/:provider", () => {
  it("1. authorization_code happy path (body-auth provider): forwards fields + injects client_id/client_secret, passes response through", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok", token_type: "bearer" }), { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-123",
        code_verifier: "verifier-abc",
        redirect_uri: VALID_REDIRECT_URI,
      }),
      ENV,
    );

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url] = fetchMock.mock.calls[0] as [RequestInfo | URL, RequestInit];
    expect(url).toBe("https://auth.atlassian.com/oauth/token");

    const sent = await upstreamFormBody(fetchMock);
    expect(sent.get("grant_type")).toBe("authorization_code");
    expect(sent.get("code")).toBe("auth-code-123");
    expect(sent.get("code_verifier")).toBe("verifier-abc");
    expect(sent.get("redirect_uri")).toBe(VALID_REDIRECT_URI);
    expect(sent.get("client_id")).toBe("atlassian-client-id");
    expect(sent.get("client_secret")).toBe("atlassian-secret");

    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(await res.json()).toEqual({ access_token: "tok", token_type: "bearer" });
  });

  it("2. basic-auth provider (bitbucket): sends Authorization: Basic header, no client_secret field in body", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));

    await handleRequest(
      postToken("bitbucket", {
        grant_type: "authorization_code",
        code: "auth-code-456",
        code_verifier: "verifier-456",
        redirect_uri: "http://127.0.0.1:8976/plugin-oauth/bitbucket/profile/default/callback",
      }),
      ENV,
    );

    const headers = upstreamHeaders(fetchMock);
    const authHeader = headers.get("authorization");
    expect(authHeader).toBeTruthy();
    const decoded = atob((authHeader ?? "").replace(/^Basic\s+/i, ""));
    expect(decoded).toBe("bitbucket-client-id:bitbucket-secret");

    const sent = await upstreamFormBody(fetchMock);
    expect(sent.has("client_secret")).toBe(false);
  });

  it("3. caller-supplied client_id/client_secret are overridden/dropped", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));

    await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-789",
        code_verifier: "verifier-789",
        redirect_uri: VALID_REDIRECT_URI,
        client_id: "attacker-client-id",
        client_secret: "attacker-secret",
      }),
      ENV,
    );

    const sent = await upstreamFormBody(fetchMock);
    expect(sent.get("client_id")).toBe("atlassian-client-id");
    expect(sent.get("client_secret")).toBe("atlassian-secret");
  });

  it("4. unexpected extra form keys are never forwarded", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));

    await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-abc",
        code_verifier: "verifier-abc-2",
        redirect_uri: VALID_REDIRECT_URI,
        scope: "admin",
        audience: "evil",
      }),
      ENV,
    );

    const sent = await upstreamFormBody(fetchMock);
    expect(sent.has("scope")).toBe(false);
    expect(sent.has("audience")).toBe(false);
  });

  it("5. redirect_uri not matching the loopback shape -> 400, upstream never called", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-xyz",
        code_verifier: "verifier-xyz",
        redirect_uri: "https://evil.example.com/callback",
      }),
      ENV,
    );

    expect(res.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("6. grant_type=client_credentials -> 400, upstream never called", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "client_credentials",
      }),
      ENV,
    );

    expect(res.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("7. refresh_token happy path: forwards refresh_token + injected creds, drops code/redirect_uri even if sent", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok2" }), { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "refresh_token",
        refresh_token: "refresh-abc",
        code: "should-be-dropped",
        redirect_uri: VALID_REDIRECT_URI,
      }),
      ENV,
    );

    const sent = await upstreamFormBody(fetchMock);
    expect(sent.get("grant_type")).toBe("refresh_token");
    expect(sent.get("refresh_token")).toBe("refresh-abc");
    expect(sent.get("client_id")).toBe("atlassian-client-id");
    expect(sent.get("client_secret")).toBe("atlassian-secret");
    expect(sent.has("code")).toBe(false);
    expect(sent.has("redirect_uri")).toBe(false);
    expect(res.status).toBe(200);
  });

  it("8a. unknown provider -> 404", async () => {
    const res = await handleRequest(postToken("not-a-real-provider", { grant_type: "refresh_token", refresh_token: "x" }), ENV);
    expect(res.status).toBe(404);
  });

  it("8b. unconfigured provider (missing secret in env) -> 503, without revealing which var", async () => {
    const partialEnv: Env = { ...ENV, ATLASSIAN_CLIENT_SECRET: undefined };
    const res = await handleRequest(postToken("atlassian", { grant_type: "refresh_token", refresh_token: "x" }), partialEnv);
    expect(res.status).toBe(503);
    const bodyText = JSON.stringify(await res.json());
    expect(bodyText).not.toContain("ATLASSIAN_CLIENT_SECRET");
  });

  it("8c. GET /token/atlassian -> 405 with Allow: POST", async () => {
    const req = new Request("https://relay.test/token/atlassian", { method: "GET" });
    const res = await handleRequest(req, ENV);
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("POST");
  });

  it("8d. wrong content-type -> 415", async () => {
    const req = new Request("https://relay.test/token/atlassian", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ grant_type: "refresh_token", refresh_token: "x" }),
    });
    const res = await handleRequest(req, ENV);
    expect(res.status).toBe(415);
  });

  it("8e. prototype-chain provider key (__proto__) -> 404, not 503; upstream never called", async () => {
    // PROVIDERS[key] with a plain index access resolves __proto__/constructor/toString to an
    // inherited Object.prototype value, which is truthy and would fall through to the 503
    // "provider_unconfigured" branch instead of correctly 404ing as an unknown provider.
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));
    const res = await handleRequest(postToken("__proto__", { grant_type: "refresh_token", refresh_token: "x" }), ENV);
    expect(res.status).toBe(404);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("9. upstream error response is passed through unchanged, and the code is never logged", async () => {
    const logSpy = spyOn(console, "log");
    stubFetch(async () => new Response(JSON.stringify({ error: "invalid_grant" }), { status: 400 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "super-secret-code-value",
        code_verifier: "verifier-super-secret",
        redirect_uri: VALID_REDIRECT_URI,
      }),
      ENV,
    );

    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ error: "invalid_grant" });

    const loggedText = logSpy.mock.calls.map((args) => args.join(" ")).join("\n");
    expect(loggedText).not.toContain("super-secret-code-value");
    logSpy.mockRestore();
  });
});

describe("GET /health", () => {
  it("returns 200 {ok:true}", async () => {
    const res = await handleRequest(new Request("https://relay.test/health"), ENV);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });
});

describe("unknown routes", () => {
  it("404s any other path", async () => {
    const res = await handleRequest(new Request("https://relay.test/nope"), ENV);
    expect(res.status).toBe(404);
  });
});

describe("body size cap", () => {
  it("rejects bodies over 4KB with 413, upstream never called", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));
    const huge = `grant_type=refresh_token&refresh_token=${"a".repeat(5000)}`;
    const res = await handleRequest(postToken("atlassian", huge), ENV);
    expect(res.status).toBe(413);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("rate limiting (guard on absence)", () => {
  it("skips limiting when the binding is absent from env (tests / wrangler dev)", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));
    const res = await handleRequest(postToken("atlassian", { grant_type: "refresh_token", refresh_token: "x" }), ENV);
    expect(res.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("returns 429 and never calls upstream when the binding denies the request", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));
    const limiter: Env["OAUTH_RELAY_RATE_LIMITER"] = {
      limit: mock(async () => ({ success: false })),
    } as unknown as Env["OAUTH_RELAY_RATE_LIMITER"];

    const res = await handleRequest(
      postToken("atlassian", { grant_type: "refresh_token", refresh_token: "x" }, { "cf-connecting-ip": "203.0.113.7" }),
      { ...ENV, OAUTH_RELAY_RATE_LIMITER: limiter },
    );

    expect(res.status).toBe(429);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("proceeds when the binding allows the request", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));
    const limiter: Env["OAUTH_RELAY_RATE_LIMITER"] = {
      limit: mock(async () => ({ success: true })),
    } as unknown as Env["OAUTH_RELAY_RATE_LIMITER"];

    const res = await handleRequest(postToken("atlassian", { grant_type: "refresh_token", refresh_token: "x" }), {
      ...ENV,
      OAUTH_RELAY_RATE_LIMITER: limiter,
    });

    expect(res.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

describe("redirect_uri gate (table-driven near-misses)", () => {
  // Each of these is a plausible bypass shape for the loopback-callback allowlist: userinfo
  // smuggling, trailing/traversal path tricks, encoded/real newline suffixes, off-by-one host
  // and port, and a scheme/case mismatch. Every one must be rejected with 400 *before* the
  // upstream token endpoint is ever called — a false accept here would leak the client secret
  // exchange to an attacker-controlled redirect target.
  const HOSTILE_REDIRECT_URIS: Array<[string, string]> = [
    [
      "userinfo smuggling (@evil.com after the loopback authority)",
      "http://127.0.0.1:8976@evil.com/plugin-oauth/atlassian/profile/atlassian-cloud/callback",
    ],
    ["trailing slash", "http://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback/"],
    ["encoded newline suffix (%0A)", "http://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback%0A"],
    ["real newline suffix", "http://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback\n"],
    ["wrong port (8977)", "http://127.0.0.1:8977/plugin-oauth/atlassian/profile/atlassian-cloud/callback"],
    ["confusable host (127.0.0.10)", "http://127.0.0.10:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback"],
    ["uppercase scheme (HTTP://)", "HTTP://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback"],
    ["empty plugin segment", "http://127.0.0.1:8976/plugin-oauth//profile/atlassian-cloud/callback"],
    ["path traversal segment (..)", "http://127.0.0.1:8976/plugin-oauth/../atlassian/profile/atlassian-cloud/callback"],
    ["https scheme instead of http", "https://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback"],
  ];

  for (const [label, redirectUri] of HOSTILE_REDIRECT_URIS) {
    it(`rejects with 400, upstream never called: ${label}`, async () => {
      const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));

      const res = await handleRequest(
        postToken("atlassian", {
          grant_type: "authorization_code",
          code: "auth-code-near-miss",
          code_verifier: "verifier-near-miss",
          redirect_uri: redirectUri,
        }),
        ENV,
      );

      expect(res.status).toBe(400);
      expect(fetchMock).not.toHaveBeenCalled();
    });
  }

  it("positive control: the exact valid loopback shape passes the gate (proves the table isn't just rejecting everything)", async () => {
    const fetchMock = stubFetch(async () => new Response(JSON.stringify({ access_token: "tok" }), { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-positive-control",
        code_verifier: "verifier-positive-control",
        redirect_uri: "http://127.0.0.1:8976/plugin-oauth/atlassian/profile/atlassian-cloud/callback",
      }),
      ENV,
    );

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(res.status).toBe(200);
  });
});

describe("code_verifier requirement on authorization_code grants", () => {
  it("missing code_verifier -> 400, upstream never called (Cockpit always sends PKCE)", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-no-verifier",
        redirect_uri: VALID_REDIRECT_URI,
      }),
      ENV,
    );

    expect(res.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("empty code_verifier -> 400, upstream never called", async () => {
    const fetchMock = stubFetch(async () => new Response("{}", { status: 200 }));

    const res = await handleRequest(
      postToken("atlassian", {
        grant_type: "authorization_code",
        code: "auth-code-empty-verifier",
        code_verifier: "",
        redirect_uri: VALID_REDIRECT_URI,
      }),
      ENV,
    );

    expect(res.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("upstream response header filtering", () => {
  it("strips Set-Cookie and x-ratelimit-* from the upstream response, keeps only content-type + cache-control", async () => {
    const fetchMock = stubFetch(
      async () =>
        new Response(JSON.stringify({ access_token: "tok" }), {
          status: 200,
          headers: {
            "set-cookie": "session=evil-upstream-cookie; HttpOnly",
            "x-ratelimit-remaining": "3",
            "x-ratelimit-reset": "60",
          },
        }),
    );

    const res = await handleRequest(postToken("atlassian", { grant_type: "refresh_token", refresh_token: "refresh-header-test" }), ENV);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(res.headers.get("set-cookie")).toBeNull();
    expect(res.headers.get("x-ratelimit-remaining")).toBeNull();
    expect(res.headers.get("x-ratelimit-reset")).toBeNull();
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(res.headers.get("cache-control")).toBe("no-store");
  });
});
