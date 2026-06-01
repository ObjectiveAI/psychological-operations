//! Build-time-embedded `psychological-operations-x-api-mcp` binary +
//! runtime extract.
//!
//! The binary bytes are inlined via `include_bytes!(env!(...))` at
//! compile time (the env var is set by `psychological-operations-cli`'s
//! `build.rs` after `psychological-operations-x-api-mcp/validate.sh`
//! confirms the pre-built embed binary is current).
//!
//! Extract location is a content-hashed temp dir
//! `${TMPDIR}/psychological-operations-x-api-mcp-bin-<hash:016x>/` —
//! mirrors `objectiveai-api/src/agent/completions/claude_agent_sdk/client.rs`'s
//! pattern verbatim. Different psyop builds with different embedded
//! bytes resolve to different hashes → no clobbering across versions.

use std::path::PathBuf;

/// The X-API MCP binary embedded at build time. Path resolved by
/// `psychological-operations-cli/build.rs` via
/// `psychological-operations-x-api-mcp/validate.sh`.
pub const X_API_MCP_BINARY: &[u8] = include_bytes!(env!("PSYOPS_X_API_MCP_BINARY_PATH"));

#[cfg(windows)]
pub const X_API_MCP_BINARY_NAME: &str = "psychological-operations-x-api-mcp.exe";
#[cfg(not(windows))]
pub const X_API_MCP_BINARY_NAME: &str = "psychological-operations-x-api-mcp";

/// Idempotently extract the embedded X-API MCP binary into a
/// per-binary-hash temp dir. Subsequent calls with the same embedded
/// bytes are a single `try_exists` hop.
pub async fn ensure_extracted() -> std::io::Result<PathBuf> {
    use std::hash::{Hash, Hasher};

    let mut h = std::collections::hash_map::DefaultHasher::new();
    X_API_MCP_BINARY.len().hash(&mut h);
    X_API_MCP_BINARY[..X_API_MCP_BINARY.len().min(4096)].hash(&mut h);
    X_API_MCP_BINARY[X_API_MCP_BINARY.len().saturating_sub(4096)..].hash(&mut h);
    let hash = h.finish();

    let dir = std::env::temp_dir()
        .join(format!("psychological-operations-x-api-mcp-bin-{hash:016x}"));
    let path = dir.join(X_API_MCP_BINARY_NAME);

    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&path, X_API_MCP_BINARY).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o755),
            )
            .await?;
        }
    }
    Ok(path)
}
