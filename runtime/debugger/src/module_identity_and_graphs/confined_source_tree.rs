//! Snapshot view of a capability-confined source tree for module and package resolution.
//!
//! Adapted from Bun's resolver directory cache (`src/resolver/fs.rs` `DirEntry`/`Entry`
//! and `src/resolver/dir_info.rs`, MIT licensed): every directory is listed once, file
//! existence and exact spelling are answered from that listing, symlinks are stat'd and
//! resolved only when a path actually traverses one, and real paths are derived from the
//! parent's real path instead of being recomputed per file.
//!
//! Resolvers query [`VirtualDir`] instead of the disk. A provider sees one snapshot for its
//! lifetime (one build or run); a name missing from a cached listing re-reads that one
//! directory once, so files created after the listing are still found.
#[path = "confined_source_tree/cached_directory_listing.rs"]
mod entries;
#[path = "confined_source_tree/cached_path_resolution.rs"]
mod virtual_dir;

pub(crate) use entries::Kind;
pub(crate) use virtual_dir::VirtualDir;

#[cfg(test)]
#[path = "../../tests/unit/virtual_dir.rs"]
mod tests;
