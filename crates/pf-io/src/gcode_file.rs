use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::IoError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Fail with `IoError::AlreadyExists` if the file is already there.
    CreateNew,
    Overwrite,
}

/// Writes `gcode` to `path` atomically: readers see either the old file or
/// the complete new one, never a partial write.
pub fn export_gcode(path: &Path, gcode: &str, mode: WriteMode) -> Result<(), IoError> {
    let mut temp = TempFile::create_beside(path)?;
    temp.write_all(gcode.as_bytes())?;
    match mode {
        WriteMode::Overwrite => temp.rename_to(path),
        WriteMode::CreateNew => temp.persist_new(path),
    }
}

/// A temporary file in the destination's directory, so the final rename
/// stays on one filesystem. Removed on drop unless it was renamed.
struct TempFile {
    path: PathBuf,
    file: Option<fs::File>,
    renamed: bool,
}

impl TempFile {
    fn create_beside(destination: &Path) -> Result<TempFile, IoError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = destination.file_name().ok_or_else(|| IoError::Io {
            path: destination.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file path"),
        })?;
        let dir = match destination.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let mut attempts = 0;
        loop {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut temp_name = std::ffi::OsString::from(".");
            temp_name.push(name);
            temp_name.push(format!(".{}.{n}.tmp", std::process::id()));
            let path = dir.join(temp_name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(TempFile {
                        path,
                        file: Some(file),
                        renamed: false,
                    });
                }
                // A stale file from an earlier crashed run; try the next name.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 100 => {
                    attempts += 1;
                }
                Err(source) => return Err(IoError::Io { path, source }),
            }
        }
    }

    /// Writes, flushes to disk, and closes the file.
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), IoError> {
        let result = match self.file.take() {
            Some(mut file) => file.write_all(bytes).and_then(|()| file.sync_all()),
            None => Ok(()),
        };
        result.map_err(|source| IoError::Io {
            path: self.path.clone(),
            source,
        })
    }

    fn rename_to(mut self, destination: &Path) -> Result<(), IoError> {
        fs::rename(&self.path, destination).map_err(|source| IoError::Io {
            path: destination.to_owned(),
            source,
        })?;
        self.renamed = true;
        Ok(())
    }

    fn persist_new(self, destination: &Path) -> Result<(), IoError> {
        // A hard link fails atomically if the destination exists.
        match fs::hard_link(&self.path, destination) {
            Ok(()) => Ok(()), // drop removes the temporary name
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(IoError::AlreadyExists(destination.to_owned()))
            }
            Err(_) => {
                // FAT and exFAT (common on SD cards) have no hard links.
                // Check-then-rename can race with another writer.
                if fs::symlink_metadata(destination).is_ok() {
                    return Err(IoError::AlreadyExists(destination.to_owned()));
                }
                self.rename_to(destination)
            }
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        self.file = None;
        if !self.renamed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pf-io-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_a_new_file() {
        let dir = scratch_dir("new");
        let path = dir.join("job.gcode");
        export_gcode(&path, "G21\nG90\n", WriteMode::CreateNew).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "G21\nG90\n");
        assert_eq!(entries(&dir), vec!["job.gcode"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn create_new_never_overwrites() {
        let dir = scratch_dir("keep");
        let path = dir.join("job.gcode");
        fs::write(&path, "old").unwrap();
        let result = export_gcode(&path, "new", WriteMode::CreateNew);
        assert!(matches!(result, Err(IoError::AlreadyExists(p)) if p == path));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(entries(&dir), vec!["job.gcode"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn overwrite_replaces_the_content() {
        let dir = scratch_dir("replace");
        let path = dir.join("job.gcode");
        fs::write(&path, "old content that is longer").unwrap();
        export_gcode(&path, "new", WriteMode::Overwrite).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(entries(&dir), vec!["job.gcode"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_directory_reports_the_path() {
        let dir = scratch_dir("missing");
        let path = dir.join("no-such-dir").join("job.gcode");
        match export_gcode(&path, "G21\n", WriteMode::Overwrite) {
            Err(IoError::Io { path: reported, .. }) => {
                assert!(reported.starts_with(dir.join("no-such-dir")));
            }
            other => panic!("unexpected {other:?}"),
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_a_path_without_a_file_name() {
        let result = export_gcode(Path::new(".."), "G21\n", WriteMode::Overwrite);
        assert!(matches!(result, Err(IoError::Io { .. })));
    }
}
