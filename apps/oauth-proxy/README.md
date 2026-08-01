# @ryuzi/oauth-proxy

A small Cloudflare Worker that performs the OAuth **token exchange** on behalf of Ryuzi
Cockpit, for providers whose token endpoint requires confidential-client credentials
(currently Atlassian and Bitbucket).

Ryuzi Cockpit is a desktop app distributed from a public repo, so it cannot carry a
confidential `client_secret` — not in the repo, and not in a signed plugin bundle. This
Worker holds those secrets server-side and performs only the back-channel token exchange on
the app's behalf. The browser authorize/consent step is unaffected: it still goes directly
from the user's browser to the provider. Only the POST that trades a code (or refresh token)
for tokens is relayed through here.

## Deploy runbook

Run these from `apps/oauth-proxy/` unless noted otherwise — that's where `wrangler.toml`
lives, and `wrangler login` / `wrangler deploy` / `wrangler secret put` all expect to be run
from there.

1. From the repo root, move into the Worker directory and install dependencies:

   ```sh
   cd apps/oauth-proxy
   bun install
   ```

2. Authenticate Wrangler with your Cloudflare account:

   ```sh
   bunx wrangler login
   ```

3. First deploy:

   ```sh
   bunx wrangler deploy
   ```

   On first use, Cloudflare will ask you to pick an account subdomain (e.g.
   `your-team.workers.dev`). The command prints the final Worker URL when it finishes:

   ```
   https://ryuzi-oauth-relay.<subdomain>.workers.dev
   ```

4. Set the secrets. These are **never committed** — they live only as Worker secrets, set
   interactively:

   ```sh
   bunx wrangler secret put ATLASSIAN_CLIENT_SECRET
   ```

   Paste the secret value when prompted. Repeat for Bitbucket once that OAuth consumer
   exists:

   ```sh
   bunx wrangler secret put BITBUCKET_CLIENT_SECRET
   ```

5. Verify the deploy:

   ```sh
   curl https://ryuzi-oauth-relay.<subdomain>.workers.dev/health
   ```

   A healthy Worker responds `200 {"ok":true}`.

6. Hand back the printed URL. A separate change points the plugin manifests' `token-url` at
   this Worker — `<url>/token/atlassian` and `<url>/token/bitbucket` — followed by a new
   signed plugin release. The relay does **not** take effect for an already-installed plugin
   until that plugin updates.

## Security model

- The relay only performs the back-channel token exchange (`authorization_code` and
  `refresh_token` grants). It never participates in the browser authorize/consent step.
- Upstream token endpoints are hardcoded per provider in `src/providers.ts` — never taken
  from the incoming request.
- Only an allowlisted set of form fields is ever forwarded upstream
  (`grant_type`, `code`, `code_verifier`, `redirect_uri`, `refresh_token`); anything else the
  caller sends is silently dropped.
- `redirect_uri` must match the Cockpit loopback callback shape
  (`http://127.0.0.1:8976/plugin-oauth/<plugin>/profile/<profile>/callback`); requests with any
  other `redirect_uri` are rejected before the secret is ever used.
- Client secrets live only as Worker secrets (`wrangler secret put`) — never in this repo,
  never in `wrangler.toml`, never in a plugin bundle.
- No CORS headers are returned. This Worker is called by the desktop app's backend, not by
  browser JavaScript, so there is no `Access-Control-Allow-Origin` — a drive-by web page
  cannot read a response from it.
- Nothing sensitive is logged: request bodies, authorization codes, code verifiers, refresh
  tokens, issued tokens, and client secrets are never written to logs. Only the provider key
  and the outcome status (or a generic transport-error message) are logged.
