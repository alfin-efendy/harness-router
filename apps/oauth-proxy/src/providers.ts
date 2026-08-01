export type ClientAuth = "body" | "basic";

export type Provider = {
  /** The provider's REAL token endpoint. Hardcoded — never taken from the request. */
  tokenUrl: string;
  /** Public OAuth client id, from wrangler [vars]. */
  clientIdVar: string;
  /** Worker secret name holding the client secret. */
  clientSecretVar: string;
  /** How the provider expects client credentials on the token call. */
  clientAuth: ClientAuth;
};

export const PROVIDERS: Record<string, Provider> = {
  atlassian: {
    tokenUrl: "https://auth.atlassian.com/oauth/token",
    clientIdVar: "ATLASSIAN_CLIENT_ID",
    clientSecretVar: "ATLASSIAN_CLIENT_SECRET",
    clientAuth: "body",
  },
  bitbucket: {
    tokenUrl: "https://bitbucket.org/site/oauth2/access_token",
    clientIdVar: "BITBUCKET_CLIENT_ID",
    clientSecretVar: "BITBUCKET_CLIENT_SECRET",
    clientAuth: "basic",
  },
};
