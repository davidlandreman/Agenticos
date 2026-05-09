---
date: 2026-05-08
topic: feat-kernel-mcp-debug-bridge
---

# Kernel-Resident Tool Registry Exposed to Host as MCP

## Summary

Build a kernel-resident tool registry whose tools are callable from the host via a serial-port bridge that speaks MCP, so Claude can drive the running OS — screenshot it, run shell commands, send synthesized input, read serial output, and snapshot kernel state — without ad-hoc instrumentation or screenshot-paste loops.

---

## Problem Frame

The current dev loop for working on AgenticOS in Conductor + Claude looks like this: Claude adds temporary `dbg!()` or log lines in the kernel, the user rebuilds and reruns under QEMU, the user (or Claude) tails serial output for a while waiting for the relevant event, and then the diff gets reverted. For UX issues — wrong pixels, misbehaving windows, misaligned cursors — the user has to take a screenshot of the QEMU window manually and paste it into chat. Both loops are slow, both pollute the working tree with throwaway code or screenshots, and both depend on the user being in the chair to relay information that the running kernel could surface itself.

Beyond the immediate pain, the project's name and stated direction ("agentic" computing) imply that, eventually, agents will need a structured way to act on the OS — both from outside (driving / observing) and from in-OS apps (calling each other and the kernel). Today there is no such structured surface; everything goes through manual relay or shell-only invocations.

---

## Actors

- A1. **Claude (host-side)** — calls MCP tools to introspect or drive the running OS during development and testing.
- A2. **MCP bridge process (host-side)** — translates MCP requests from Claude into kernel RPC requests over a serial chardev, and translates kernel responses back. Lives outside QEMU, alongside it.
- A3. **Kernel tool registry (in-OS)** — owns the canonical `Tool` abstraction, holds the registered tool implementations, and answers RPC requests over the dedicated serial channel.
- A4. **Developer (human)** — runs `./build.sh` (or equivalent), occasionally invokes the bridge directly to sanity-check, but normally only experiences this through Claude.

---

## Key Flows

- F1. **Visual UX bug investigation**
  - **Trigger:** Developer reports a visual bug to Claude ("the window border is one pixel off after dragging").
  - **Actors:** A1, A2, A3.
  - **Steps:** Claude calls `screenshot` via MCP. Bridge forwards the request over serial. Kernel captures the framebuffer and returns raw pixel bytes. Bridge encodes to PNG and returns to Claude. Claude inspects the image and proposes a fix. Iterates without the developer ever opening the QEMU window.
  - **Outcome:** Claude has direct visual feedback; no manual screenshot-paste step.
  - **Covered by:** R3, R7, R10.

- F2. **Reproduce and observe a flaky kernel scenario**
  - **Trigger:** Claude is investigating a panic that happens after some shell sequence.
  - **Actors:** A1, A2, A3.
  - **Steps:** Claude calls `shell_run("ls /host")`, reads the captured stdout, calls `read_serial(timeout)` to drain log output, calls `kernel_state("heap")` to look at allocator state, then calls `send_input(...)` to drive the next step. No `dbg!()` lines were added to the kernel.
  - **Outcome:** A scenario is reproduced and observed end-to-end through tool calls instead of source edits.
  - **Covered by:** R3, R4, R5, R6, R10.

- F3. **Failure: kernel is unresponsive**
  - **Trigger:** Kernel is panicked, halted, or stuck in a tight loop; it cannot service RPC.
  - **Actors:** A1, A2.
  - **Steps:** Claude issues an MCP call. Bridge attempts to write to the serial chardev and waits for a response. After a bounded timeout, the bridge returns a structured error to Claude ("kernel did not respond within Nms; serial may be unattached or kernel halted"). Claude does not hang.
  - **Outcome:** Tool calls fail loudly and quickly when the kernel is not answering.
  - **Covered by:** R8, R12.

---

## Requirements

**Kernel-side tool registry**
- R1. The kernel exposes a `Tool` abstraction with at least: a stable name, a human-readable description, a structured input schema (or equivalent argument descriptor), and a `call` entry point that returns a structured result or a structured error.
- R2. Tools are registered into a single in-kernel registry. The registry can enumerate its tools (name + description) so the bridge can advertise capabilities to MCP clients without hardcoding a list.
- R3. v1 ships with these tools registered: `screenshot`, `shell_run`, `read_serial`, `send_input`, `kernel_state`.
- R4. `shell_run` reuses the existing shell command dispatcher (`src/commands/`) and returns the captured stdout as part of its structured result. It does not introduce a parallel command path.
- R5. `kernel_state` accepts a discriminator argument (e.g., `windows`, `processes`, `heap`) and returns a structured snapshot for that subsystem. Adding a new discriminator is additive — it does not change the tool's external identity or break older callers.
- R6. `send_input` synthesizes keyboard and/or mouse events into the same input pipeline that hardware-driven input feeds. Tools observing input downstream cannot tell the difference.
- R7. `screenshot` returns the framebuffer contents as raw bytes plus enough metadata (width, height, pixel format) for the host bridge to encode a PNG. The kernel does not perform PNG encoding.

**RPC transport (kernel ↔ host bridge)**
- R8. The kernel exposes the registry over a dedicated chardev distinct from the existing `-serial stdio` log channel. The existing log channel keeps its current role unchanged.
- R9. The wire protocol is request/response and message-framed (one tool call → one response). Streaming and long-lived sessions are not supported in v1.
- R10. The kernel-side dispatcher reads a request, invokes the named tool, and writes a response. Errors during dispatch (unknown tool, malformed request, tool failure) return a structured error rather than panicking.
- R11. Binary payloads (e.g., framebuffer bytes) can be transported without losing their semantics — the wire protocol either is binary-safe or the bridge handles encoding/decoding transparently.

**Host-side MCP bridge**
- R12. A host-side process implements the MCP protocol and translates each MCP tool call into one kernel RPC request. The bridge is a separate process that can be started and stopped independently of QEMU.
- R13. The bridge advertises tools to MCP clients dynamically by querying the registry (R2), so adding a tool kernel-side does not require a bridge code change.
- R14. The bridge surfaces transport-level failures (timeout, broken socket, kernel halted) as structured MCP errors with enough detail to distinguish them from tool-level errors.

**Architecture and future-fitness**
- R15. The `Tool` abstraction (R1) is designed so the underlying transport can later be swapped from a serial chardev to virtio-serial without changing tool implementations.
- R16. The registry is designed so a future in-OS consumer (e.g., an in-OS app) can call tools through an in-kernel entry point rather than through serial — a second consumer of the same registry, not a parallel registry.
- R17. The feature is built unconditionally into the kernel for the everyday dev loop. It is not gated behind `--features test`.

---

## Acceptance Examples

- AE1. **Covers R5.** Given the OS is booted with a window manager and one terminal window, when Claude calls `kernel_state("windows")`, the response is a structured snapshot of the window tree including at least each window's id, parent, position, and size.
- AE2. **Covers R8, R10.** Given the bridge is running and the kernel is healthy, when Claude calls a tool that does not exist, the kernel returns a structured "unknown tool" error and the bridge surfaces it as an MCP-level tool error — not as a transport error and not as a kernel panic.
- AE3. **Covers R8.** Given the new RPC chardev is added, when the OS boots, the existing `-serial stdio` log output (panic info, debug logs, shell prompt mirrors) is unchanged in content and routing.
- AE4. **Covers R12, R14.** Given the bridge is running but QEMU is not, when Claude calls any tool, the bridge returns a structured transport error within a bounded timeout. Claude's call does not hang indefinitely.
- AE5. **Covers R6.** Given a UI test scenario, when Claude calls `send_input` with a sequence of keystrokes, the kernel observes those keystrokes through the same input path that a real keyboard would feed, and any tool reading downstream input state sees them as ordinary input.
- AE6. **Covers R7.** Given a frame is on screen, when Claude calls `screenshot`, the bridge returns a PNG whose dimensions and content match the current framebuffer. The kernel did not perform PNG encoding to produce the result.

---

## Success Criteria

- A typical debug session for a visual or shell-driven bug uses zero `dbg!()` insertions and zero manual screenshots. Claude operates the running OS through MCP tool calls.
- Adding a new kernel tool (e.g., a sixth introspection target) requires changing only the kernel-side tool implementation and registration — no host-side bridge changes, no protocol revision.
- A future PR that swaps the transport from a second serial chardev to virtio-serial touches the transport layer only — `Tool` implementations, the registry, and tool consumers are unchanged.
- Downstream planning (`ce-plan`) does not need to re-litigate which tools v1 exposes, where the registry lives conceptually, whether the kernel does PNG encoding, or whether to gate the feature behind `--features test`.

---

## Scope Boundaries

- A virtio-serial driver. v1 uses a second `-serial` chardev; virtio-serial is the planned next transport but is not built here.
- Exposing the registry to in-OS apps. The architecture must keep the door open (R16), but the second consumer is not built in v1.
- A networking stack, virtio-net, or any TCP-based transport.
- Read-write vvfat or any filesystem-as-IPC scheme.
- Streaming, server-push, or long-lived MCP sessions. v1 is request/response only (R9).
- AuthN/authZ, rate limiting, schema versioning. Single user, single machine, host-loopback only.
- Replacing the existing `-serial stdio` log channel. The new RPC chardev is additive (R8, AE3).
- Persistent recordings of sessions, replay, or scripted scenario runners. Out of scope; the dev loop drives this manually through Claude.
- Test-mode-only behavior. The feature is unconditional (R17).

---

## Key Decisions

- **Kernel hosts the registry; host bridges to MCP.** Rationale: only the kernel can answer kernel-state questions truthfully, and the registry is the future canonical surface for in-OS agentic APIs. The bridge handles MCP framing and host-side concerns (PNG encoding, MCP advertisement) so the kernel stays small.
- **Approach A (line/JSON-Lines over a second serial chardev) for v1, designed so transport can swap to virtio-serial later.** Rationale: ships in days, no new driver work, infrastructure already exists in `build.sh`. Registry shape (R1, R15) prevents tool implementations from depending on the transport.
- **`kernel_state` is one tool with a discriminator, not three.** Rationale: keeps the surface tight and makes adding a new introspection target additive (R5).
- **Kernel does not encode PNG.** Rationale: image encoding belongs on the host where memory and crates are abundant; the kernel returns raw framebuffer bytes plus metadata (R7).
- **Bridge advertises tools dynamically via the registry.** Rationale: avoids a second source of truth for tool definitions (R13).
- **No `--features test` gating.** Rationale: the user's pain is the everyday dev loop, not test-only scenarios. Gating it would require a parallel un-gated path or duplicate work (R17).

---

## Dependencies / Assumptions

- QEMU's `-serial` chardev mechanism supports adding a second serial in addition to the existing `stdio`. Standard QEMU functionality; assumed available.
- The existing shell command dispatcher in `src/commands/` can be invoked programmatically and have its stdout captured. To verify during planning — `shell_run` (R4) depends on this.
- The existing input pipeline (`src/input/`) can accept synthesized events alongside hardware events. To verify during planning — `send_input` (R6) depends on this. If not currently possible, planning will need to address that.
- A no_std-compatible JSON layer (`serde_json` with `alloc`) is acceptable to add as a kernel dependency, OR the wire format will be simple enough not to need one. Choice deferred to planning.
- The host bridge can be written in any language with a usable MCP SDK (Node, TypeScript, Python, Rust). Choice deferred to planning.

---

## Outstanding Questions

### Resolve Before Planning

- (none — all scope-shaping questions are resolved.)

### Deferred to Planning

- [Affects R8, R9][Technical] Exact wire format: bare line-delimited JSON, length-prefixed JSON, or a tighter binary frame? Decision shaped by binary-payload concerns in R11.
- [Affects R7, R11][Technical] How are large binary payloads (full-screen framebuffers can be multiple MB) chunked or framed over the chardev? Affects timeout tuning in R14.
- [Affects R4][Needs investigation] Can the existing shell dispatcher's stdout be captured cleanly without forking a new I/O surface? Verify against `src/commands/` and `src/process/`.
- [Affects R6][Needs investigation] Does the input pipeline already accept programmatic injection, or does this require a small refactor? Verify against `src/input/`.
- [Affects R12][Technical] Bridge implementation language and process-management story (how is it launched alongside QEMU; how does it find the chardev socket).
- [Affects R3][Technical] Concrete schemas for each v1 tool's arguments and result types.
- [Affects R5][Technical] What heap-allocator stats are practical to expose for `kernel_state("heap")` given the current allocator implementation in `src/mm/`?
