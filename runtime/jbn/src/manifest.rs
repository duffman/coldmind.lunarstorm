//! The manifest: what a package holds and how its modules link, encoded as
//! canonical CBOR (see [`crate::wire`]). Its SHA-256 is the package identity and
//! what the signature covers.
//!
//! | key | field       | value                                                  |
//! |-----|-------------|--------------------------------------------------------|
//! | 0   | format      | `1`                                                    |
//! | 1   | name        | package name                                           |
//! | 2   | version     | package version                                        |
//! | 3   | release     | release sequence number                                |
//! | 4   | engine      | identity of the engine the bytecode is for             |
//! | 5   | entry       | name of the entry module                               |
//! | 6   | modules     | `[{0 name, 1 blob, 2 [[specifier, target]...]}...]`    |
//! | 7   | resources   | `[{0 name, 1 blob}...]`                                |
//! | 8   | metadata    | `[[key, value]...]`                                    |
//! | 9   | blobs       | `[{0 offset, 1 stored, 2 size, 3 sha256, 4 codec}...]` |

use crate::wire::{self, Value};
use crate::{Error, Limits};
use std::collections::HashSet;

/// The manifest format this crate writes and reads.
pub const FORMAT: u64 = 1;

/// How a blob is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// As it is.
    None,
    /// zstd frame.
    Zstd,
}

/// One payload: a byte range of the payload section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    /// Offset in the payload section, a multiple of 8.
    pub offset: u64,
    /// Stored size.
    pub stored: u64,
    /// Size once decoded.
    pub size: u64,
    /// SHA-256 of the decoded bytes.
    pub digest: [u8; 32],
    /// How it is stored.
    pub compression: Compression,
}

/// One compiled module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Canonical name, `/app/<path>`; the engine names stack frames after it.
    pub name: String,
    /// Index of its bytecode in [`Manifest::blobs`].
    pub blob: u64,
    /// Each import specifier the module uses, with the name it resolved to at
    /// build time: another module of the package, or a builtin or host module.
    pub imports: Vec<(String, String)>,
}

/// One resource: bytes the application reads at run time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// Its name.
    pub name: String,
    /// Index of its bytes in [`Manifest::blobs`].
    pub blob: u64,
}

/// What a package holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Release sequence number; a trust store may require a minimum.
    pub release: u64,
    /// The engine the bytecode was written by (`adapter engine version contract`);
    /// a Runtime on any other engine refuses the package.
    pub engine: String,
    /// Name of the entry module.
    pub entry: String,
    /// Modules, sorted by name.
    pub modules: Vec<Module>,
    /// Resources, sorted by name.
    pub resources: Vec<Resource>,
    /// Free-form metadata, sorted by key.
    pub metadata: Vec<(String, String)>,
    /// Payloads, in offset order.
    pub blobs: Vec<Blob>,
}

impl Manifest {
    /// # Returns
    /// The canonical encoding.
    pub fn encode(&self) -> Vec<u8> {
        let text = |s: &str| Value::Text(s.to_owned());
        let pair = |(a, b): &(String, String)| Value::Array(vec![text(a), text(b)]);
        wire::encode(&Value::Map(vec![
            Value::U(FORMAT),
            text(&self.name),
            text(&self.version),
            Value::U(self.release),
            text(&self.engine),
            text(&self.entry),
            Value::Array(
                self.modules
                    .iter()
                    .map(|m| {
                        Value::Map(vec![text(&m.name), Value::U(m.blob), Value::Array(m.imports.iter().map(pair).collect())])
                    })
                    .collect(),
            ),
            Value::Array(self.resources.iter().map(|r| Value::Map(vec![text(&r.name), Value::U(r.blob)])).collect()),
            Value::Array(self.metadata.iter().map(pair).collect()),
            Value::Array(
                self.blobs
                    .iter()
                    .map(|b| {
                        Value::Map(vec![
                            Value::U(b.offset),
                            Value::U(b.stored),
                            Value::U(b.size),
                            Value::Bytes(b.digest.to_vec()),
                            Value::U(match b.compression {
                                Compression::None => 0,
                                Compression::Zstd => 1,
                            }),
                        ])
                    })
                    .collect(),
            ),
        ]))
    }

    /// Decodes canonical bytes. Call [`Manifest::validate`] before trusting it.
    ///
    /// # Errors
    /// Not a canonical manifest of [`FORMAT`].
    pub fn decode(bytes: &[u8]) -> Result<Manifest, Error> {
        let root = wire::decode(bytes)?;
        let f = root.map(10)?;
        if f[0].uint()? != FORMAT {
            return Err(Error::Format(format!("manifest format {} is not {FORMAT}", f[0].uint()?)));
        }
        let pair = |v: &Value| -> Result<(String, String), Error> {
            match v.array()? {
                [a, b] => Ok((a.text()?.to_owned(), b.text()?.to_owned())),
                _ => Err(Error::Format(String::from("expected a pair"))),
            }
        };
        let modules = f[6]
            .array()?
            .iter()
            .map(|m| {
                let m = m.map(3)?;
                Ok(Module {
                    name: m[0].text()?.to_owned(),
                    blob: m[1].uint()?,
                    imports: m[2].array()?.iter().map(pair).collect::<Result<_, Error>>()?,
                })
            })
            .collect::<Result<_, Error>>()?;
        let resources = f[7]
            .array()?
            .iter()
            .map(|r| {
                let r = r.map(2)?;
                Ok(Resource { name: r[0].text()?.to_owned(), blob: r[1].uint()? })
            })
            .collect::<Result<_, Error>>()?;
        let blobs = f[9]
            .array()?
            .iter()
            .map(|b| {
                let b = b.map(5)?;
                Ok(Blob {
                    offset: b[0].uint()?,
                    stored: b[1].uint()?,
                    size: b[2].uint()?,
                    digest: b[3].bytes()?.try_into().map_err(|_| Error::Format(String::from("digest is not 32 bytes")))?,
                    compression: match b[4].uint()? {
                        0 => Compression::None,
                        1 => Compression::Zstd,
                        other => return Err(Error::Format(format!("unknown compression {other}"))),
                    },
                })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Manifest {
            name: f[1].text()?.to_owned(),
            version: f[2].text()?.to_owned(),
            release: f[3].uint()?,
            engine: f[4].text()?.to_owned(),
            entry: f[5].text()?.to_owned(),
            modules,
            resources,
            metadata: f[8].array()?.iter().map(pair).collect::<Result<_, Error>>()?,
            blobs,
        })
    }

    /// Checks the manifest's own consistency.
    ///
    /// Decision: every blob belongs to exactly one module or resource, so a
    /// package carries nothing its manifest does not account for.
    ///
    /// # Errors
    /// Unsorted or duplicate names, a missing entry, a blob used twice or never,
    /// or more than `limits` allow.
    pub fn validate(&self, limits: &Limits) -> Result<(), Error> {
        let bad = |message: &str| Err(Error::Format(message.to_owned()));
        if self.name.is_empty() || self.version.is_empty() || self.engine.is_empty() {
            return bad("name, version and engine are required");
        }
        if self.modules.len() > limits.modules || self.resources.len() > limits.resources {
            return Err(Error::Limit);
        }
        if !self.modules.windows(2).all(|w| w[0].name < w[1].name) {
            return bad("modules must be sorted and unique");
        }
        if !self.resources.windows(2).all(|w| w[0].name < w[1].name) {
            return bad("resources must be sorted and unique");
        }
        if !self.metadata.windows(2).all(|w| w[0].0 < w[1].0) {
            return bad("metadata must be sorted and unique");
        }
        if !self.modules.iter().any(|m| m.name == self.entry) {
            return bad("the entry is not one of the modules");
        }
        if self.modules.iter().map(|m| m.imports.len()).sum::<usize>() > limits.edges {
            return Err(Error::Limit);
        }
        let mut used = HashSet::new();
        for index in self.modules.iter().map(|m| m.blob).chain(self.resources.iter().map(|r| r.blob)) {
            if index >= self.blobs.len() as u64 || !used.insert(index) {
                return bad("each blob must belong to exactly one module or resource");
            }
        }
        if used.len() != self.blobs.len() {
            return bad("a blob belongs to no module or resource");
        }
        for blob in &self.blobs {
            if blob.size > limits.blob_bytes as u64 || blob.stored > limits.blob_bytes as u64 {
                return Err(Error::Limit);
            }
            if blob.compression == Compression::None && blob.size != blob.stored {
                return bad("an uncompressed blob's sizes differ");
            }
        }
        Ok(())
    }

    /// # Returns
    /// The module named `name`.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.binary_search_by(|m| m.name.as_str().cmp(name)).ok().map(|i| &self.modules[i])
    }

    /// # Returns
    /// The resource named `name`.
    pub fn resource(&self, name: &str) -> Option<&Resource> {
        self.resources.binary_search_by(|r| r.name.as_str().cmp(name)).ok().map(|i| &self.resources[i])
    }
}
