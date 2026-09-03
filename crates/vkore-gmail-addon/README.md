# vKore Gmail add-on

Credential-brokered Gmail search, read, draft, send, and trash tools. The
process emits a credential-free request plan; `GmailCredentialBroker` owns the
in-memory OAuth access token and performs the bounded HTTPS call. See
[`docs/addons.md`](../../docs/addons.md) for scopes, mediation, packaging, and
connection responsibilities.
