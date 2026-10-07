use crate::{
    config::Config,
    graphics::{Frame, Graphics},
    input,
    model::{INITIAL_COLS, INITIAL_ROWS, Terminal},
    pty::{Output, Pty},
    view::TerminalView,
};
use std::{
    io,
    process::ExitStatus,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, Ime, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

pub enum UserEvent {
    PtyReady,
    ChildExit(io::Result<ExitStatus>),
    Repaint(Instant),
    WriteError(io::Error),
    GpuError(String),
}

pub struct App {
    config: Config,
    pty: Option<Pty>,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    terminal: Terminal,
    view: TerminalView,
    modifiers: ModifiersState,
    preedit: String,
    focused: bool,
    occluded: bool,
    cursor_on: bool,
    next_blink: Instant,
    repaint: Option<Instant>,
    input_started: Option<Instant>,
    child_exited: bool,
    eof: bool,
    demo: bool,
    profile: bool,
}

impl App {
    pub fn new(
        pty: Option<Pty>,
        proxy: EventLoopProxy<UserEvent>,
        profile: bool,
        config: Config,
    ) -> Self {
        let demo = pty.is_none();
        let mut terminal = Terminal::new(INITIAL_ROWS, INITIAL_COLS, config.scrollback_lines);
        if demo {
            terminal.demo();
        }
        Self {
            pty,
            proxy,
            window: None,
            graphics: None,
            terminal,
            view: TerminalView::new(&config),
            modifiers: ModifiersState::empty(),
            preedit: String::new(),
            focused: true,
            occluded: false,
            cursor_on: true,
            next_blink: Instant::now() + Duration::from_millis(config.cursor_blink_ms),
            repaint: None,
            input_started: None,
            child_exited: false,
            eof: false,
            demo,
            profile,
            config,
        }
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.terminal.scroll_to_bottom();
        self.terminal.selection = None;
        self.cursor_on = true;
        self.next_blink = Instant::now() + Duration::from_millis(self.config.cursor_blink_ms);
        self.input_started.get_or_insert_with(Instant::now);
        if let Some(pty) = &self.pty
            && let Err(error) = pty.write(bytes)
        {
            eprintln!("PTY input failed: {error}");
        }
        self.redraw();
    }

    fn key(&mut self, key: &Key, text: Option<&str>) {
        if !self.view.focused || !self.focused || !self.preedit.is_empty() {
            return;
        }
        let ctrl_shift = self.modifiers.control_key() && self.modifiers.shift_key();
        let character = match key {
            Key::Character(value) => value.to_lowercase(),
            _ => String::new(),
        };
        if (ctrl_shift && character == "c") || *key == Key::Named(NamedKey::Copy) {
            if let Some(graphics) = &mut self.graphics {
                graphics
                    .input
                    .set_clipboard_text(self.terminal.selected_text());
            }
            return;
        }
        if (ctrl_shift && character == "v")
            || (*key == Key::Named(NamedKey::Insert) && self.modifiers.shift_key())
            || *key == Key::Named(NamedKey::Paste)
        {
            if let Some(text) = self
                .graphics
                .as_mut()
                .and_then(|graphics| graphics.input.clipboard_text())
            {
                self.send(input::paste(
                    &text,
                    self.terminal.screen().bracketed_paste(),
                ));
            }
            return;
        }
        if self.modifiers.shift_key()
            && matches!(key, Key::Named(NamedKey::PageUp | NamedKey::PageDown))
        {
            let rows = isize::try_from(self.terminal.screen().size().0).unwrap();
            self.terminal
                .scroll(if *key == Key::Named(NamedKey::PageUp) {
                    rows
                } else {
                    -rows
                });
            self.redraw();
            return;
        }
        self.send(input::encode_key(
            key,
            text,
            self.modifiers,
            self.terminal.screen().application_cursor(),
        ));
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(graphics) = &mut self.graphics else {
            return;
        };
        match graphics.draw(
            &mut self.terminal,
            &mut self.view,
            &self.preedit,
            self.cursor_on && self.focused,
            self.demo,
            self.input_started,
        ) {
            Ok((frame, size)) => {
                if let (Some(pty), Some((rows, cols))) = (&self.pty, size)
                    && let Err(error) = pty.resize(rows, cols)
                {
                    eprintln!("PTY resize failed: {error}");
                }
                match frame {
                    Frame::Presented => {
                        self.input_started = None;
                        if self.child_exited && self.eof {
                            event_loop.exit();
                        }
                    }
                    Frame::Retry => self.repaint = Some(Instant::now() + Duration::from_millis(16)),
                    Frame::Hidden => {
                        if self.child_exited && self.eof {
                            event_loop.exit();
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("Graphics failed: {error}");
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let window = match &self.window {
                Some(window) => window.clone(),
                None => Arc::new(
                    event_loop.create_window(
                        Window::default_attributes()
                            .with_title(if self.demo {
                                "Terminal — demo"
                            } else {
                                "Terminal"
                            })
                            .with_inner_size(winit::dpi::LogicalSize::new(
                                self.config.window.width,
                                self.config.window.height,
                            )),
                    )?,
                ),
            };
            window.set_ime_purpose(winit::window::ImePurpose::Terminal);
            self.graphics = Some(futures_lite::future::block_on(Graphics::new(
                window.clone(),
                self.proxy.clone(),
                self.profile,
                &self.config.font,
            ))?);
            self.window = Some(window);
            self.view = TerminalView::new(&self.config);
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Graphics initialization failed: {error}");
            event_loop.exit();
        }
        self.redraw();
    }

    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.graphics = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        if let Some(graphics) = &mut self.graphics
            && graphics
                .input
                .on_window_event(&graphics.window, &event)
                .repaint
        {
            graphics.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.draw(event_loop),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = &mut self.graphics {
                    graphics.resize(size);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(graphics) = &mut self.graphics {
                    graphics.resize(graphics.window.inner_size());
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                self.key(&event.logical_key, event.text.as_deref())
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.focused && self.view.focused => {
                self.preedit.clear();
                self.send(text.into_bytes());
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) if self.focused && self.view.focused => {
                self.preedit = text;
                self.redraw();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.preedit.clear();
                self.redraw();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.preedit.clear();
                self.modifiers = ModifiersState::empty();
                self.cursor_on = true;
                self.next_blink =
                    Instant::now() + Duration::from_millis(self.config.cursor_blink_ms);
                self.redraw();
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    self.redraw();
                }
            }
            _ => (),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyReady => {
                if let Some(pty) = &self.pty {
                    for output in pty.drain() {
                        match output {
                            Output::Data(bytes) => self.terminal.process(&bytes),
                            Output::Eof(result) => {
                                self.eof = true;
                                if let Err(error) = result {
                                    eprintln!("PTY output failed: {error}");
                                }
                            }
                        }
                    }
                    if let Err(error) = pty.write(self.terminal.take_replies()) {
                        eprintln!("PTY response failed: {error}");
                    }
                }
                if let (Some(window), Some(title)) = (&self.window, self.terminal.take_title()) {
                    window.set_title(&title);
                }
                self.redraw();
            }
            UserEvent::ChildExit(status) => {
                self.child_exited = true;
                match status {
                    Ok(status) if status.success() => (),
                    Ok(status) => eprintln!("Shell exited with {status}"),
                    Err(error) => eprintln!("Failed to wait for shell: {error}"),
                }
                self.redraw();
            }
            UserEvent::Repaint(deadline) => {
                self.repaint = Some(
                    self.repaint
                        .map_or(deadline, |current| current.min(deadline)),
                )
            }
            UserEvent::WriteError(error) => eprintln!("PTY write failed: {error}"),
            UserEvent::GpuError(error) => {
                eprintln!("GPU failure: {error}");
                event_loop.exit();
            }
        }
        // An invisible window cannot present, but all queued output must still
        // reach the model before the child-exit notification may close it.
        if self.child_exited
            && self.eof
            && (self.occluded || self.graphics.as_ref().is_none_or(Graphics::is_zero_sized))
        {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let visible = !self.occluded
            && self
                .graphics
                .as_ref()
                .is_some_and(|graphics| !graphics.is_zero_sized());
        if !visible {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if self.repaint.is_some_and(|deadline| deadline <= now) {
            self.repaint = None;
            self.redraw();
        }
        let blinking = self.config.cursor_blink_ms != 0
            && self.focused
            && self.view.focused
            && !self.terminal.screen().hide_cursor()
            && self.terminal.screen().scrollback() == 0;
        if blinking && self.next_blink <= now {
            self.cursor_on = !self.cursor_on;
            self.next_blink = now + Duration::from_millis(self.config.cursor_blink_ms);
            self.redraw();
        }
        let deadline = if blinking {
            Some(
                self.repaint
                    .map_or(self.next_blink, |repaint| repaint.min(self.next_blink)),
            )
        } else {
            self.repaint
        };
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}
