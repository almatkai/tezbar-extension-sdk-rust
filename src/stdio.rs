//! A fixed process-owned stdin pump avoids Tokio's uninterruptible stdin work
//! keeping its blocking pool alive when `shutdown` arrives with the pipe open.
//! The single OS thread exits on EOF or process exit; its queue is bounded.
use std::{
    io::{self, Read},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::mpsc,
};
struct Input {
    receiver: mpsc::Receiver<io::Result<Vec<u8>>>,
    current: Vec<u8>,
    offset: usize,
}
impl AsyncRead for Input {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.offset == self.current.len() {
            match self.receiver.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(Some(Ok(bytes))) => {
                    self.current = bytes;
                    self.offset = 0;
                }
            }
        }
        let count = output.remaining().min(self.current.len() - self.offset);
        output.put_slice(&self.current[self.offset..self.offset + count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}
pub(crate) fn input() -> io::Result<impl AsyncRead + Send + Unpin> {
    let (sender, receiver) = mpsc::channel(2);
    std::thread::Builder::new()
        .name("extension-stdin".into())
        .spawn(move || {
            let stdin = io::stdin();
            let mut stdin = stdin.lock();
            loop {
                let mut bytes = vec![0u8; 8192];
                match stdin.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(count) => {
                        bytes.truncate(count);
                        if sender.blocking_send(Ok(bytes)).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        let _ = sender.blocking_send(Err(e));
                        break;
                    }
                }
            }
        })?;
    Ok(Input {
        receiver,
        current: Vec::new(),
        offset: 0,
    })
}
