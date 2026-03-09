pub mod tools;

use scitadel_core::config::load_config;
use scitadel_db::sqlite::Database;

/// Shared state for the MCP server.
pub struct McpState {
    pub db: Database,
}

impl McpState {
    pub fn new() -> anyhow::Result<Self> {
        let config = load_config();
        let db = Database::open(&config.db_path)?;
        db.migrate()?;
        Ok(Self { db })
    }
}
