# The operator agent (experimental)

`vkore-agent` lets a model operate one machine: look at it, read its console and
disk, type into its shell, start and stop it. The Tauri desktop shows it as the
**Agent** tile of a machine's workspace. The iced desktop and the CLI do not use
it yet.

## The protocol

The agent is a client of the OpenAI chat protocol, and uses a deliberately small
part of it, so that a model served by Kore later has little to implement:

- `POST {base}/chat/completions` with `model`, `messages`, `tools` and
  `stream: true`; an `Authorization: Bearer` header when there is a key.
- A `text/event-stream` answer: `choices[0].delta.content`,
  `delta.tool_calls` (with `index`, `id`, `function.name` and
  `function.arguments`, in pieces), `finish_reason`, then `data: [DONE]`.
- Function tools, and `role: "tool"` messages carrying `tool_call_id`.

Nothing else is sent: no sampling options, no token limit. The model needs no
`/models` endpoint. Other fields a server adds are ignored.

vKore's own local server ([inference.md](inference.md)) does not serve this yet:
it refuses `stream` and `tools`. Whether a Kore guest is later reached through
that server and a libkore-exported channel ([MIGRATION.md](../MIGRATION.md)) or
at an address of its own is Kore's decision; the agent only needs a base address.
Machines run with QEMU user networking and no port forwarding, so a server inside
a guest is not reachable from the host today.

## Configuration

Read once, when the app starts:

| Variable | Meaning |
| --- | --- |
| `VKORE_AGENT_BASE_URL` | The part before `/chat/completions`, usually ending in `/v1`. `https`, or `http` for this computer only. |
| `VKORE_AGENT_MODEL` | The model's name there. |
| `VKORE_AGENT_API_KEY` | The key, if the endpoint wants one. |

The key stays in the app's Rust process: it is sent only in the request header,
is removed from error messages, never reaches the web page or a log, and is taken
out of the environment once read, so QEMU and other programs the app starts do
not inherit it. It is not stored; set it again for each launch.

A local model works too, which is also how the loop was first tested:

```sh
llama-server -m model.gguf --jinja --host 127.0.0.1 --port 8080 -c 16384
```

```sh
VKORE_AGENT_BASE_URL=http://127.0.0.1:8080/v1 VKORE_AGENT_MODEL=local pnpm tauri dev
```

## What it may do

| Tool | Acts on | Approval |
| --- | --- | --- |
| `machine_status` | state, boot stage, hardware, newest boot events | none |
| `read_console` | the console's newest output (8 KiB) | none |
| `list_files`, `read_file` | a **stopped** machine's volume, through `vkore-volume`; text only, its first 16 KiB | none |
| `type_console` | one line of printable ASCII, at most 400 characters | asked |
| `start_machine`, `stop_machine` | as the window's buttons do | asked |

Which approvals may be remembered is `rule` in
[`crates/vkore-agent/src/rule.rs`](../crates/vkore-agent/src/rule.rs). The gate
holds a floor under it: a change is never made without a person having approved
it. A remembered approval is bound to one machine and one agent session, and ends
when the agent is turned off.

## What it keeps

- **Off until turned on**, for each machine, after saying where what it reads is
  sent. With a hosted model, console output, boot events and file contents leave
  this computer.
- **Text is never approval.** Console and file contents reach the model as tool
  results. A model that is talked into something by them can still only ask.
  Reading needs no approval, so such text can steer what the agent reads next.
- **What is approved is what runs.** Typed input is printable ASCII on one line,
  shown exactly; a call with arguments the tool does not take is refused. The
  approval buttons arm a moment after they appear.
- **Audit.** Each request and answer is appended to
  `<state>/agent/audit-<session>.jsonl` by `vkore-policy`, chained and signed with
  a key in the same folder (`audit.key`, owner-readable). This detects damage and
  edits by someone without the key; it does not protect against the account that
  owns the key.
- **Bounds.** 8 KiB prompts, 8 model replies a prompt, 8 tool calls and 64 KiB of
  text a reply, 256 KiB stream lines, 8 MiB a stream, a 96 KiB conversation
  (oldest whole turns dropped), and a transcript of 80 entries or 128 KiB.
- **No replay.** A failed or stopped request is reported, never retried.

## Testing against a real endpoint

With the variables above set:

```sh
VKORE_AGENT_LIVE=1 cargo test -p vkore-agent --test live -- --nocapture
```

The test asks the model to run a command on a pretend machine and then to recall
the result. Nothing real is touched.
