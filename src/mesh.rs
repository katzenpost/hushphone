//! Reaching a Pigeonhole replica on a Reticulum mesh.

use std::path::{Path, PathBuf};

/// namenlos's bridge replica, shared by the mesh and the mixnet.
pub const BRIDGE: &str = "162.220.11.225:4242";
/// namenlos's box plaintext size.
pub const BOX_SIZE: usize = 1549;

/// Writes the Reticulum config for one TCP link to `replica` (`host:port`) under `data_dir` and
/// returns its directory. Written on every start, so a changed address takes effect; the identity
/// and storage beside it stay. Same layout as katzenrns's `--replica`.
pub fn replica_config(data_dir: &Path, replica: &str) -> Result<PathBuf, String> {
    let (host, port) = host_port(replica)?;
    let dir = data_dir.join("reticulum");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let config = format!(
        "# written by hushphone --replica; edit the flag, not this file\n\
         [reticulum]\n  enable_transport = No\n  share_instance = No\n\
         [interfaces]\n  [[replica]]\n    type = TCPClientInterface\n    enabled = yes\n\
         \x20   target_host = {host}\n    target_port = {port}\n"
    );
    std::fs::write(dir.join("config"), config).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// `replica`'s host, without brackets, and port; an error naming `--replica` if it is not
/// `host:port`.
pub fn host_port(replica: &str) -> Result<(&str, &str), String> {
    let bad = || format!("--replica wants HOST:PORT, got {replica:?}");
    let (host, port) = replica
        .rsplit_once(':')
        .filter(|(h, p)| !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(bad)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.chars().any(|c| c.is_whitespace() || c == '[' || c == ']') {
        return Err(bad());
    }
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_tcp_interface() {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("mesh-test-{}", std::process::id()));
        let dir = replica_config(&d, "162.220.11.225:4242").unwrap();
        let c = std::fs::read_to_string(dir.join("config")).unwrap();
        assert!(c.contains("    target_host = 162.220.11.225\n    target_port = 4242\n"), "{c}");
        assert!(replica_config(&d, "[::1]:4242").is_ok());
        for bad in ["nohost", ":4242", "h:", "h:42x", "a b:1"] {
            assert!(replica_config(&d, bad).is_err(), "{bad}");
        }
        std::fs::remove_dir_all(d).unwrap();
    }
}
