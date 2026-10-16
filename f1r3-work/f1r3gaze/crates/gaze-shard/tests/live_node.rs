//! Opt-in compatibility check against an installed F1R3Node HTTP API.
//!
//! Set F1R3NODE_BASE_URL to the standalone node's local API address. The
//! ordinary crate test suite does not require a node process.

use gaze_net::Http;
use gaze_shard::node::Node;
use serde_json::json;

#[test]
#[ignore = "requires F1R3NODE_BASE_URL and a running node"]
fn packaged_node_api_matches_gaze_client() {
    let base = std::env::var("F1R3NODE_BASE_URL").expect("set F1R3NODE_BASE_URL");
    let node = Node::new(&base, Http::new());
    let (hash, height) = node.last_finalized().expect("last finalized block");
    assert_eq!(
        hash.len(),
        64,
        "finalized block hash must be 32 bytes in hex"
    );
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(height >= 0);

    let (entries, registry_hash, registry_height) = node
        .registry("rho:id:missing", Some(&hash))
        .expect("missing registry entry at finalized block");
    assert!(
        entries.is_empty(),
        "a missing test registry entry returned data"
    );
    assert_eq!(registry_hash, hash);
    assert_eq!(registry_height, height);

    let (result, explored_hash, cost) = node
        .explore("new return in { return!(1) }")
        .expect("read-only exploratory deploy");
    assert_eq!(result, vec![json!({"ExprInt": {"data": 1}})]);
    assert_eq!(explored_hash, hash);
    assert!(cost > 0);
}
