use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(windows)]
use std::time::Duration;

pub(crate) struct BootstrapLineReader {
    receiver: tokio::sync::oneshot::Receiver<io::Result<String>>,
    cancellation: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BootstrapLineReader {
    pub(crate) fn spawn() -> io::Result<Self> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let thread_cancellation = cancellation.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("lethetic-terminal-line-input".to_string())
            .spawn(move || {
                let _ = sender.send(read_bootstrap_line(&thread_cancellation));
            })?;
        Ok(Self {
            receiver,
            cancellation,
            thread: Some(thread),
        })
    }

    pub(crate) async fn receive(&mut self) -> io::Result<String> {
        let result = (&mut self.receiver).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal line input reader stopped unexpectedly",
            )
        })?;
        self.join()?;
        result
    }

    pub(crate) fn cancel_and_join(&mut self) -> io::Result<()> {
        self.cancellation.store(true, Ordering::SeqCst);
        self.join()
    }

    fn join(&mut self) -> io::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| io::Error::other("terminal line input reader panicked unexpectedly"))
    }
}

impl Drop for BootstrapLineReader {
    fn drop(&mut self) {
        let _ = self.cancel_and_join();
    }
}

#[cfg(unix)]
fn read_bootstrap_line(cancellation: &AtomicBool) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        if cancellation.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "terminal line input was cancelled",
            ));
        }
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let polled = unsafe { libc::poll(&mut descriptor, 1, 50) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if polled == 0 {
            continue;
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("terminal line input failed"));
        }
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            continue;
        }
        let mut byte = 0_u8;
        let read = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                (&mut byte as *mut u8).cast::<libc::c_void>(),
                1,
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            return String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
        bytes.push(byte);
        if bytes.len() > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "terminal line input is too long",
            ));
        }
        if byte == b'\n' {
            return String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
}

#[cfg(windows)]
fn read_bootstrap_line(cancellation: &AtomicBool) -> io::Result<String> {
    use crossterm::event::{Event, KeyCode, KeyEventKind};

    loop {
        if cancellation.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "terminal line input was cancelled",
            ));
        }
        if !crossterm::event::poll(Duration::from_millis(50))? {
            continue;
        }
        match crossterm::event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Enter => {
                return Ok("\n".to_string());
            }
            Event::Paste(value) if value.ends_with('\n') => return Ok(value),
            _ => {}
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn read_bootstrap_line(_cancellation: &AtomicBool) -> io::Result<String> {
    let mut acknowledgement = String::new();
    io::stdin().read_line(&mut acknowledgement)?;
    Ok(acknowledgement)
}
