//! Shared sensing and mapping runtime with the galbot capture adapter.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    spatial_home::robot_runner::run(spatial_home::robot_runner::Hardware::Galbot).await
}
