// The source a bridge was built from is stamped into it, and a new stamp rebuilds it.
fn main() {
    println!("cargo:rerun-if-env-changed=SPATIAL_BRIDGE_SOURCE");
}
