# Terminal

A Linux terminal emulator using winit, egui and wgpu's Vulkan backend. The shell
is forked before window/graphics initialization. `vt100` owns terminal parsing,
the grid, cursor, modes and a 10,000-line scrollback buffer independently of egui.

## Run

Requires Rust 1.88 or newer, an X11 or Wayland session, and a Vulkan driver.
The dependency set is locked in `Cargo.lock`: egui/egui-winit/egui-wgpu 0.33.3,
wgpu 27.0.1 and the existing winit 0.30.13. These matching egui releases were
available in the implementation environment's offline cache.

```sh
cargo run --locked --release
cargo run --locked --release -- --demo
cargo run --locked --release -- --profile
```

The default command starts `$SHELL`, falling back to `/bin/sh`, with
`TERM=xterm-256color` and `COLORTERM=truecolor`. The PTY starts at 80×24 and
receives the actual grid size after the first frame and subsequent resizes.
`--demo` paints a deterministic grid without starting a shell. `--profile` can
be combined with either mode and logs frame preparation, optional Vulkan
timestamps and input-to-present timing to stderr.

| Interaction | Action |
| --- | --- |
| Type, arrows, Home/End, Delete, function keys | Send terminal input |
| Ctrl+C / Ctrl+D / Ctrl+Z | Normal shell control characters |
| Alt+key | Escape-prefixed terminal input |
| Drag with the left mouse button | Select visible text |
| Ctrl+Shift+C | Copy selection |
| Ctrl+Shift+V or Shift+Insert | Paste, respecting bracketed-paste mode |
| Mouse wheel or Shift+PageUp/PageDown | Scroll history |
| Type after scrolling | Return to the live screen |

IME preedit is an overlay; committed text is sent once. Raw winit keyboard
events have one terminal focus owner; egui's translated text/clipboard events
are not sent a second time. Tab stays in the terminal. Selection is cleared by
output, scrolling or resizing because its coordinates refer to visible cells.

## Implementation

- `src/model.rs`: parser, modes, selection/copy, scrollback, terminal replies
  and the demo fixture; no egui dependency.
- `src/view.rs`: one clipped terminal widget, explicit pixel-aligned cell
  metrics, merged background runs, text attributes, selection and a legible
  block cursor. Embedded DejaVu Sans Mono and Droid Sans Fallback cover the
  sample's combining and CJK glyphs; licenses are in `assets/fonts/`.
- `src/pty.rs`: worker threads for reads and partial writes. Output has a
  64 × 4 KiB bounded queue, coalesced wakeups and a maximum 64 KiB consumed per
  UI turn. EOF follows queued bytes; child exit alone does not close the app.
- `src/graphics.rs`: surface/device lifecycle, egui texture/buffer uploads,
  submission/presentation, deferred texture frees and optional GPU timing.
- `src/app.rs` and `src/input.rs`: input ownership/encoding, PTY resize,
  redraw scheduling and child-exit coordination. Idle operation uses `Wait`
  or `WaitUntil`; a focused visible cursor blinks every 500 ms.

Zero-sized surfaces are skipped, lost/outdated surfaces are reconfigured and
transient acquisition timeouts are retried later. Small viewports clip a
minimum 2×2 grid because the parser requires it for wide glyphs and autowrap;
dimensions are capped at 1,000 cells per axis. Fallback font advances do not
determine subsequent columns.

This is an initial terminal implementation, not a full xterm compatibility
claim. Advanced shaping, color emoji, application mouse reporting, custom
keypad modes, accessibility, tabs and settings UI remain outside this change.
Font coverage is finite; unsupported glyphs still use the font's replacement
character. Bold is synthesized and italic uses egui's text formatting.

## Verify

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --release offscreen_renderer -- --ignored --nocapture
```

The last command requires a Vulkan adapter, including software Vulkan, and
writes PPM captures to `target/terminal-validation/`. It reports a bounded
80×24/240×80 timing study. See [the validation record](EGUI_WGPU_VALIDATION.md)
for measured results, their limits, and the remaining real-display checks.
