# vKore add-ons

An add-on is a separately packaged collection of typed tools. Installing or
registering one makes it visible to the vKore product; it does not grant an AI
agent permission to invoke it.

## Trust and execution

Every tool has one manifest-selected execution domain:

- `host` runs as a fresh, bounded child process for service APIs and local app
  integrations.
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
