//! Authenticated application stream shared by native and browser runtimes.

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::io::{AsyncRead, AsyncWrite};
use libp2p::Stream;

use crate::AuthenticatedPeer;

// Process-local application bytes only: excludes authentication, transport
// framing, relay control, HTTP and retransmissions. No payloads are retained.
static TRAFFIC: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));

/// Cumulative (received, sent) authenticated application bytes in this process.
pub fn application_traffic_totals() -> (u64, u64) {
    *TRAFFIC.lock().unwrap_or_else(|error| error.into_inner())
}

fn count_io(result: &Poll<std::io::Result<usize>>, received: bool) {
    if let Poll::Ready(Ok(size)) = result {
        if *size == 0 { return; }
        let mut totals = TRAFFIC.lock().unwrap_or_else(|error| error.into_inner());
        let counter = if received { &mut totals.0 } else { &mut totals.1 };
        *counter = counter.saturating_add(*size as u64);
    }
}

/// The public byte-stream boundary. The inner libp2p stream is deliberately not
/// exposed and this wrapper can only be constructed after mutual DDS auth.
pub struct AuthenticatedStream {
    inner: Stream,
    remote: AuthenticatedPeer,
    traffic: Option<crate::traffic::Counter>,
}

impl AuthenticatedStream {
    pub(crate) fn new(inner: Stream, remote: AuthenticatedPeer, local: crate::PeerId) -> Self {
        let traffic=crate::traffic::open(&local.to_string(),&remote.peer_id.to_string());
        Self { inner, remote, traffic }
    }

    pub fn remote_peer(&self) -> &AuthenticatedPeer {
        &self.remote
    }
}

impl std::fmt::Debug for AuthenticatedStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedStream")
            .field("remote", &self.remote)
            .finish_non_exhaustive()
    }
}

impl AsyncRead for AuthenticatedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_read(context, buffer);
        count_io(&result, true);
        crate::traffic::record(&self.traffic,&result,true);
        result
    }
}

impl AsyncWrite for AuthenticatedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(context, buffer);
        count_io(&result, false);
        crate::traffic::record(&self.traffic,&result,false);
        result
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(context)
    }
}

#[cfg(test)]
mod traffic_tests {
    use super::*;
    #[test]
    fn counts_only_successful_io_in_each_direction() {
        let before = application_traffic_totals();
        count_io(&Poll::Pending, true);
        count_io(&Poll::Ready(Err(std::io::Error::other("test"))), false);
        count_io(&Poll::Ready(Ok(0)), true);
        assert_eq!(application_traffic_totals(), before);
        count_io(&Poll::Ready(Ok(13)), true);
        count_io(&Poll::Ready(Ok(7)), false);
        assert_eq!(application_traffic_totals(), (before.0 + 13, before.1 + 7));
    }
}

impl Drop for AuthenticatedStream {
    fn drop(&mut self){crate::traffic::close(&self.traffic);}
}
