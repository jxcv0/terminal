use std::{
    ffi::CStr,
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
    // open the PTM
    let ptm = open_ptm().unwrap();
    let pts = open_pts(&ptm).unwrap();

    // get the fd of the PTS
    dbg!(&ptm, &pts);
}
