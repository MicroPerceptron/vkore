# Optional accounts and image download authorization

vKore starts and uses local machines, volumes, imported ISOs and its local HTTP
API without signing in. Cached images are inspected and their digest rechecked
before reuse, without requesting an account. Catalog refresh is optional network
work; catalog metadata is never download authorization.

## Using an account

Open **Settings → Account → Sign in with Google**. The browser opens the
first-party service. Enter the code displayed in vKore, sign in with a verified
Google account, read the agreement and explicitly confirm review and acceptance.
The Account card shows the server's acceptance version and timestamp. Review
agreement starts a new authorization, so changed terms can be accepted there.
Sign-out discards the local credential even when the server is unreachable.

Credentials live only in memory, for at most one hour (also capped by the
verified Google identity token's expiration). Restarting vKore requires a new
sign-in. No passwords, refresh tokens or download secrets are stored in machine
configuration, arguments, environment variables or preference files.

CLI users add `--sign-in` to `vkore machine create --image ID` or a first-party
`--iso https://dev.microperceptron.com/dl/ID/FILE.iso` request. It prints the
browser address and public code and waits for approval. Ctrl-C cancels. The flag
is unnecessary for local imports and verified cache reuse. Other URL origins,
redirects, and links with query credentials are refused: import a local file for
an image obtained elsewhere.

## Authority and protocol

The matching implementation lives in MicroPerceptron/placement. Its versioned
base is `https://dev.microperceptron.com/api/v1/download-account`:

| Method and path | Requirement | Result |
| --- | --- | --- |
| POST `/device` | None | Private `device_code`, public `user_code`, verification page, expiry, poll interval |
| GET `/review` | Verified Firebase Google ID token | EULA version, SHA-256, text, account and subject-bound review challenge |
| POST `/approve` | Same verified identity | Exact version/digest/challenge, `reviewed: true`, `accepted: true`, and device user code; saves acceptance before approval |
| POST `/token` | Private device code | 202 pending, 429 slow down, or one-time download session handoff |
| GET `/session` | Download bearer | Current identity/acceptance status or refusal |
| POST `/revoke` | Download bearer | Revokes that session |
| GET `/terms` | None | Public agreement text, version and digest |

The flow follows the separation of public user codes and private polling secrets
in [RFC 8628](https://www.rfc-editor.org/rfc/rfc8628); these are a deliberately
narrow JSON API, **not** a general OAuth authorization server. Google identity
verification uses Placement's existing
[Firebase Admin verifier](https://firebase.google.com/docs/auth/admin/verify-id-tokens).
No new cryptographic implementation or Rust dependency is introduced.

The receipt records stable Firebase subject, server timestamp, document version
and digest, and receipt ID. It proves explicit acknowledgement from an
authenticated account, not actual reading or comprehension. The trial EULA is
clearly marked as placeholder; replace its text and version together before
using final terms.

## Enforcement and rollout

Every new desktop and CLI transfer reaches the same runtime gate. It checks a
live server session before issuing the image GET. Credentials go only to the
fixed HTTPS origin, without redirects. Missing/unavailable/malformed account
responses fail closed. Cancellation and digest/size inspection remain in place.

The **server** is the security boundary: every GET, HEAD, range request and
checksum sidecar under `/dl` independently requires a current download session
backed by durable acceptance of the current EULA. Public catalog visibility,
legacy image tokens, signed links and administrator membership do not bypass it.
Existing copies of an image cannot be recalled by changing this gate.

Deploy the matching Placement changes before using this vKore release for
remote downloads. Old clients may no longer list/download protected images;
update them. Ensure reverse proxies/CDNs do not cache or serve `/dl` files
independently, and remove any previously cached anonymous downloads. No static
image storage mount may bypass the application router.

Placement's existing team and admin routes keep their domain restrictions.
The download account route accepts any verified Google account, and refuses
local development identities. Sessions are bounded and held in server memory;
a server restart signs clients out. The current single-process service must
use sticky routing or a shared session store before scaling to multiple workers.
Durable acceptance records remain in the server database across restarts.
