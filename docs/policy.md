# User approvals and standing policy

vKore records authenticated user decisions and converts them into bounded
authority ceilings. It does not mint Kore capabilities. The resident Kore
broker independently verifies its live holdings, the target generation, uses,
expiry, and the complete transaction before any authority is installed.

## Decisions

Approval requests preserve whether a proposal came from authenticated user
intent, a parent agent, or an untrusted worker. The UI presents the current and
requested scopes as a structural diff, including files, tools, accounts,
recipients, network destinations, budgets, delegation, duration, and the action
that will resume.

Supported decisions are deny, allow once, allow for a tool call, prompt,
session, or deadline, create a narrow standing rule, always ask, and immediate
revocation. A partial approval must be a mathematical subset of the request; it
produces a newly constrained grant instead of blessing the original proposal.

Destructive and external effects remain marked in the scope. Policy can require
confirmation but cannot erase an add-on's minimum mediation requirement.

## Authentication and storage

Model output, tool results, files, web pages, and transcript text cannot approve
a request. A local UI action is authenticated over its exact action, session,
ID, issue time, and expiry. It is single-use. Approval receipts bind the exact
request, agent generation, policy revision, scope, expiry, and use count without
serializing a Kore capability or reusable endpoint.

Policy data is separate from conversation history. `PolicyStore` uses a keyed,
append-only JSON-lines audit chain. Sequence gaps, edits, reordering, the wrong
key, malformed state, and unsupported schemas fail closed. The current policy
view is rebuilt from that journal after restart, so a disconnected UI cannot
silently approve or discard a pending request.

Policy export is reviewable JSON. Import requires an authenticated user action
bound to the exact file digest. No credentials or guest capabilities appear in
either representation.

## Matching and commitment

Standing rules match normalized structural scopes. Exact resources and
operations, recursive directory ceilings, add-on/tool/argument restrictions,
accounts, exact recipients or explicit email domains, destinations, budgets,
and delegation limits must all fit. Explicit deny wins over allow, and
`always ask` prevents automatic use. Non-matches explain that no policy
contains the complete requested authority.

Matching a standing allow records its use and creates a short-lived,
generation-bound grant. `AuthorityTransport` sends only a digest-bound request
ceiling through the HostLink authority path. The committed/refused Kore result
is recorded and shown. Revocation is authenticated, sent immediately, and
removes the materialized host grant or rule while retaining its audit history.

The reusable dashboard renderer escapes all untrusted text and shows pending
requests, active grants, policies, recent decisions, and revoke controls. The
interactive product wires this view to loopback-authenticated endpoints.
