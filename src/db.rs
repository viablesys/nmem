use crate::schema::MIGRATIONS;
use crate::NmemError;
use rusqlite::Connection;
use std::path::Path;

#[cfg(unix)]
fn ensure_secure_permissions(db_path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let nmem_dir = db_path.parent().expect("db_path has parent");
    if !nmem_dir.exists() {
        std::fs::create_dir_all(nmem_dir)?;
        // Only set permissions on dirs we created
        std::fs::set_permissions(nmem_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    if db_path.exists() {
        std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_secure_permissions(db_path: &Path) -> std::io::Result<()> {
    let nmem_dir = db_path.parent().expect("db_path has parent");
    if !nmem_dir.exists() {
        std::fs::create_dir_all(nmem_dir)?;
    }
    Ok(())
}

pub fn open_db_readonly(db_path: &Path) -> Result<Connection, NmemError> {
    if !db_path.exists() {
        return Err(NmemError::Config(format!(
            "database not found: {}",
            db_path.display()
        )));
    }

    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    Ok(conn)
}

pub fn open_db(db_path: &Path) -> Result<Connection, NmemError> {
    ensure_secure_permissions(db_path)?;

    let mut conn = Connection::open(db_path)?;

    // PRAGMAs — must be set before migrations
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // auto_vacuum must be set before first table creation.
    // On an existing DB this is a no-op (can't change after tables exist),
    // but on first run it takes effect.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;

    MIGRATIONS.to_latest(&mut conn)?;

    // Set file permissions after DB creation (Connection::open creates if missing)
    #[cfg(unix)]
    {
        if db_path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))?;
        }
    }

    Ok(conn)
}
