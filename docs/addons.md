# vKore add-ons

An add-on is a separately packaged collection of typed tools. Installing or
registering one makes it visible to the vKore product; it does not grant an AI
agent permission to invoke it.

## Trust and execution

Every tool has one manifest-selected execution domain:

- `host` runs as a fresh, bounded child process for service APIs and local app
  integrations.
- `brokered_process` is the host entrypoint used for credentialed services. The
  child emits a typed, credential-free request plan; vKore validates its exact
  connection, destination, method, path, query, body, and bounds before the
  credential broker attaches a token and performs HTTPS.
- `guest` names a content-addressed Kore executable for operations that depend
  on the agent's isolated workspace or environment.
- `split` performs a bounded host phase, then passes only an explicitly granted
  result handle to a Kore executable.

The add-on author chooses the domain and declares effects, destinations,
connections, limits, sensitivity, and minimum mediation. The worker model
cannot change these values. Host add-ons receive an empty environment, a
per-invocation scratch directory, bounded input/output, and no credentials.
Service credentials should be used by a vKore credential broker rather than
placed in the child environment. `HostRuntime` checks declared network
destinations against its configured allow-list; production packaging must also
place third-party processes in the platform's OS-level network/filesystem
sandbox, because process isolation alone cannot constrain malicious native code.

## Manifest

Create an `AddonManifest` with `manifest_version = 1` and
`sdk_protocol = 1`. IDs are stable ASCII identifiers. A package hash is the
lowercase BLAKE3 digest of the installed executable or guest artifact. Tools
use JSON Schema-shaped input and result descriptions and must declare hard
byte/time limits.

Destructive tools must require `user_confirm`. Policy may strengthen an
add-on's `minimum_mediation`, but cannot weaken it.

See `vkore-reference-addon` for complete host and guest manifests.

## Invocation flow

1. vKore loads and validates the manifest and content hash.
2. The UI may show every installed tool. The broker candidate catalog is
   filtered by the user's maximum policy.
3. Kore installs a `ToolCapability` for an exact add-on version/hash, tool,
   agent generation, prompt, argument/resource constraints, expiration, and
   call budget.
4. vKore creates an invocation ticket over the complete invocation record.
5. `AuthoritySet::validate_and_consume` verifies the ticket, live grant,
   generation, prompt, package, schema, normalized resources, arguments,
   limits, mediation, replay state, and revocation state.
6. Only the returned `ValidatedInvocation` can enter `HostRuntime`. Guest tools
   use the equivalent versioned HostLink record and Kore's executable service.
7. The add-on returns a `ResultHandle`. Disclosure is a separate capability;
   `DisclosureGrant` selects only approved fields.

If broker or user review is required, validation returns `MediationRequired`
without consuming the call. The trusted vKore policy path then repeats the
validation with the completed mediation level. Model output, tool output, web
pages, and workspace text can propose a call but cannot produce that trusted
mediation state.

## Child protocol

Host add-ons read one JSON `InvocationApproval` from standard input and write
one JSON `InvocationResult` to standard output. They must not write logs to
stdout. Stderr is treated as untrusted diagnostics and is not disclosed to the
agent. The process is terminated at its timeout. Result identity must match the
invocation ID, agent generation, and package hash.

A brokered host add-on writes one JSON `ServiceRequest` instead. That record
cannot contain headers, credentials, or an arbitrary URL. `HostRuntime` accepts
it only from a validated `BrokeredProcess`, requires its connection to equal the
invocation's capability-checked `connection` resource, and requires its
tool identity to equal the capability-checked invocation. Its connection kind
and destination must appear in the installed tool manifest. A service-specific
broker additionally binds each tool to an exact method/path/query/body shape,
preventing a read tool from becoming a confused deputy for a write. The trusted
`ServiceBroker` returns bounded JSON which vKore exposes only through the
invocation's disclosure fields.

`CancellationRecord` and `ToolRevocation` are generation-bound records for
closing in-flight work and preventing new calls. A revoked tool disappears from
subsequent worker catalogs immediately.

## Reference implementation

`vkore-reference-addon` provides:

- `reference.host.echo`, an out-of-process host tool; and
- `reference.guest.list_workspace`, a guest descriptor that can only run via a
  Kore executable capability.

Run its SDK and adversarial tests with:

```text
cargo test -p vkore-addon-sdk -p vkore-reference-addon
```

The tests cover forged tickets and tool names, changed arguments, extra
prompt-injected fields, stale generations, result disclosure, expiry, replay,
revocation, mediation, package identity, bounded output, and prevention of
guest execution on the host.

## Git add-on

`vkore-git-addon` provides a useful review-before-commit workflow against one
exact, canonical Git worktree root:

- `git.status`, `git.diff`, `git.log`, and `git.show` are read-only;
- `git.stage` and `git.unstage` accept only explicit repository-relative paths
  and require authenticated user confirmation;
- `git.commit` commits only the already-staged index, requires an explicit
  author and message, requires user confirmation, and disables hooks and commit
  signing.

Arguments are passed directly to Git without a shell. Global and system Git
configuration, terminal prompting, pagers, fsmonitor, external diff drivers,
and textconv are disabled. Repository-local clean filters may still run during
staging, so production packaging must place the process in the OS sandbox
described above. The process never receives network authority.

A normal code workflow is: Kore edits the capability-scoped workspace; the Git
add-on reports `status`; the agent requests `diff`; the user approves exact
paths for `stage`; the staged `diff` is reviewed; then the user approves
`commit`. Checkout, reset, force push, credential use, and remote operations are
intentionally absent.

## Gmail add-on

`vkore-gmail-addon` provides:

- `gmail.search` and `gmail.read` with per-call mediation;
- `gmail.create_draft` and `gmail.send` with authenticated user confirmation;
- `gmail.trash` as a destructive, user-confirmed operation.

The add-on emits only requests to `gmail.googleapis.com:443`. Search results
are bounded to 100 messages, message identifiers are validated before entering
a URL path, email addresses and subjects reject header injection, outbound
messages are URL-safe base64 MIME, redirects are disabled, and responses are
bounded before JSON decoding. `GmailCredentialBroker` keeps caller-supplied
access tokens only in memory, zeroizes them on disconnect/drop, checks the
declared OAuth scopes for every operation, and attaches the bearer header after
the add-on process exits. The add-on process, model, transcript, and project
configuration never receive the token.

The narrow supported scopes are `gmail.readonly`, `gmail.send`,
`gmail.compose`, and `gmail.modify`; the full-mail scope is accepted when a user
has deliberately connected it. OAuth browser consent, refresh-token storage,
and refresh are connection-provider responsibilities rather than add-on tool
calls.

Build either executable, calculate its lowercase BLAKE3 digest, and ask the
executable for its installable manifest:

```text
cargo build -p vkore-git-addon -p vkore-gmail-addon
target/debug/vkore-git-addon --manifest <BLAKE3-DIGEST>
target/debug/vkore-gmail-addon --manifest <BLAKE3-DIGEST>
```

Run the complete SDK and add-on tests with:

```text
cargo test -p vkore-addon-sdk -p vkore-reference-addon \
  -p vkore-git-addon -p vkore-gmail-addon
```
