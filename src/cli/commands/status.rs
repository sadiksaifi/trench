use std::path::Path;

use anyhow::Result;

use crate::state::Database;

pub fn execute(_cwd: &Path, _db: &Database, _branch: Option<&str>) -> Result<String> {
    todo!()
}

pub fn execute_json(_cwd: &Path, _db: &Database, _branch: Option<&str>) -> Result<String> {
    todo!()
}

pub fn execute_porcelain(_cwd: &Path, _db: &Database, _branch: Option<&str>) -> Result<String> {
    todo!()
}
