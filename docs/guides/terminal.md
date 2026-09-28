# Terminal (native PTY over WebSocket)

**Purpose:** In-browser shell via a native in-process PTY (replaces the old
ttyd-based implementation).

## Backend

Router built by `src/handlers/terminal_pty.rs::create_terminal_router`, nested at
`/api/terminal` (`src/web/mod.rs`). Key types: `PtyManager`, `PtySession`,
`spawn_session`, `handle_socket`, `terminal_ws_handler`, `handle_terminal_ticket`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /terminal/ticket` | admin | single-use ticket (exchanged for a WebSocket) |
| `POST /terminal/close?tab=` | admin | tear down one shell tab |
| `GET /terminal/cwd?tab=` | admin | real shell cwd, resolved from `/proc/<pid>/cwd` |
| `GET /terminal/ls?tab=&path=` | admin | directory listing for the completion popup |
| `WS /terminal/ws?ticket=...` | admin | PTY socket (ticket redeemed once) |

`/cwd` and `/ls` exist only to power the client-side completion popup. They
reuse the terminal's admin gate (401 no session / 403 non-admin, matching the
ticket endpoint) and require a **live session for the given `tab`** before
touching the filesystem (`/ls` answers 404 otherwise). `get_session_cwd`
resolves the real working directory of the shell behind a tab by reading
`/proc/<pid>/cwd`, walking the `sudo -u <user> -i` process tree a few levels
down (`descendant_pids`) and keeping the deepest readable cwd.

## Frontend

- `frontend/src/routes/Terminal.svelte` (Svelte 5 runes).
- `frontend/src/lib/services/api.ts`: `getTerminalTicket()`, `closeTerminalSession()`,
  `getTerminalCwd()`, `listTerminalDir()`.
- WebSocket opened from the ticket URL (not the long-lived session token in the
  query string).

### Window anatomy

The window is a non-scrolling flex column; each bar is `flex-none` and only the
xterm body scrolls (`data-testid="terminal-body"`, `flex:1; min-height:0`).
Top to bottom:

1. `WindowHeader` (`desktop/WindowHeader.svelte`) — icon tile + the app title.
   It is the window's `[data-hv-drag]` region, and its right edge belongs to the
   min / max / close trio.
2. `data-testid="terminal-tabs"` — the tab strip (one chip per `Tab`, plus the
   add button). **Deliberately in the window content, never inside
   `WindowHeader`**: a strip there competes with the window controls pinned to
   the header's right edge.
3. `data-testid="terminal-infobar"` — `user@host`, the live cwd, the connection
   badge and the shell version.
4. `data-testid="terminal-body"` — one `.hv-terminal-content` per tab, all
   stacked; only the active one is unhidden, so switching tabs never re-parents
   an xterm instance.

E2E `2.8` guards this ordering (the strip must not be a `.hv-winhdr` descendant
and must sit between the header and the info bar). `2.9` guards that the strip
scrolls **horizontally only**. Before the Tailwind-on-tokens migration the strip
carried `scrollbar-width: none`, which hid both bars; that declaration has no
Tailwind utility and was dropped, so the engine default came back. Because a
lone `overflow-x: auto` computes `overflow-y` to `auto` as well, and the 1px a
`min-h-11` border-box leaves over its content box (44px minus the bottom border)
is all a scrollbar needs, a **vertical** bar appeared on a strip that scrolls
horizontally. `overflow-y-hidden` removes that axis at the source rather than
hiding scrollbars, so the horizontal bar still appears when the tabs genuinely
overflow (a per-user cap of `MAX_TABS_PER_USER` tabs can overflow) — restore
`scrollbar-width: none` only if hiding that too is the intent.

`terminal-body` paints the **same** background as the xterm theme above it
(`theme.background` in `initTabTerminal`, i.e. `--hv-shell-glass`). `FitAddon`
sizes the terminal to `floor(height / cellHeight)` rows, so a sub-row remainder
is always left at the bottom of the body, and whatever the body paints there is
what shows. The two had drifted apart: the Tailwind-on-tokens migration carried
the body over as a hardcoded dark `bg-[#1e1e1e]` — the pre-migration scoped
`.hv-terminal-content` was light (`rgba(255,255,255,0.55)`, matching its theme
of the day) — while the theme itself became opaque `#FFFFFF`, so the remainder
painted as a black band under the last line of output. E2E `2.10` compares the
two computed colours live, so changing the theme drags the container along.

### Completion (ghost text + Tab candidate popup)

`Terminal.svelte` tracks the user's current input line locally (from `term.onData`)
and offers client-side completion on top of the real shell:

- **Inline ghost** — when exactly one candidate exists, the suggestion suffix is
  rendered as a dim italic suffix right after the cursor and accepted with Tab.
- **Candidate popup** — several candidates → a small `data-testid="terminal-completion-popup"`
  list appears on Tab; clicking an item completes it, Escape / typing hides it.
- **Fall-through** — with no client-side candidate, Tab is passed through to the
  shell unchanged, so bash's own native completion keeps working.
- First token = command (static `COMMANDS` list); later tokens = path completion
  against the shell's real cwd via `GET /terminal/ls`.

Correctness rules that make the ghost safe on a real echo PTY (treat as easy
to break):

- The ghost is cleared **before** any cursor-moving event: the next keystroke,
  incoming server output (`ws.onmessage` clears first), tab switch, or resize.
  Because the cursor cannot move between ghost render and the next event, the
  relative `\x1b[nD` + spaces + `\x1b[nD` erase always lands exactly on the ghost.
- `computeCompletions` is debounced (180 ms) and re-validates that the tracked
  line didn't change while it computed; `handleTab` resolves a pending debounce
  synchronously so a Tab right after typing acts on fresh candidates.
- Tracked line resets on escape sequences (arrows/home/end) and other control
  keys, so a cursor-moved input never yields a wrong suggestion.

## Lifecycle / invariants

- A single-use ticket is minted by `getTerminalTicket` and redeemed by the
  WebSocket upgrade; it cannot be reused.
- When the shell exits (e.g. user types `exit`), the server sends a clean WebSocket
  close frame so the UI stops showing "Connected".
- Sessions are tracked in `PtyManager`; on user deletion
  (`handle_user_delete`) the session is killed, and on graceful shutdown
  `shutdown_signal` calls `pty_manager.cleanup_all()` so shells never outlive the
  server.
- Output is replayed after a reconnect (shared attach).
- The WS closing must **not** kill the PTY — only `close_tab`, `reap_stale`,
  `kill_session`, `cleanup_all` kill (reconnect support).

## Related

- [Users](./users.md), [API Contract](../API.md)