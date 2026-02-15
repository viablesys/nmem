use crate::db::open_db;
use crate::NmemError;
use std::path::Path;

pub fn handle_pin(db_path: &Path, id: i64) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;
    let updated = conn.execute(
        "UPDATE observations SET is_pinned = 1 WHERE id = ?1",
        [id],
    )?;
    if updated == 0 {
        return Err(NmemError::Config(format!("observation {id} not found")));
    }
    eprintln!("nmem: pinned observation {id}");
    Ok(())
}

pub fn handle_unpin(db_path: &Path, id: i64) -> Result<(), NmemError> {
    let conn = open_db(db_path)?;
    let updated = conn.execute(
        "UPDATE observations SET is_pinned = 0 WHERE id = ?1",
        [id],
    )?;
    if updated == 0 {
        return Err(NmemError::Config(format!("observation {id} not found")));
    }
    eprintln!("nmem: unpinned observation {id}");
    Ok(())
}
