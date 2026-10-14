# EventKit Decisions

Architectural decisions and their rationale, newest first. Earlier decisions
live in the CHANGELOG and in the doc comments they shaped (the per-thread store
cache and the never-`spawn_blocking` rule are on `EventKitServer`).

---

## 0.8.0 — Both Lifecycles From One Server, Up To A Named Ceiling

### Decision

The MCP server speaks 2026-07-28 and every earlier revision:

- `supported_protocol_versions()` is `known_up_to(&MAX_PROTOCOL_VERSION)`,
  with `MAX_PROTOCOL_VERSION` = 2026-07-28 written in the source.
- `initialize` answers `LATEST_WITH_INITIALIZE` (2025-11-25). rmcp gives that
  answer to a client that asks `initialize` for 2026-07-28, too.
- `server/discover` is answered only at a revision without a handshake; at a
  2025 version it is method-not-found. Its `supportedVersions` is every
  revision the server implements, the same list a `-32022` carries.

### Rationale

Agent Muse, which embeds this server in process, now offers 2026-07-28 as a
choice. Serving both lifecycles costs a 2025 client nothing.

The ceiling stays named rather than rmcp's `LATEST`, so an SDK bump cannot
move the protocol on its own.

The discover gate closes a trap in rmcp's default, which answers a probe at
any supported version and would let a client run the stateless lifecycle at a
revision that defines none.

`supportedVersions` first listed only the handshake-free revisions. It was
wrong: the version list a `-32022` gives and the one `discover` gives are one
fact, and the official suite's `sep-2575-server-unsupported-version-error`
holds a server to that. The gate on `discover` itself keeps a stateless client
off 2025.

---

## 0.8.0 — Tasks Return As The SEP-2663 Extension, Promoted Late, On The EventKit Thread

### Decision

The server offers `io.modelcontextprotocol/tasks` on both revisions
(`src/mcp_tasks.rs`). A `tools/call` becomes a task only when all of these
hold:

- the client declared the extension;
- the tool is one of eight (`TASK_TOOLS`): the v1 six, plus `request_access`
  and `get_current_location`, which wait on the user and the hardware;
- the call is still running after 1 second;
- admission allows it: at most 4 running and 256 held.

Otherwise the call is answered inline. A task lives 1 hour.

**Where the operation runs.** On the one EventKit thread (`mcp_thread.rs`),
like every call, spawned there by the task runtime with its `ToolRun` in
scope; only the bookkeeping (promotion, status, the request's token) stays on
rmcp's runtime. It runs under a token derived from the connection's lifetime
rather than the request's.

**Cancellation.** `tasks/cancel` cancels that token. A batch stops between
items (`run_batch`). A single EventKit call runs to its end.

### Rationale

**Promoted late.** SEP-2663 makes the server the sole decider. The calls worth
a task are the ones that would outlive a client's request timeout, which a
1-second run identifies; quick calls see no difference.

**One thread, a small cap.** EventKit stores are cached per thread, and a
process may hold only a few before EventKit refuses it ("too many
EKEventStore instances", `EKCADErrorDomain 1021`), so a task's tool runs where
every other call runs: on the EventKit thread. Calls take turns there, so a
promoted call holds the thread for as long as it blocks, which is why only 4
run at once. Past that, a slow call stays an ordinary request: no worse than
before. A tokio task-local does not follow work onto another runtime, so the
runtime sets the call's `ToolRun` on the spawned operation itself; without it
a batch's progress and its cancellation record would vanish
(`a_task_mode_call_runs_on_the_eventkit_thread`,
`a_promoted_task_runs_on_the_eventkit_thread_and_sees_its_run`).

**The token is not the request's.** rmcp cancels a request's token as it sends
that request's response, and a task's response is its handle. An operation on
the request's token would cancel itself at promotion.

**Between items, never mid-call.** An EventKit save or remove cannot be
interrupted part-way, so between items is the only safe stopping point.

**`cancelled` follows what the tool did.** A batch that saw the cancellation
settles `cancelled`, with a status message saying how far it got. A call that
could not stop finishes and settles `completed`.

### Consequence

A tool that panics fails its task: the operation runs in its own tokio task,
whose join error settles it. The 0.6.x task manager had the opposite bug,
where a panicking tool held its slot until the process exited.

Tasks live with the connection. Closing it cancels every operation token,
waits 2 seconds, then aborts what is left.

The three batch tools share one loop (`run_batch`), so cancellation and
progress live in one place. Item errors are built directly, instead of by
splitting a formatted string on its first `": "`.
