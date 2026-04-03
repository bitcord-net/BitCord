// NodeAddr is now defined in the bitcord-dht crate and re-exported here for
// backwards compatibility.  All existing code that uses `crate::network::NodeAddr`
// continues to work without any changes.
pub use bitcord_dht::NodeAddr;
