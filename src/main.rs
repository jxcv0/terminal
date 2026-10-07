use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    io::Write,
    os::{
        fd::{AsFd, AsRawFd},
        unix::{fs::OpenOptionsExt, process::ExitStatusExt},
    },
    process::ExitStatus,
};
use winit::event_loop::{ControlFlow, EventLoop};

mod app;
mod graphics;
mod input;
mod model;
mod pty;
mod view;

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
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(pts_path)
}

fn child_error(mut stderr: &File, operation: &str) -> ! {
    let error = std::io::Error::last_os_error();
    let _ = writeln!(stderr, "{operation}: {error}");
    unsafe { libc::_exit(127) }
}

fn main() {
    let mut demo = false;
    let mut profile = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--demo" => demo = true,
            "--profile" => profile = true,
            "--help" | "-h" => {
                println!(
                    "Usage: terminal [--demo] [--profile]\n\n--demo     Render a fixed sample without starting a shell\n--profile  Log CPU frame preparation, GPU timestamps (if supported), and input-to-present time"
                );
                return;
            }
            _ => {
                eprintln!("Unknown argument: {argument}");
                std::process::exit(2);
            }
        }
    }
    if demo {
        run_emulator(None, None, profile);
        return;
    }
    let ptm = open_ptm().unwrap();
    let pts = open_pts(&ptm).unwrap();
    pty::set_size(&ptm, model::INITIAL_ROWS, model::INITIAL_COLS).unwrap();
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let shell = CString::new(shell).unwrap();
    let shell_path = shell.as_ptr();
    let argv = [shell_path, std::ptr::null()];
    // Prepare the child environment before fork; graphics and worker threads
    // are only initialized in the parent after the shell has been forked.
    use std::os::unix::ffi::OsStringExt;
    let mut environment: Vec<CString> = std::env::vars_os()
        .filter(|(key, _)| key != "TERM" && key != "COLORTERM")
        .map(|(key, value)| {
            let mut bytes = key.into_vec();
            bytes.push(b'=');
            bytes.extend(value.into_vec());
            CString::new(bytes).unwrap()
        })
        .collect();
    environment.push(CString::new("TERM=xterm-256color").unwrap());
    environment.push(CString::new("COLORTERM=truecolor").unwrap());
    let mut envp: Vec<_> = environment.iter().map(|entry| entry.as_ptr()).collect();
    envp.push(std::ptr::null());
    // Keep startup errors visible after stderr is redirected to the PTY.
    // The duplicated descriptor is closed automatically on a successful exec.
    let stderr = File::from(std::io::stderr().as_fd().try_clone_to_owned().unwrap());

    match unsafe { libc::fork() } {
        0 => {
            // set up PTY and exec
            let pid = unsafe { libc::setsid() };
            if -1 == pid {
                child_error(&stderr, "setsid");
            }
            if -1 == unsafe { libc::ioctl(pts.as_raw_fd(), libc::TIOCSCTTY, 0) } {
                child_error(&stderr, "TIOCSCTTY");
            }
            std::mem::drop(ptm);
            for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                if -1 == unsafe { libc::dup2(pts.as_raw_fd(), fd) } {
                    child_error(&stderr, "dup2");
                }
            }
            std::mem::drop(pts);
            unsafe { libc::execve(shell_path, argv.as_ptr(), envp.as_ptr()) };
            child_error(&stderr, "execve");
        }
        -1 => child_error(&stderr, "fork"),
        child_pid => {
            // close pts fd
            std::mem::drop(pts);
            std::mem::drop(stderr);

            run_emulator(Some(ptm), Some(child_pid), profile);
        }
    }
}

fn wait_for_child(child_pid: libc::pid_t) -> std::io::Result<ExitStatus> {
    let mut status = 0;
    loop {
        if -1 != unsafe { libc::waitpid(child_pid, &mut status, 0) } {
            return Ok(ExitStatus::from_raw(status));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn run_emulator(ptm: Option<File>, child_pid: Option<libc::pid_t>, profile: bool) {
    let event_loop = EventLoop::with_user_event().build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    if let Some(child_pid) = child_pid {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let _ = proxy.send_event(app::UserEvent::ChildExit(wait_for_child(child_pid)));
        });
    }
    let pty = ptm.map(|ptm| pty::Pty::new(ptm, proxy.clone()).unwrap());
    let mut app = app::App::new(pty, proxy, profile);
    event_loop.run_app(&mut app).unwrap();
}

#[cfg(test)]
#[allow(
    clippy::zombie_processes,
    reason = "wait_for_child reaps these children with libc::waitpid"
)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn wait_collects_exit_status_and_reaps_child() {
        for (program, expected) in [("/bin/true", 0), ("/bin/false", 1)] {
            let child = Command::new(program).spawn().unwrap();
            let child_pid = child.id() as libc::pid_t;
            let status = wait_for_child(child_pid).unwrap();
            assert_eq!(status.code(), Some(expected));
            assert_eq!(
                wait_for_child(child_pid).unwrap_err().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
    }

    #[test]
    fn wait_reports_signal_termination() {
        let child = Command::new("/bin/sh")
            .args(["-c", "kill -TERM $$"])
            .spawn()
            .unwrap();
        let status = wait_for_child(child.id() as libc::pid_t).unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        assert!(!status.success());
    }
}
