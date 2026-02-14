use crate::config::load_config;
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

// --- Key management ---

/// Load encryption key: NMEM_KEY env var > config key_file > ~/.nmem/key > None.
pub fn load_key() -> Option<String> {
    if let Ok(k) = std::env::var("NMEM_KEY")
        && !k.is_empty()
    {
        return Some(k);
    }

    // Check config for custom key file path
    let key_path = if let Ok(config) = load_config() {
        config
            .encryption
            .key_file
            .unwrap_or_else(default_key_path)
    } else {
        default_key_path()
    };

    if key_path.exists()
        && let Ok(k) = std::fs::read_to_string(&key_path)
    {
        let trimmed = k.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    None
}

/// Load key or create one. Returns the hex key.
fn load_or_create_key() -> Result<String, NmemError> {
    if let Some(k) = load_key() {
        return Ok(k);
    }

    let key = generate_random_key()?;
    let key_path = if let Ok(config) = load_config() {
        config
            .encryption
            .key_file
            .unwrap_or_else(default_key_path)
    } else {
        default_key_path()
    };

    write_key_file(&key_path, &key)?;
    Ok(key)
}

fn default_key_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(home).join(".nmem").join("key")
}

/// Generate 32 random bytes, hex-encoded to 64 chars.
fn generate_random_key() -> Result<String, NmemError> {
    let mut buf = [0u8; 32];
    let mut file = std::fs::File::open("/dev/urandom")?;
    std::io::Read::read_exact(&mut file, &mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Write key file with 0600 permissions.
fn write_key_file(path: &std::path::Path, key: &str) -> Result<(), NmemError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    std::fs::write(path, key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Apply encryption key to a connection. MUST be the first statement.
/// Uses raw hex key format (x'...') to skip PBKDF2.
fn apply_key(conn: &Connection, key: &str) -> Result<(), NmemError> {
    let pragma_value = format!("x'{key}'");
    conn.pragma_update(None, "key", &pragma_value)?;
    // Verify key works
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|_| NmemError::Config("wrong encryption key or corrupt database".into()))?;
    Ok(())
}

/// Check if a database file is encrypted (unreadable without key).
pub fn is_db_encrypted(db_path: &Path) -> bool {
    if !db_path.exists() {
        return false;
    }
    // Try opening without a key — if sqlite_master query fails, it's encrypted
    if let Ok(conn) = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_err()
    } else {
        false
    }
}

/// Apply standard PRAGMAs (after key, before migrations).
fn apply_pragmas(conn: &Connection, readonly: bool) -> Result<(), NmemError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    if !readonly {
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    }
    Ok(())
}

// --- Public open functions ---

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

    if let Some(key) = load_key() {
        apply_key(&conn, &key)?;
    }

    apply_pragmas(&conn, true)?;
    Ok(conn)
}

pub fn open_db(db_path: &Path) -> Result<Connection, NmemError> {
    ensure_secure_permissions(db_path)?;

    let is_new = !db_path.exists();
    let mut conn = Connection::open(db_path)?;

    if let Some(key) = load_key() {
        if is_new {
            // New DB: just apply key
            apply_key(&conn, &key)?;
        } else {
            // Existing DB: try encrypted first
            match apply_key(&conn, &key) {
                Ok(()) => {}
                Err(_) => {
                    // Key failed — might be unencrypted DB needing migration
                    drop(conn);
                    migrate_to_encrypted(db_path, &key)?;
                    conn = Connection::open(db_path)?;
                    apply_key(&conn, &key)?;
                }
            }
        }
    }

    apply_pragmas(&conn, false)?;
    MIGRATIONS.to_latest(&mut conn)?;

    // Set file permissions after DB creation
    #[cfg(unix)]
    {
        if db_path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))?;
        }
    }

    Ok(conn)
}

// --- Migration ---

/// Migrate an unencrypted database to encrypted.
/// Opens the plain DB, exports to a new encrypted file, then swaps.
fn migrate_to_encrypted(db_path: &Path, key: &str) -> Result<(), NmemError> {
    let encrypted_path = db_path.with_extension("db-encrypting");
    let backup_path = db_path.with_extension("db-unencrypted-backup");

    // Open unencrypted source
    let conn = Connection::open(db_path)?;
    // Verify it's actually unencrypted
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .map_err(|_| NmemError::Config("database is neither unencrypted nor matches the provided key".into()))?;

    // Attach encrypted destination
    conn.execute("ATTACH DATABASE ?1 AS encrypted", [encrypted_path.to_str().unwrap()])?;
    let pragma_value = format!("x'{key}'");
    conn.pragma_update(Some("encrypted"), "key", &pragma_value)?;

    // Export
    conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
    conn.execute_batch("DETACH DATABASE encrypted")?;
    drop(conn);

    // Atomic swap: original → backup, encrypted → original
    std::fs::rename(db_path, &backup_path)?;
    std::fs::rename(&encrypted_path, db_path)?;

    // Clean up WAL/SHM from the old unencrypted DB
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));

    eprintln!("nmem: migrated database to encrypted format");
    eprintln!(
        "nmem: unencrypted backup at {}",
        backup_path.display()
    );
    eprintln!("nmem: verify and delete the backup manually");

    Ok(())
}

// --- Encrypt subcommand ---

pub fn handle_encrypt(db_path: &Path) -> Result<(), NmemError> {
    if !db_path.exists() {
        return Err(NmemError::Config(format!(
            "database not found: {}",
            db_path.display()
        )));
    }

    if is_db_encrypted(db_path) {
        eprintln!("nmem: database is already encrypted");
        return Ok(());
    }

    let key = load_or_create_key()?;
    migrate_to_encrypted(db_path, &key)?;

    // Verify the encrypted DB works
    let conn = Connection::open(db_path)?;
    apply_key(&conn, &key)?;
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master",
        [],
        |r| r.get(0),
    )?;
    eprintln!("nmem: encryption verified ({count} tables/indexes accessible)");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_key_is_64_hex_chars() {
        let key = generate_random_key().unwrap();
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn apply_key_to_in_memory_db() {
        let conn = Connection::open_in_memory().unwrap();
        let key = "a".repeat(64);
        apply_key(&conn, &key).unwrap();
        // Should be able to create tables after key
        conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY)").unwrap();
    }

    #[test]
    fn encrypted_db_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let key = generate_random_key().unwrap();

        // Write
        {
            let conn = Connection::open(&db_path).unwrap();
            let pragma_value = format!("x'{key}'");
            conn.pragma_update(None, "key", &pragma_value).unwrap();
            conn.execute_batch("CREATE TABLE test (val TEXT)").unwrap();
            conn.execute("INSERT INTO test VALUES (?1)", ["hello"]).unwrap();
        }

        // Read with correct key
        {
            let conn = Connection::open(&db_path).unwrap();
            apply_key(&conn, &key).unwrap();
            let val: String = conn
                .query_row("SELECT val FROM test", [], |r| r.get(0))
                .unwrap();
            assert_eq!(val, "hello");
        }

        // Wrong key fails
        {
            let conn = Connection::open(&db_path).unwrap();
            let wrong_key = "b".repeat(64);
            let result = apply_key(&conn, &wrong_key);
            assert!(result.is_err());
        }
    }
}
