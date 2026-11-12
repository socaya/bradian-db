# Temporary Bradian v1.1 HTTP API bootstrap gate

The reviewed original router allows analytical read and write endpoints without route-level authentication. The initial hardening adds an administrator token gate covering `/api/analytics/v1/*` and DHIS2 compatibility operations. Only `GET /healthz` is public.

Generate a cryptographically strong token with at least 32 non-whitespace characters. Provision it as `BRADIAN_API_TOKEN` in the service environment through a secure secret manager. Never commit the token, embed it in the Studio frontend, copy it into logs or transmit it without secure transport. The server refuses non-loopback bindings until first-class TLS/mTLS and appropriate user authentication exist.

Usage during local development:

```sh
BRADIAN_API_TOKEN="<secure-value-injected-by-secret-manager>" cargo run -p dap-admin -- serve --root ./data --bind 127.0.0.1:8080
```

Call the API with `Authorization: Bearer <token>`. No token or unknown token produces 401; a recognized non-administrator produces 403. Keep legacy HTTP API paths for compatibility.

**Security limitation:** a bootstrap administrator can access all protected APIs. This is not user login, complete RBAC, production access control, certificate validation, token rotation, credential expiry or SQL-level row policy enforcement. The browser Studio cannot safely operate protected endpoints until a session-based identity service is implemented. Remote deployments require a new reviewed security implementation and penetration tests.
