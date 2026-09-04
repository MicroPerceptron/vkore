# Generic capability-scoped agents

vKore uses the conventional model/tool loop without treating model output as
authority. The broker model proposes a task-local capability set from bounded,
policy-filtered metadata. Trusted vKore and Kore code validate that proposal,
and Kore resolves the selected opaque references only against authority the
resident broker already holds. The worker model then alternates between a
structured action and a typed result until it answers, refuses, is cancelled,
or exhausts its budget.

```text
authenticated intent + policy ceiling
              |
              v
bounded resource and tool metadata
              |
              v
untrusted broker proposal (zero, one, or many selectors)
              |
              v
host validation + Kore validation + affine capability derivation
              |
              v
private runner namespace and counted tool manifest
              |
              v
worker decision -> validated Task-C invocation -> typed result -> worker
```

Structured output is an effects boundary, not a reasoning policy. A worker may
answer directly with no tool. If it calls a tool, its command and typed
arguments must match a descriptor in the current `available_tools` catalog.
After each result the catalog is regenerated from the live namespace, so use
limits and revocation take effect without restarting the conversation. No
private chain-of-thought is required, retained, or trusted; public replies,
structured calls, results, authority changes, and lifecycle events are the
auditable record.

## Catalog disclosure

Catalog entries are inert metadata. They never contain host paths, file
descriptors, capability slots, task pointers, or usable endpoints. Resource
labels are exposed only when authenticated project policy allows the model to
learn that name. Credential-like and excluded paths remain absent. A broad
workspace view is offered only when the standing policy authorizes that scope,
typically for editing or bounded discovery; an unambiguous read should use an
exact object grant. List, find, and search execute within the worker's current
filesystem view and therefore cannot reveal inaccessible names, counts,
snippets, scores, or timing from outside that view.

Executable offers include bounded semantic display metadata so an unfamiliar
tool name can be selected without a command-name branch. The runner receives a
separate versioned descriptor containing the local name, usage, summary,
positional argument schema, effects, result sensitivity, and remaining uses.
The descriptor describes an already-derived endpoint; copying or inventing it
does not create one.

## Validation and lifecycle

The broker transport accepts schema-valid zero/one/many selections, but trusted
validation still enforces offer membership and kind, duplicate rejection,
rights attenuation, generation and lineage, attachment bounds, and the
authenticated budget ceiling. Kore applies the accepted plan atomically. A
failure before publication rolls back every staged face, grant, endpoint,
channel, route, mapping, task, and budget owner.

Task A sends Task B a versioned, counted manifest. Task B binds zero or more
tools, rejects duplicate names or unsupported schemas, validates each call,
re-resolves the executable immediately before invocation, and derives Task C's
filesystem capability from its current private face. Generic install and
revoke messages transfer the exact endpoint out of band and identify the live
grant/generation. Revocation removes the namespace entry and cuts cached,
descendant, and in-flight authority at the service boundary.

A tool result is untrusted data. It may suggest another action but cannot grant
or expand authority. A missing capability can produce a typed request for
authority; only authenticated user intent, standing policy, or an authorized
parent decision can approve it. Prompt injection therefore cannot turn a
guessed path or tool name into access.

## Deterministic witnesses

The deterministic backend and guest are conformance oracles, not production
planning policy. Their historical `X`/`Y`/`V` and prompt-injection scripts
exercise isolation, revocation, and stale-generation behavior. Production
llama.cpp prompts and validators contain no resolution table for those names,
for greetings, or for particular commands. Generic tests use arbitrary tool
names and typed schemas so literal coupling cannot satisfy them.
