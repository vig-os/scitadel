pub mod bib_import;
pub mod bib_rekey;
pub mod bib_watch;
pub mod events;
pub mod extract;
pub mod paths;
pub mod project_pin;
#[allow(clippy::unused_self, clippy::needless_pass_by_value)]
pub mod server;
pub mod tools;

/// Crate-wide mutex every test that mutates `SCITADEL_*` / `HOME`
/// must hold. The path-scoping tests (paths.rs) and the end-to-end
/// MCP tests (server.rs) both touch those env vars; without one
/// shared lock a cross-module race leaks another test's workspace
/// override into the observing test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use scitadel_db::sqlite::Database;

/// Shared state for the MCP server.
pub struct McpState {
    pub db: Database,
}

impl McpState {
    /// Open the DB at `<project_root>/.scitadel/scitadel.db` — resolved
    /// through the server's PINNED project (round 5). The MCP server
    /// never creates a `.scitadel` marker, and never confines against
    /// the launching directory (#266). If no project marker is
    /// visible, this returns the project resolver's error unchanged;
    /// callers surface it as a tool-level refusal.
    ///
    /// The pin is lazy on the first project-aware call; this call
    /// itself triggers it, so building `McpState` in a project dir
    /// locks the server to that dir for its lifetime.
    pub fn new() -> anyhow::Result<Self> {
        let db_path = crate::project_pin::pinned_project_db_path()?;
        let db = Database::open(&db_path)?;
        db.migrate()?;
        Ok(Self { db })
    }
}
