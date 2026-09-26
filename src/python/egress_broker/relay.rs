use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
pub(super) const MAX_TUNNEL_DURATION: Duration = Duration::from_secs(30 * 60);
const MAX_BYTES_PER_DIRECTION: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Default)]
pub(super) struct RelayCounters {
    pub(super) from_left: AtomicU64,
    pub(super) from_right: AtomicU64,
}

impl RelayCounters {
    pub(super) fn get(&self) -> (u64, u64) {
        (
            self.from_left.load(Ordering::Relaxed),
            self.from_right.load(Ordering::Relaxed),
        )
    }
}

pub(super) async fn relay_bidirectional<A, B>(
    left: &mut A,
    right: &mut B,
) -> Result<(u64, u64), String>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let counters = RelayCounters::default();
    relay_bidirectional_counted(left, right, &counters).await?;
    Ok(counters.get())
}

pub(super) async fn relay_bidirectional_counted<A, B>(
    left: &mut A,
    right: &mut B,
    counters: &RelayCounters,
) -> Result<(), String>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (left_read, left_write) = tokio::io::split(left);
    let (right_read, right_write) = tokio::io::split(right);
    let from_left = copy_with_idle_counted(left_read, right_write, &counters.from_left);
    let from_right = copy_with_idle_counted(right_read, left_write, &counters.from_right);
    tokio::try_join!(from_left, from_right).map(|_| ())
}

#[cfg(test)]
pub(super) async fn copy_with_idle<R, W>(reader: R, writer: W) -> Result<u64, String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let counter = AtomicU64::new(0);
    copy_with_idle_counted(reader, writer, &counter).await?;
    Ok(counter.load(Ordering::Relaxed))
}

pub(super) async fn copy_with_idle_counted<R, W>(
    mut reader: R,
    mut writer: W,
    counter: &AtomicU64,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];
    let mut copied = counter.load(Ordering::Relaxed);
    if copied > MAX_BYTES_PER_DIRECTION {
        return Err("egress tunnel byte limit exceeded".to_string());
    }
    loop {
        let read = timeout(IDLE_TIMEOUT, reader.read(&mut buffer))
            .await
            .map_err(|_| "egress tunnel idle timeout".to_string())?
            .map_err(|error| format!("egress tunnel read failed: {error}"))?;
        if read == 0 {
            writer
                .shutdown()
                .await
                .map_err(|error| format!("egress tunnel shutdown failed: {error}"))?;
            return Ok(());
        }
        let mut offset = 0_usize;
        while offset < read {
            if copied == MAX_BYTES_PER_DIRECTION {
                return Err("egress tunnel byte limit exceeded".to_string());
            }
            let remaining = MAX_BYTES_PER_DIRECTION - copied;
            let permitted = usize::try_from(remaining.min((read - offset) as u64))
                .map_err(|_| "egress tunnel byte limit conversion failed".to_string())?;
            let written = writer
                .write(&buffer[offset..offset + permitted])
                .await
                .map_err(|error| format!("egress tunnel write failed: {error}"))?;
            if written == 0 {
                return Err("egress tunnel write failed: zero-length write".to_string());
            }
            copied = copied
                .checked_add(written as u64)
                .ok_or_else(|| "egress tunnel byte count overflowed".to_string())?;
            offset += written;
            counter.store(copied, Ordering::Relaxed);
        }
    }
}
