#[allow(dead_code)]
mod support;
use auki_component_protocol::{CatalogResponse, ComponentProtocolClient, ComponentProtocolEndpoint};
use auki_components::ComponentRuntime;

/// Open TCP connections this process holds from a dialer to `port`.
fn outbound_connections(port: u16) -> usize {
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-a", "-p", &std::process::id().to_string(), "-iTCP", "-sTCP:ESTABLISHED"])
        .output()
        .expect("lsof");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.contains(&format!("->127.0.0.1:{port}")))
        .count()
}

/// Every request to a peer at a known address used to dial a brand-new
/// connection, so a Home held 100+ open connections to one phone. Requests
/// now share the open connection; each stream still authenticates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_exact_requests_share_one_connection() -> anyhow::Result<()> {
    let pair = support::LocalPair::start().await?;
    let result = async {
        let runtime = ComponentRuntime::new(pair.b.peer_id().to_string());
        let endpoint = ComponentProtocolEndpoint::mount(pair.b.protocols(), runtime)?;
        let route = pair.b.listen_addresses().into_iter()
            .find(|a| a.to_string().starts_with("/ip4/127.0.0.1/tcp/"))
            .ok_or_else(|| anyhow::anyhow!("no loopback route"))?;
        let port: u16 = route.to_string().rsplit('/').next().unwrap().parse()?;
        let client = ComponentProtocolClient::new(pair.a.protocols());
        for _ in 0..20 {
            let response = client.catalog_exact(pair.b.peer_id(), route.clone(), None).await?;
            assert!(matches!(response, CatalogResponse::Snapshot { .. }));
        }
        let open = outbound_connections(port);
        assert!(open <= 2, "20 requests left {open} connections open");
        endpoint.close().await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    pair.shutdown().await?;
    result
}
