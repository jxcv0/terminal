- **Review scope:** Current committed prototype at `e782f2c`, reviewed on
  7 October 2026. The working tree was clean, so this review covers the current
  implementation rather than an uncommitted diff. Findings are ordered by
  importance. No application code was changed.

- **Fix first: Keep the parent from acquiring the shell's controlling terminal.**
  Location: [src/main.rs:35](src/main.rs#L35), called before `fork()` at line 40.

  The parent opens the PTY slave without `O_NOCTTY`. If the emulator starts as a
  session leader without a controlling terminal, such as through `setsid`, Linux
  can assign that slave to the parent's session. The child then creates a
  different session, and its attempt to claim the same terminal fails with
  `EPERM`. The shell starts without the controlling terminal needed for normal
  job control.

  **Suggested correction:** Open the slave with `O_NOCTTY` and let the child
  explicitly acquire it after `setsid()`. This follows the documented behavior
  of [terminal opens](https://man7.org/linux/man-pages/man2/open.2.html) and
  [controlling-terminal assignment](https://man7.org/linux/man-pages/man2/tiocsctty.2const.html).

- **Fix first: Supply the required `TIOCSCTTY` argument and handle failure.**
  Location: [src/main.rs:56](src/main.rs#L56).

  This `ioctl` request requires a third integer argument, normally `0`. The
  current unsafe variadic call supplies only two arguments, leaving the callee
  to read an argument that was never provided. The code also discards the return
  value, so a failed controlling-terminal setup still proceeds to launch the
  shell. This also hides the failure described above.

  **Suggested correction:** Pass an explicit `0` and stop the child with a
  nonzero exit status if the call returns `-1`. Preserve enough error information
  to diagnose the failure. See the
  [required signature and error behavior](https://man7.org/linux/man-pages/man2/tiocsctty.2const.html).

- **Medium priority: Stop scheduling another redraw from every redraw event.**
  Location: [src/main.rs:102](src/main.rs#L102).

  Each `RedrawRequested` event immediately requests another redraw, even though
  nothing is drawn or changed. Once a redraw arrives, this creates a continuous
  stream of work. In the installed winit 0.30.13 X11 implementation, a pending
  redraw makes the event loop poll with a zero timeout, so `ControlFlow::Wait`
  does not make this loop idle. The result is avoidable CPU use while the window
  has nothing to update.

  **Suggested correction:** Finish handling the redraw without requesting
  another one. Request future redraws when content changes, the window needs
  repainting, or a deliberate animation timer fires. The
  [winit redraw documentation](https://docs.rs/winit/0.30.13/winit/window/struct.Window.html#method.request_redraw)
  describes how this call queues another event. The X11 polling behavior was
  checked in the locally installed dependency source; CPU use was not measured.

- **Medium priority: Retain the child PID and collect its exit status.**
  Location: [src/main.rs:67](src/main.rs#L67).

  The parent discards the shell's PID and never calls a wait function. When the
  shell exits, the parent keeps running its window loop and, with the normal
  `SIGCHLD` disposition, the child remains a zombie until the parent exits.
  The application also has no way to react to shell termination or distinguish
  a successful exit from a failure. An immediately exiting executable, such as
  `SHELL=/bin/true`, is a simple case to check once GUI execution is available.

  **Suggested correction:** Keep the PID, collect its status without blocking
  the UI, and notify the event loop when the child exits so the application can
  close the window or show an ended-session state. See
  [child-process waiting and zombie behavior](https://man7.org/linux/man-pages/man2/waitpid.2.html).

- **Medium priority: Report shell execution failure as an error.**
  Location: [src/main.rs:64](src/main.rs#L64).

  If `SHELL` names a missing or non-executable file, `execv()` returns an error.
  The child ignores that result and falls out of `main()`, producing a successful
  exit status despite never starting a shell. No diagnostic explains the failure.
  This remains incorrect even after the parent starts collecting child statuses.

  **Suggested correction:** Treat any return from `execv()` as failure, report
  the cause, and terminate with `libc::_exit(127)` rather than returning normally.
  The underlying [exec system call](https://man7.org/linux/man-pages/man2/execve.2.html)
  returns only on failure.

- **Verification:** `cargo check --locked --offline`,
  `cargo build --locked --offline`, and `cargo test --locked --offline` passed.
  The test command ran **zero tests**. The compiler reported one warning:
  `App::ptm` is never read.

- **Runtime limits:** Attempts to run under Xvfb failed because the sandbox
  prevented the display server from creating its listening sockets; the
  application then failed to open the display. `strace` was also blocked by
  `ptrace` restrictions. The findings above are based on application source,
  installed winit source, and API documentation, not successful end-to-end
  reproductions.

- **Unfinished work, separate from the findings:** PTY reads and writes,
  keyboard forwarding, terminal rendering, and terminal-size updates are not
  implemented. These are expected development gaps in this prototype; they are
  not counted as additional defects in this review.
