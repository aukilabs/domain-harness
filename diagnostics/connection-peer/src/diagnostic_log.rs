//! Diagnostic disk writes must never block capture or the libp2p runtime.
use std::sync::{Mutex, atomic::{AtomicU64, Ordering}, mpsc::{self, SyncSender}};
use std::thread::JoinHandle;

static SENDER: Mutex<Option<SyncSender<String>>> = Mutex::new(None);
static DROPPED: AtomicU64 = AtomicU64::new(0);
const MAX_MESSAGE: usize = 4096;

pub struct Guard(Option<JoinHandle<()>>);
pub fn start() -> std::io::Result<Guard> {
    let (send, receive) = mpsc::sync_channel::<String>(64);
    let mut slot = SENDER.lock().unwrap();
    if slot.is_some() { return Err(std::io::Error::other("Diagnostic logger already running")); }
    let worker = std::thread::Builder::new().name("diagnostic-log".into()).spawn(move || {
        use std::io::Write;
        while let Ok(message) = receive.recv() {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{message}");
            let dropped = DROPPED.swap(0, Ordering::Relaxed);
            if dropped != 0 { let _ = writeln!(stderr, "Diagnostic log queue full: dropped {dropped} messages"); }
        }
    })?;
    *slot = Some(send);
    Ok(Guard(Some(worker)))
}
pub fn record(args: std::fmt::Arguments<'_>) {
    let Ok(slot) = SENDER.try_lock() else { DROPPED.fetch_add(1,Ordering::Relaxed); return; };
    let Some(send) = slot.as_ref() else { return; };
    enqueue(send, args.to_string());
}
fn enqueue(send: &SyncSender<String>, mut message: String) {
    if message.len() > MAX_MESSAGE {
        let mut end=MAX_MESSAGE; while !message.is_char_boundary(end) { end-=1; }
        message.truncate(end);
    }
    if send.try_send(message).is_err() { DROPPED.fetch_add(1,Ordering::Relaxed); }
}
impl Drop for Guard {
    fn drop(&mut self) {
        // Called after network/capture shutdown; join the bounded writer, don't
        // leave a detached logging thread or unbounded task behind.
        SENDER.lock().unwrap().take();
        if let Some(worker)=self.0.take() {let _=worker.join();}
    }
}
#[macro_export]
macro_rules! diagnostic_log {
    ($($args:tt)*) => { $crate::diagnostic_log::record(format_args!($($args)*)) };
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_writer_cannot_block_producer_or_grow_queue() {
        let (send, receive)=mpsc::sync_channel(1);
        enqueue(&send,"🦀".repeat(2000));
        // No reader draining the queue: a second write must return, not wait.
        enqueue(&send,"discarded".into());
        assert_eq!(receive.try_recv().unwrap().len(),MAX_MESSAGE);
        assert!(receive.try_recv().is_err());
    }
}
