# ScrollMux Agent Notes

ScrollMux is a terminal-native horizontal workspace for PTY sessions. It is inspired by niri/PaperWM-style spatial workflows, but it is not a window manager and not a full tmux/Zellij replacement.

Core idea:

```text
[ editor ][ agent ][ tests ][ logs ][ server ][ scratch ]
          └──────── current terminal viewport ────────┘
```

Fixed-width terminal panes live on a horizontal strip. The user's terminal window is only a viewport into that strip. Panes are not squeezed when new panes are added.

## Product Principles

1. Do one thing first: fixed-width PTY columns plus horizontal viewport scrolling.
2. Preserve pane width: adding panes, moving focus, and scrolling the viewport must not resize existing panes.
3. Keep focus and viewport separate: focused pane receives input; `viewport_x` controls what is visible.
4. Stay terminal-native: run inside existing terminal emulators and over SSH/devboxes.
5. Avoid tmux/Zellij scope creep for v0.1.

Out of scope for v0.1:

- daemon/client attach-detach architecture
- nested split trees, tabs, multi-workspace
- plugin system or scripting API
- complex copy mode, full mouse support, session sharing
- GUI sidebars, agent dashboards, desktop notifications
- terminal image protocols and full theming system

## Current Code Map

- `src/main.rs`: process entry, terminal size, raw mode, alternate screen, panic cleanup.
- `src/app.rs`: event loop, workspace ownership, action routing, render trigger, pane cleanup.
- `src/workspace.rs`: state and layout math: panes, focus, viewport, visible clipping.
- `src/pane.rs`: PTY lifecycle, `vt100::Parser`, reader thread, input writer.
- `src/render.rs`: host terminal painter from pane cell models.
- `src/input.rs`: raw stdin input parser, mux shortcut classifier, and PTY pass-through dispatch.
- `perf/`: terminal-output performance scenarios.
- `docs/current-design.zh.md`: concise Chinese design walkthrough.
- `docs/invarints.md`: rendering/input invariants and test targets.

## Core Invariants

The pane width invariant is the soul of the project:

```text
pane.width is independent from viewport width
```

Required behavior:

- New panes keep existing pane widths unchanged.
- Viewport movement does not resize PTYs.
- Host terminal resize may change pane height, but not pane width.
- Invisible panes continue running and parsing output.
- Rendering clips to the viewport; it must not render invisible pane regions.

The model should remain close to:

```rust
struct Workspace {
    panes: Vec<Pane>,
    focused: usize,
    viewport_x: u32,
    screen_width: u16,
    screen_height: u16,
    default_width: u16,
}
```

## Rendering Model

There are three coordinate spaces. Keep them explicit:

- virtual x: horizontal strip coordinate
- pane-local x: inside a pane
- host x: visible terminal screen coordinate

Use half-open intervals everywhere:

```text
pane range:     [pane_start, pane_end)
viewport range: [viewport_x, viewport_x + screen_width)
visible range:  intersection(pane range, viewport range)
```

PTY output bytes must not be replayed directly to the host terminal. They contain terminal commands in pane-local state. Correct path:

```text
PTY output bytes -> vt100::Parser -> pane-local cell grid -> viewport clipping -> host terminal drawing
```

Current renderer is intentionally simple but expensive: it clears and redraws the visible workspace. The next robustness/perf step is host-side backbuffer/damage tracking, likely line/chunk based, inspired by Zellij's output buffer approach.

## Input Model

Host stdin bytes are parsed only to decide whether ScrollMux should consume a
single mux shortcut. Raw bytes remain the source of truth for PTY input:

- ScrollMux action, consumed by the mux.
- PTY input bytes, forwarded verbatim only to the focused pane.

Mux shortcuts must not leak into PTYs. Every other byte sequence should behave
like a normal terminal because it is passed through without re-encoding. This
includes paste markers, mouse reports, IME/UTF-8 input, kitty keyboard protocol
sequences, and OSC52.

## Suggested Keybindings

```text
Alt-h       focus previous pane
Alt-l       focus next pane
Alt-H       move focused pane left
Alt-L       move focused pane right
Alt-n       new pane to the right
Alt-w       close focused pane
Alt-f       center focused pane
Alt-[       scroll viewport left
Alt-]       scroll viewport right
Alt-=       grow focused pane width
Alt--       shrink focused pane width
Alt-q       quit
```

Keep keybinding code simple and configurable later.

Alt shortcuts require the host terminal to send ESC-prefixed Meta input. On
Terminal.app, enable "Use Option as Meta key"; iTerm/Ghostty/WezTerm commonly
send Meta-style input by default or expose an equivalent setting.

## MVP Scope

Required:

- single workspace
- horizontal strip of fixed-width PTY panes
- one PTY per pane
- focused pane
- horizontal viewport offset
- render only visible pane portions
- spawn, close, move, focus panes
- center focused pane
- scroll viewport without changing focus
- simple status bar

Nice to have later:

- config file and session restore
- rename pane
- command palette
- basic pane scrollback
- copy support through terminal selection or a focused copy mode

## Dependencies

Use established libraries where possible:

- `portable-pty` for PTY management
- `crossterm` for raw mode, alternate screen, host drawing
- `termwiz` for parsing raw host stdin bytes into semantic input events
- `vt100` for terminal parsing/emulation
- consider `ratatui` for host-side buffer/diff rendering if it preserves ScrollMux's viewport model

Do not implement a terminal emulator from scratch.

## Testing And Perf

Layout math should be covered by unit/property tests around `visible_panes()`:

- source ranges stay within pane width
- destination ranges stay within screen width
- visible destination ranges are ordered and non-overlapping
- exact pane/viewport boundary cases are stable

Perf suite:

```bash
nix develop
perf/run.sh
perf/run.sh --scenario nvim-scroll
perf/run.sh --strace
```

Key metric: output amplification from direct program output to ScrollMux-rendered output, especially for full-screen TUIs such as nvim.

## Development Guidance

- Prefer existing local patterns over new abstractions.
- Keep changes tightly scoped to the project invariant being improved.
- Use structured geometry and tests instead of ad hoc clipping math.
- Do not add multiplexer features that dilute the fixed-width horizontal workspace goal.
- When changing rendering, preserve semantics before optimizing output volume.
