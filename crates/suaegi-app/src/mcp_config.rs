use std::path::{Path, PathBuf};

use suaegi_mcp::{
    inspect_mcp_config_content, McpConfigInspection, MCP_CONFIG_CANDIDATES, MCP_STARTER_CONFIG,
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LoadedMcpConfig {
    pub absolute_path: PathBuf,
    pub inspection: McpConfigInspection,
    pub read_error: Option<String>,
}

pub fn inspect_root(root: &Path) -> Vec<LoadedMcpConfig> {
    MCP_CONFIG_CANDIDATES
        .iter()
        .copied()
        .map(|candidate| {
            let absolute_path = root.join(candidate.relative_path);
            let (content, read_error) = match std::fs::metadata(&absolute_path) {
                Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES => {
                    match std::fs::read_to_string(&absolute_path) {
                        Ok(content) => (Some(content), None),
                        Err(error) => (None, Some(error.to_string())),
                    }
                }
                Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => (
                    None,
                    Some("File is larger than the 1 MiB inspection limit.".to_string()),
                ),
                Ok(_) => (None, Some("Path is not a regular file.".to_string())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
                Err(error) => (None, Some(error.to_string())),
            };
            let inspection = inspect_mcp_config_content(candidate, content.as_deref());
            LoadedMcpConfig {
                absolute_path,
                inspection,
                read_error,
            }
        })
        .collect()
}

pub fn create_starter(root: &Path) -> Result<PathBuf, String> {
    let target = root.join(".mcp.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&target)
        .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
    use std::io::Write;
    file.write_all(MCP_STARTER_CONFIG.as_bytes())
        .map_err(|error| format!("Could not write {}: {error}", target.display()))?;
    file.sync_all()
        .map_err(|error| format!("Could not sync {}: {error}", target.display()))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_mcp::McpConfigStatus;

    #[test]
    fn inspection_finds_all_orca_candidates_and_masks_secrets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"demo":{"command":"npx","env":{"TOKEN":"secret"}}}}"#,
        )
        .unwrap();
        let loaded = inspect_root(dir.path());
        assert_eq!(loaded.len(), MCP_CONFIG_CANDIDATES.len());
        let workspace = &loaded[0].inspection;
        assert_eq!(workspace.status, McpConfigStatus::Valid);
        assert_eq!(workspace.servers.len(), 1);
        assert_eq!(
            workspace.servers[0].env.as_ref().unwrap()[0].1,
            suaegi_mcp::MASKED_ENV_VALUE
        );
    }

    #[test]
    fn starter_creation_is_exclusive_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_starter(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), MCP_STARTER_CONFIG);
        assert!(create_starter(dir.path()).is_err());
    }
}
