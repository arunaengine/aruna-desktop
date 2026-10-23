use serde_json::Value;

fn read(text: &str) -> Value {
    serde_json::from_str(text).expect("a JSON file")
}

#[test]
fn matches_pinned_node() {
    // The desktop ships exactly one node release, so both carry one version.
    let pins = read(include_str!("../versions.json"));
    let config = read(include_str!("../tauri.conf.json"));
    let node = pins["aruna"]["version"].as_str().expect("a node version");

    assert_eq!(env!("CARGO_PKG_VERSION"), node);
    assert_eq!(config["version"].as_str(), Some(node));
}
