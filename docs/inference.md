# Local inference API: first pass

Run `vkore serve` or open **Settings → Local API → Start API** on the desktop.
Configure an OpenAI-compatible client's base URL as `http://127.0.0.1:8080/v1`.
No API key is required; clients that require one may use a dummy local value.
The desktop's **Port** field defaults to 8080; **Save port** persists a value
from 1 to 65535, and **Start API** saves the field before binding. A change made
while running takes effect after **Stop API** and **Start API**. The running URL
always reflects the actual listener. The API never starts automatically.
`vkore serve --listen 127.0.0.1:PORT` independently chooses the CLI listener;
it does not use the desktop preference. Non-loopback bindings are refused. Ctrl-C or closing the desktop stops the listener and its requests.
If another instance owns the port, startup reports that error.

**This pass does not serve model inference yet.** The current production
backend advertises no models and valid chat requests return HTTP 503 with
`error.code = "backend_unavailable"`. Booting an ISO is not evidence that a
model is loaded or that an inference channel exists. This work makes no new
performance or SR-IOV validation claim.

## Supported HTTP surface

- `GET /v1/models`: OpenAI list envelope containing backend-advertised model IDs.
  With the disconnected backend, this is `{"object":"list","data":[]}`.
- `POST /v1/chat/completions`: text messages with `system`, `developer`, `user`
  or `assistant` roles; `model`; optional `temperature` (0–2), `top_p` (0–1),
  `max_tokens` or `max_completion_tokens` (1–32768), `n: 1`, `stream: false`.
  A connected backend supplies the assistant text, finish reason and measured
  token counts; vKore supplies the completion envelope, ID and timestamp.
- Errors use `{"error":{"message":...,"type":...,"param":null,"code":...}}`.
  Unknown models return 404 once a backend is available. Unknown fields and
  unsupported options are refused explicitly. Malformed JSON and oversized
  bodies return client errors in the same envelope.

This is a limited Chat Completions surface, not full OpenAI API compatibility.
Streaming, tool calls, structured output, multimodal messages, legacy
completions, embeddings and the Responses API are not implemented. No chat
history or agent workflow is required. Select non-streaming text chat in clients
when connecting this first pass.

```sh
curl http://127.0.0.1:8080/v1/models
curl http://127.0.0.1:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"your-model","messages":[{"role":"user","content":"Hello"}],"stream":false}'
```

## Ownership and bounds

`vkored::inference::Backend` is the host provider interface. It lists currently
usable models and accepts typed requests asynchronously. Its future must honor
all accepted options, avoid blocking the HTTP runtime, and cancel/reclaim guest
work when dropped. A timeout or lost response must never cause replay. The HTTP
adapter knows nothing about guest channels, console inference, VM management or
DagFS. A future implementation consumes Kore-owned exports through libkore;
this pass changes neither Kore nor libkore and invents no guest wire format.

The listener allows at most 32 connections, one HTTP/1 request per connection,
16 KiB HTTP parser buffers, 10 seconds for headers, a 300-second request deadline
and a 305-second connection deadline. JSON bodies are limited to 1 MiB, message
lists to 256 entries, model identifiers to 256 bytes, and completion text to
4 MiB. Backends must bound their own allocations before returning; checking
returned data is not a replacement for provider-side bounds. Connections beyond
the limit are closed without queueing. Request disconnects, deadlines and server
shutdown drop the request work. No requests or model output are logged.

Loopback is intended for trusted local applications. Requests with browser
Origin headers or non-local Host authorities are refused, and no CORS access is
enabled. This does not isolate mutually untrusted users on the same host; LAN
exposure, authentication and multi-user deployment require a separate design.

## Dependency choice

HTTP framing and JSON extraction use axum 0.8 over Hyper 1 and Tokio 1 rather
than a handwritten parser. Axum is MIT-licensed and forbids unsafe code; Hyper
and Tokio are maintained MIT infrastructure with platform/socket unsafe code.
Only HTTP/1, JSON and the required runtime/network features are enabled; no
HTTP/2, TLS server or websocket stack is added. Per-connection buffers and the
number of concurrent connection tasks are explicitly bounded. The existing
ureq 2/native-tls dependency handles outbound HTTPS image transfers; URL parsing
and temporary-file publication reuse the existing url and tempfile families.

Protocol and lifecycle tests use an isolated test backend to verify completion
envelopes. That backend is never available in production and is not guest
inference evidence. Real image boot and model round-trip acceptance remain
required once a suitable image and exported channel are available.
