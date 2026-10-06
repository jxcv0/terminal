use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
};

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
            unsafe { libc::execv(shell_path, argv.as_ptr()) };
            // unsafe { libc::_exit(0) };
        }
        child_pid => {
            println!("Child created with pid {child_pid}");
            // close pts fd
            std::mem::drop(pts);
            
            std::thread::sleep(std::time::Duration::new(3, 0));

            // start up the emulator
        }
    }
}
