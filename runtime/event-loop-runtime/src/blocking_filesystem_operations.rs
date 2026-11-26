//! Blocking filesystem operations.
//!
//! One function, [`exec`], implements every operation. The JS glue selects an
//! operation by code; the same function runs inline on the reactor for the `*Sync`
//! API and on a worker thread for the async API, so both paths share one
//! implementation and one error format.

use crate::reactor_io_state::Fault;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Operation codes, used as they are by `node_builtins/fs_module.rs`.
pub mod op {
    /// Read a file as bytes.
    pub const READ: i32 = 0;
    /// Read a file as UTF-8 text (invalid sequences become U+FFFD).
    pub const READ_UTF8: i32 = 1;
    /// Create or truncate a file and write the payload.
    pub const WRITE: i32 = 2;
    /// Append the payload, creating the file if needed.
    pub const APPEND: i32 = 3;
    /// Stat a path, following symlinks.
    pub const STAT: i32 = 4;
    /// List the names in a directory.
    pub const READDIR: i32 = 5;
    /// Create one directory.
    pub const MKDIR: i32 = 6;
    /// Create a directory and any missing parents.
    pub const MKDIR_P: i32 = 7;
    /// Remove a file.
    pub const UNLINK: i32 = 8;
    /// Remove an empty directory.
    pub const RMDIR: i32 = 9;
    /// Rename; the payload holds the destination path.
    pub const RENAME: i32 = 10;
    /// Test whether a path exists.
    pub const EXISTS: i32 = 11;
    /// Remove a file or a directory tree; a missing path is not an error.
    pub const RM_RF: i32 = 12;
}

/// Successful result of a filesystem operation, converted to a JS value on the reactor.
pub enum FsValue {
    /// No value (`undefined`).
    Unit,
    /// A boolean.
    Bool(bool),
    /// Raw file contents; handed to JS without copying.
    Bytes(Vec<u8>),
    /// Decoded file contents. Decoding happens on the calling thread, so for async
    /// reads the UTF-8 validation runs off the reactor.
    Text(String),
    /// Metadata of a path.
    Stat {
        /// Size in bytes.
        size: u64,
        /// Modification time in milliseconds since the Unix epoch.
        mtime_ms: f64,
        /// 1 for a regular file, 2 for a directory, 0 for anything else.
        kind: u8,
    },
    /// Directory entry names.
    Names(Vec<String>),
}

/// Runs one filesystem operation to completion.
///
/// # Arguments
/// * `op` - operation code, one of [`op`].
/// * `path` - target path.
/// * `data` - payload for writes, destination path for [`op::RENAME`], unused otherwise.
///
/// # Returns
/// The operation's value.
///
/// # Errors
/// A [`Fault`] carrying a Node-style code (`ENOENT`, `EACCES`, ...) and message.
pub fn exec(op: i32, path: &str, data: &[u8]) -> Result<FsValue, Fault> {
    let fault = |call: &'static str| move |e: io::Error| Fault::new(&e, call, path);
    let unit = |_| FsValue::Unit;
    match op {
        op::READ => fs::read(path).map(FsValue::Bytes).map_err(fault("open")),
        op::READ_UTF8 => fs::read(path)
            .map(|bytes| {
                FsValue::Text(String::from_utf8(bytes).unwrap_or_else(|e| {
                    String::from_utf8_lossy(e.as_bytes()).into_owned()
                }))
            })
            .map_err(fault("open")),
        op::WRITE => fs::write(path, data).map(unit).map_err(fault("open")),
        op::APPEND => fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut file| file.write_all(data))
            .map(unit)
            .map_err(fault("open")),
        op::STAT => fs::metadata(path)
            .map(|meta| FsValue::Stat {
                size: meta.len(),
                mtime_ms: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0.0, |d| d.as_secs_f64() * 1000.0),
                kind: if meta.is_file() {
                    1
                } else if meta.is_dir() {
                    2
                } else {
                    0
                },
            })
            .map_err(fault("stat")),
        op::READDIR => fs::read_dir(path)
            .and_then(|entries| {
                entries
                    .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect::<io::Result<Vec<_>>>()
            })
            .map(FsValue::Names)
            .map_err(fault("scandir")),
        op::MKDIR => fs::create_dir(path).map(unit).map_err(fault("mkdir")),
        op::MKDIR_P => fs::create_dir_all(path).map(unit).map_err(fault("mkdir")),
        op::UNLINK => fs::remove_file(path).map(unit).map_err(fault("unlink")),
        op::RMDIR => fs::remove_dir(path).map(unit).map_err(fault("rmdir")),
        op::RENAME => fs::rename(path, String::from_utf8_lossy(data).as_ref())
            .map(unit)
            .map_err(fault("rename")),
        op::EXISTS => Ok(FsValue::Bool(Path::new(path).exists())),
        op::RM_RF => match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(path).map(unit).map_err(fault("rm")),
            Ok(_) => fs::remove_file(path).map(unit).map_err(fault("rm")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(FsValue::Unit),
            Err(e) => Err(fault("rm")(e)),
        },
        _ => Err(Fault {
            code: "EINVAL",
            message: format!("EINVAL: unknown fs operation {op}"),
        }),
    }
}
