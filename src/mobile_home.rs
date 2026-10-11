//! Connection-local reuse of the standard SDK Catalog. No alternate discovery protocol.
use anyhow::{Result, anyhow, bail, ensure};
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, ComponentProtocolError};
use auki_components::ComponentReference;
use auki_sdk::{AukiPeer, Multiaddr, PeerId};
use futures::{StreamExt, stream};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// Diagnostics must never format authentication tokens or arbitrary SDK source chains.
fn operation_failure(error: &ComponentProtocolError) -> String {
    use auki_sdk::AukiProtocolError;
    match error {
        ComponentProtocolError::Timeout(stage) => format!("{stage}_timeout"),
        ComponentProtocolError::Sdk(AukiProtocolError::P2p(error)) => {
            use auki_p2p::Error::*;
            match error {
                AuthenticationTimeout => "authentication_timeout".into(),
                RemoteRejected => "authentication_rejected".into(),
                InvalidToken(_) | TokenVerification(_) | MissingToken | VerificationKeysStale => {
                    "authentication_credential_invalid".into()
                }
                Io(e) => format!("transport_io_{:?}", e.kind()),
                TargetedStream(e) => format!("stream_open: {e}"),
                OpenStream(e) => format!("stream_open: {e}"),
                Dial(_) => "dial_failed".into(),
                SwarmStopped => "transport_stopped".into(),
                other => format!("transport_{:?}", std::mem::discriminant(other)),
            }
        }
        ComponentProtocolError::Sdk(_) => "protocol_open_failed".into(),
        ComponentProtocolError::Wire(e) => format!("exchange_wire: {e}"),
        ComponentProtocolError::Codec(e) => format!("response_decode: {e}"),
        ComponentProtocolError::InvalidResponse(e) => format!("response_validation: {e}"),
        ComponentProtocolError::InvalidRequest(_) => "invalid_request".into(),
        ComponentProtocolError::Close(_) => "stream_cleanup_failed".into(),
        _ => "component_protocol_failed".into(),
    }
    .chars()
    .filter(|c| !c.is_control())
    .take(300)
    .collect()
}

struct Resolved {
    route: Multiaddr,
    components: HashMap<String, ComponentReference>,
    received: Instant,
    generation: u64,
}

/// SDK open/authentication failed before operation_opened can send the request.
/// Retrying these transient admission failures cannot replay an operation.
fn retryable_open_failure(error: &ComponentProtocolError) -> bool {
    matches!(error,
        ComponentProtocolError::Sdk(auki_sdk::AukiProtocolError::P2p(auki_p2p::Error::Io(error)))
        if matches!(error.kind(), std::io::ErrorKind::WriteZero | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted))
}
#[derive(Default)]
struct Cache {
    resolved: Option<Resolved>,
    generation: u64,
    retry_after: Option<Instant>,
}
pub struct HomeOperations {
    client: ComponentProtocolClient,
    expected: PeerId,
    routes: Vec<Multiaddr>,
    pub frame: String,
    cache: Mutex<Cache>,
}
impl HomeOperations {
    pub fn new(peer: &AukiPeer, home: &crate::network::HomeConnection) -> Result<Self> {
        let expected = home.validate(peer)?;
        ensure!(
            !home.routes.is_empty() && home.routes.len() <= 16,
            "Home needs 1–16 routes"
        );
        Ok(Self {
            client: ComponentProtocolClient::new(peer.protocols()),
            expected,
            routes: home
                .routes
                .iter()
                .map(|r| r.parse().map_err(anyhow::Error::from))
                .collect::<Result<_>>()?,
            frame: home.frame.clone(),
            cache: Mutex::new(Cache::default()),
        })
    }

    async fn discover(&self, preferred: Option<Multiaddr>) -> Result<Resolved> {
        let mut routes = self.routes.clone();
        if let Some(route) = preferred {
            routes.retain(|r| r != &route);
            routes.insert(0, route);
        }
        // Read-only route attempts may race; writes never race. A dead first
        // route must not prevent another advertised route from being tried.
        let work = async {
            let mut attempts = stream::iter(routes)
                .map(|route| async move {
                    let result = tokio::time::timeout(
                        Duration::from_secs(4),
                        self.client
                            .catalog_exact(self.expected, route.clone(), None),
                    )
                    .await;
                    (route, result)
                })
                .buffer_unordered(2);
            while let Some((route, result)) = attempts.next().await {
                if let Ok(Ok(CatalogResponse::Snapshot { snapshot })) = result {
                    return Ok(Resolved {
                        route,
                        received: Instant::now(),
                        generation: 0,
                        components: snapshot
                            .components
                            .into_iter()
                            .map(|c| (c.manifest.component_id.clone(), c.manifest.reference()))
                            .collect(),
                    });
                }
            }
            bail!(
                "Home component discovery failed; no operation was sent. Retry when the connection recovers"
            )
        };
        tokio::time::timeout(Duration::from_secs(8), work)
            .await
            .map_err(|_| {
                anyhow!("Home component discovery timed out; no operation was sent. Retry")
            })?
    }

    async fn catalog(&self) -> Result<tokio::sync::MutexGuard<'_, Cache>> {
        let mut cache = self.cache.lock().await;
        if cache
            .resolved
            .as_ref()
            .is_some_and(|r| r.received.elapsed() < Duration::from_secs(60))
        {
            return Ok(cache);
        }
        if cache.retry_after.is_some_and(|t| Instant::now() < t) {
            bail!("Home component discovery is recovering; no operation was sent. Retry shortly");
        }
        let preferred = cache.resolved.as_ref().map(|r| r.route.clone());
        match self.discover(preferred).await {
            Ok(mut result) => {
                cache.generation += 1;
                result.generation = cache.generation;
                cache.resolved = Some(result);
                cache.retry_after = None;
            }
            Err(error) => {
                cache.resolved = None;
                cache.retry_after = Some(Instant::now() + Duration::from_secs(2));
                return Err(error);
            }
        }
        Ok(cache)
    }
    pub async fn warm(&self) -> Result<()> {
        drop(self.catalog().await?);
        Ok(())
    }
    /// Successful SDK Catalog fetches for diagnostics and regression tests.
    pub async fn catalog_refresh_count(&self) -> u64 {
        self.cache.lock().await.generation
    }

    async fn invalidate(&self, generation: u64) {
        let mut cache = self.cache.lock().await;
        if let Some(resolved) = &mut cache.resolved {
            if resolved.generation == generation {
                resolved.received = Instant::now() - Duration::from_secs(60);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn invoke<I: Serialize, R: DeserializeOwned>(
        &self,
        component: &str,
        operation: &str,
        caller: &str,
        request_id: &str,
        deadline: Duration,
        request: &I,
    ) -> Result<R> {
        for attempt in 0..3 {
            let (route, reference, generation) = {
                let cache = self.catalog().await?;
                let resolved = cache.resolved.as_ref().unwrap();
                let reference = resolved.components.get(component).ok_or_else(|| {
                    anyhow!("Home is connected, but the requested component is not advertised")
                })?;
                (
                    resolved.route.clone(),
                    reference.clone(),
                    resolved.generation,
                )
            };
            let started = Instant::now();
            let result = tokio::time::timeout(
                deadline + Duration::from_secs(2),
                self.client.invoke_exact::<_, R>(
                    self.expected,
                    route,
                    reference,
                    operation,
                    caller,
                    request_id,
                    Some(deadline),
                    request,
                ),
            )
            .await;
            match result {
                Ok(Ok(reply)) => return Ok(reply.result),
                Ok(Err(ref error)) if attempt < 2 && retryable_open_failure(error) => {
                    // libp2p's bounded incoming queue can reject simultaneous opens.
                    // Back off only before payload delivery; never replay an ambiguous write.
                    tokio::time::sleep(Duration::from_millis(50 << attempt)).await;
                }
                Ok(Err(ComponentProtocolError::RemoteOperation(ref error)))
                    if attempt < 2 && error.code == "manifest_mismatch" =>
                {
                    // SDK rejected the stale reference before running the handler.
                    self.invalidate(generation).await;
                }
                Ok(Err(error @ ComponentProtocolError::RemoteOperation(_)))
                | Ok(Err(error @ ComponentProtocolError::RemoteRejected { .. })) => {
                    return Err(error.into());
                }
                failure => {
                    let reason = match failure {
                        Err(_) => "operation_deadline".to_owned(),
                        Ok(Err(error)) => operation_failure(&error),
                        _ => unreachable!(),
                    };
                    self.invalidate(generation).await;
                    // Delivery may have succeeded. Never automatically replay an
                    // ambiguous write. UI Retry keeps the same deduplicated ID.
                    bail!(
                        "Home did not confirm the operation ({reason}, {} ms); retry the same request",
                        started.elapsed().as_millis()
                    );
                }
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_transient_pre_payload_open_errors_are_retryable() {
        use auki_sdk::AukiProtocolError;
        use std::io::{Error, ErrorKind};
        for kind in [
            ErrorKind::WriteZero,
            ErrorKind::UnexpectedEof,
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
        ] {
            assert!(retryable_open_failure(&ComponentProtocolError::Sdk(
                AukiProtocolError::P2p(auki_p2p::Error::Io(Error::from(kind)))
            )));
        }
        for error in [
            ComponentProtocolError::Sdk(AukiProtocolError::P2p(auki_p2p::Error::RemoteRejected)),
            ComponentProtocolError::Sdk(AukiProtocolError::P2p(auki_p2p::Error::Io(Error::from(
                ErrorKind::PermissionDenied,
            )))),
            ComponentProtocolError::Close("write zero after reply".into()),
            ComponentProtocolError::Wire("write zero while sending payload".into()),
            ComponentProtocolError::Codec("invalid response".into()),
            ComponentProtocolError::Timeout(
                auki_component_protocol::ComponentProtocolOperation::Exchange,
            ),
        ] {
            assert!(!retryable_open_failure(&error));
        }
    }
    #[test]
    fn operation_diagnostics_distinguish_stage_without_auth_secrets() {
        let error = ComponentProtocolError::Sdk(auki_sdk::AukiProtocolError::P2p(
            auki_p2p::Error::InvalidToken("must-not-appear".into()),
        ));
        assert_eq!(
            operation_failure(&error),
            "authentication_credential_invalid"
        );
        assert_eq!(
            operation_failure(&ComponentProtocolError::Timeout(
                auki_component_protocol::ComponentProtocolOperation::Open
            )),
            "open_timeout"
        );
        assert_eq!(
            operation_failure(&ComponentProtocolError::Close("private".into())),
            "stream_cleanup_failed"
        );
    }
}
