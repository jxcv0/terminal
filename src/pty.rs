use crate::app::UserEvent;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use winit::event_loop::EventLoopProxy;

const CHUNK_SIZE: usize = 4096;
const QUEUE_CHUNKS: usize = 64;
pub const CHUNKS_PER_TURN: usize = 16;

pub enum Output {
    Data(Vec<u8>),
    Eof(io::Result<()>),
}

pub fn set_size(file: &File, rows: u16, cols: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &size) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ready(file: &File, events: libc::c_short) -> io::Result<bool> {
    let mut fd = libc::pollfd {
        fd: file.as_raw_fd(),
        events,
        revents: 0,
    };
    match unsafe { libc::poll(&mut fd, 1, 100) } {
        -1 => {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                Ok(false)
            } else {
                Err(error)
            }
        }
        0 => Ok(false),
        _ => Ok(true), // Also attempt the I/O on HUP/ERR to collect EOF/errors.
    }
}

fn wake(proxy: &EventLoopProxy<UserEvent>, pending: &AtomicBool) {
    if !pending.swap(true, Ordering::AcqRel) {
        let _ = proxy.send_event(UserEvent::PtyReady);
    }
}

pub struct Pty {
    master: File,
    output: Option<Receiver<Output>>,
    input: Sender<Vec<u8>>,
    stopped: Arc<AtomicBool>,
    wake_pending: Arc<AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
    workers: Vec<JoinHandle<()>>,
}

impl Pty {
    pub fn new(master: File, proxy: EventLoopProxy<UserEvent>) -> io::Result<Self> {
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if flags == -1
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                == -1
        {
            return Err(io::Error::last_os_error());
        }
        let reader = master.try_clone()?;
        let mut writer = master.try_clone()?;
        let (output_tx, output) = mpsc::sync_channel(QUEUE_CHUNKS);
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        let stopped = Arc::new(AtomicBool::new(false));
        let wake_pending = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let pending = wake_pending.clone();
        let notify = proxy.clone();
        let read_thread = thread::spawn(move || {
            read_output(reader, &stop, output_tx, || wake(&notify, &pending));
        });
        let stop = stopped.clone();
        let notify = proxy.clone();
        let write_thread = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let bytes = match input_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(bytes) => bytes,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if let Err(error) = write_pending(&mut writer, &bytes, &stop) {
                    let _ = notify.send_event(UserEvent::WriteError(error));
                    break;
                }
            }
        });
        Ok(Self {
            master,
            output: Some(output),
            input,
            stopped,
            wake_pending,
            proxy,
            workers: vec![read_thread, write_thread],
        })
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        set_size(&self.master, rows, cols)
    }

    pub fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.input
            .send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))
    }

    pub fn drain(&self) -> Vec<Output> {
        // Clear before receiving: a racing producer will either wake us or its
        // bytes will be included in this batch. A full batch schedules another turn.
        self.wake_pending.store(false, Ordering::Release);
        let batch: Vec<_> = self
            .output
            .as_ref()
            .unwrap()
            .try_iter()
            .take(CHUNKS_PER_TURN)
            .collect();
        if batch.len() == CHUNKS_PER_TURN {
            wake(&self.proxy, &self.wake_pending);
        }
        batch
    }
}

fn read_output(
    mut reader: File,
    stopped: &AtomicBool,
    output: mpsc::SyncSender<Output>,
    notify: impl Fn(),
) {
    let result = (|| -> io::Result<()> {
        let mut buffer = [0; CHUNK_SIZE];
        while !stopped.load(Ordering::Acquire) {
            if !ready(&reader, libc::POLLIN)? {
                continue;
            }
            let count = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                // Linux PTY masters report EIO when the final slave closes.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            // Backpressure here bounds memory and never discards PTY bytes.
            if output.send(Output::Data(buffer[..count].to_vec())).is_err() {
                return Ok(());
            }
            notify();
        }
        Ok(())
    })();
    if output.send(Output::Eof(result)).is_ok() {
        notify();
    }
}

fn write_pending(writer: &mut File, mut bytes: &[u8], stopped: &AtomicBool) -> io::Result<()> {
    while !bytes.is_empty() && !stopped.load(Ordering::Acquire) {
        if !ready(writer, libc::POLLOUT)? {
            continue;
        }
        match writer.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Release a reader blocked by backpressure before joining it. The poll
        // timeout releases idle workers, then closing all masters hangs up the shell.
        self.output.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_and_updated_size_reach_slave() {
        let master = crate::open_ptm().unwrap();
        let slave = crate::open_pts(&master).unwrap();
        for (rows, cols) in [(24, 80), (80, 240)] {
            set_size(&master, rows, cols).unwrap();
            let mut size: libc::winsize = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCGWINSZ, &mut size) },
                0
            );
            assert_eq!((size.ws_row, size.ws_col), (rows, cols));
        }
    }

    #[test]
    fn nonblocking_partial_writes_preserve_all_bytes() {
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        let (a, mut b) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        let mut writer = File::from(OwnedFd::from(a));
        let data: Vec<_> = (0..1_000_000).map(|i| (i % 251) as u8).collect();
        let expected = data.clone();
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            b.read_to_end(&mut bytes).unwrap();
            bytes
        });
        write_pending(&mut writer, &data, &AtomicBool::new(false)).unwrap();
        drop(writer);
        assert_eq!(reader.join().unwrap(), expected);
    }

    #[test]
    fn output_backpressure_preserves_order_and_drains_before_eof() {
        let master = crate::open_ptm().unwrap();
        let mut slave = crate::open_pts(&master).unwrap();
        let data: Vec<_> = (0..CHUNK_SIZE * (QUEUE_CHUNKS + 3))
            .map(|i| b'a' + (i % 26) as u8)
            .collect();
        let expected = data.clone();
        let writer = thread::spawn(move || slave.write_all(&data).unwrap());
        let (send, receive) = mpsc::sync_channel(2);
        let reader =
            thread::spawn(move || read_output(master, &AtomicBool::new(false), send, || {}));
        let mut bytes = Vec::new();
        loop {
            match receive.recv_timeout(Duration::from_secs(5)).unwrap() {
                Output::Data(chunk) => bytes.extend(chunk),
                Output::Eof(result) => {
                    result.unwrap();
                    break;
                }
            }
        }
        assert_eq!(bytes, expected);
        writer.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn interactive_shell_editing_resize_color_and_final_output() {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let mut master = crate::open_ptm().unwrap();
        let slave = crate::open_pts(&master).unwrap();
        set_size(&master, 24, 80).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-i")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        drop(command); // No slave descriptors may remain open in the parent.
        let reader_file = master.try_clone().unwrap();
        let (send, receive) = mpsc::sync_channel(2);
        let reader =
            thread::spawn(move || read_output(reader_file, &AtomicBool::new(false), send, || {}));
        master.write_all(b"stty size; printf 'READY\\n'\r").unwrap();
        let mut output = Vec::new();
        while !output.windows(9).any(|bytes| bytes == b"\r\nREADY\r\n") {
            match receive.recv_timeout(Duration::from_secs(5)).unwrap() {
                Output::Data(bytes) => output.extend(bytes),
                Output::Eof(result) => panic!("unexpected EOF: {result:?}"),
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("24 80\r\n"));
        set_size(&master, 80, 240).unwrap();
        master
            .write_all(b"stty size; printf '\\033[31mEDITEDX\x7f\\n\\033[0mFINAL\\n'; exit\r")
            .unwrap();
        loop {
            match receive.recv_timeout(Duration::from_secs(5)).unwrap() {
                Output::Data(bytes) => output.extend(bytes),
                Output::Eof(result) => {
                    result.unwrap();
                    break;
                }
            }
        }
        assert!(child.wait().unwrap().success());
        reader.join().unwrap();
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("80 240\r\n"));
        assert!(text.contains("\x1b[31mEDITED\r\n\x1b[0mFINAL\r\n"));
    }
}
