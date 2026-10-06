use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const PTMX_PATH: &str = "/dev/ptmx";

fn open_ptm() -> Result<File, std::io::Error> {
    let ptm = OpenOptions::new().read(true).write(true).open(PTMX_PATH)?;

    if -1 == unsafe { libc::grantpt(ptm.as_raw_fd()) } {
        return Err(std::io::Error::last_os_error());
    }

    if -1 == unsafe { libc::unlockpt(ptm.as_raw_fd()) } {
        return Err(std::io::Error::last_os_error());
    }

    Ok(ptm)
}

fn open_pts(ptm: &File) -> Result<File, std::io::Error> {
    let pts_path = unsafe {
        let n = libc::ptsname(ptm.as_raw_fd());
        if n.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        CStr::from_ptr(n).to_str().unwrap().to_owned()
    };
    Ok(OpenOptions::new().read(true).write(true).open(pts_path)?)
}

fn main() {
    let ptm = open_ptm().unwrap();
    let pts = open_pts(&ptm).unwrap();
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let shell = CString::new(shell).unwrap();
    let shell_path = shell.as_ptr();
    let argv = [shell_path, std::ptr::null()];

    match unsafe { libc::fork() } {
        0 => {
            // set up PTY and exec
            let pid = unsafe { libc::setsid() };
            if -1 == pid {
                panic!("{}", std::io::Error::last_os_error());
            }
            let _ = unsafe { libc::ioctl(pts.as_raw_fd(), libc::TIOCSCTTY) };
            std::mem::drop(ptm);
            for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                if -1 == unsafe { libc::dup2(pts.as_raw_fd(), fd) } {
                    unsafe { libc::_exit(127) };
                }
            }
            std::mem::drop(pts);
            unsafe { libc::execv(shell_path, argv.as_ptr()) };
        }
        -1 => unsafe { libc::_exit(127) },
        _child_pid => {
            // close pts fd
            std::mem::drop(pts);

            run_emulator();
        }
    }
}

struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.window = Some(
            event_loop
                .create_window(Window::default_attributes())
                .unwrap(),
        );
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.window.as_ref().unwrap().request_redraw();
            }
            _ => (),
        }
    }
}

fn run_emulator() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { window: None };
    event_loop.run_app(&mut app).unwrap();
}
