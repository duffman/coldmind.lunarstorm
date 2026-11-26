//! Owner-local, bounded source admission and lossless source-map queries.

use super::source_map::{Position, SourceMaps, validate_position};
use super::{
    DebugError, DebugIdentities, DebugLimits, DebugLocation, DebugReply, NativeScriptKey,
    NativeScriptMetadata, Page, ScriptId, ScriptInfo, ScriptRegistration, ScriptSelector,
};
use crate::engine::{
    ContextId, JsString, OriginalSource, ScriptSource, SourceDigest, SourceMapAttachment,
};
use std::{
    collections::HashSet,
    mem::size_of,
    thread::{self, ThreadId},
};

/// Original-source position obtained from validated mappings, without inventing
/// a generated-script identity for original text that was never registered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OriginalLocation {
    /// Resolved original URI; no filesystem normalization or network fetch occurs.
    pub origin: String,
    /// Zero-based original line.
    pub line: u32,
    /// Zero-based original UTF-16 code-unit column.
    pub column: u32,
    /// Content identity only when actual original text was supplied.
    pub digest: Option<SourceDigest>,
}
struct Record {
    registration: ScriptRegistration,
    source: ScriptSource,
    maps: Option<SourceMaps>,
}

/// Context-bound retained source registry admitting scripts before application entry.
///
/// Registration is independent of an attached debugger client. Existing revisions
/// are never silently evicted. The caller uses the returned registration in its
/// native script table; the registry also checks repeated native keys for conflicts.
/// Public queries return owned data and never invoke native code or JavaScript.
/// Source/attachment buffer capacity and decoded mapping storage are charged to
/// `DebugLimits::source_bytes`; native bindings are charged as bounded metadata.
/// Ordinary host source origins retain their exact identity. Relative map sources
/// use the generated source URI as their base because attachments carry no map URI.
/// Preceding maps must include `file`, ordered from nearest transformation backward.
/// Relative `file` names resolve against the final generated source URI. A root
/// map's explicit `file` must resolve to its attached generated URI; ambiguous
/// or mismatched file conventions return `InvalidMap` rather than being ignored.
pub struct SourceRegistry {
    context: ContextId,
    owner: ThreadId,
    issuer: Option<u64>,
    limits: DebugLimits,
    records: Vec<Record>,
    bindings: Vec<(NativeScriptKey, ScriptRegistration)>,
    bytes: usize,
    closed: bool,
}
fn plus(a: usize, b: usize) -> Result<usize, DebugError> {
    a.checked_add(b).ok_or(DebugError::Capacity)
}
fn allocation_bytes(count: usize, item_bytes: usize) -> Result<usize, DebugError> {
    count.checked_mul(item_bytes).ok_or(DebugError::Capacity)
}
fn attachment_bytes(attachment: &SourceMapAttachment) -> Result<usize, DebugError> {
    let mut bytes = attachment.json.capacity();
    let originals = allocation_bytes(attachment.originals.capacity(), size_of::<OriginalSource>())?;
    bytes = plus(bytes, originals)?;
    for original in &attachment.originals {
        bytes = plus(bytes, original.origin.capacity())?;
        let text = original.text.as_ref().map_or(0, JsString::capacity_bytes);
        bytes = plus(bytes, text)?;
    }
    let preceding = allocation_bytes(attachment.preceding_maps.capacity(), size_of::<Vec<u8>>())?;
    bytes = plus(bytes, preceding)?;
    for map in &attachment.preceding_maps {
        bytes = plus(bytes, map.capacity())?;
    }
    Ok(bytes)
}
fn source_bytes(source: &ScriptSource) -> Result<usize, DebugError> {
    plus(
        plus(source.text.capacity_bytes(), source.origin.capacity())?,
        source
            .source_map
            .as_ref()
            .map(attachment_bytes)
            .transpose()?
            .unwrap_or(0),
    )
}
fn same_json(a: &[u8], b: &[u8]) -> bool {
    a == b || super::source_map::equivalent_json(a, b)
}
fn same_attachment(a: &SourceMapAttachment, b: &SourceMapAttachment) -> bool {
    same_json(&a.json, &b.json)
        && a.preceding_maps.len() == b.preceding_maps.len()
        && a.preceding_maps
            .iter()
            .zip(&b.preceding_maps)
            .all(|(a, b)| same_json(a, b))
        && a.originals.len() == b.originals.len()
        && a.originals
            .iter()
            .zip(&b.originals)
            .all(|(a, b)| a.origin == b.origin && a.text == b.text)
}
/// Validates source/map input against static bounds before command admission.
///
/// This pure host check uses the same lossless map parser and capacity accounting
/// as registration, without issuing IDs, retaining records, fetching text, or
/// entering native code. Live registry aggregate capacity is checked again at
/// its mandatory synchronous registration gate before application execution.
pub fn validate_source(source: &ScriptSource, limits: &DebugLimits) -> Result<(), DebugError> {
    let raw = source_bytes(source)?;
    if raw > limits.source_bytes {
        return Err(DebugError::Capacity);
    }
    let decoded = source
        .source_map
        .as_ref()
        .map(|attachment| SourceMaps::parse(&source.origin, &source.text, attachment))
        .transpose()?;
    let decoded_bytes = decoded
        .as_ref()
        .map(SourceMaps::footprint)
        .transpose()?
        .unwrap_or(0);
    let total = plus(raw, decoded_bytes)?;
    if total > limits.source_bytes {
        Err(DebugError::Capacity)
    } else {
        Ok(())
    }
}
impl SourceRegistry {
    /// Creates an empty registry bound to one owner-local context.
    ///
    /// Returns `Capacity` for zero source/script/reply budgets. It does not create
    /// a native context, attach a session, or reserve an application queue slot.
    pub fn new(context: ContextId, limits: DebugLimits) -> Result<Self, DebugError> {
        if limits.scripts == 0 || limits.source_bytes == 0 || limits.reply_bytes == 0 {
            return Err(DebugError::Capacity);
        }
        Ok(Self {
            context,
            owner: thread::current().id(),
            issuer: None,
            limits,
            records: Vec::new(),
            bindings: Vec::new(),
            bytes: 0,
            closed: false,
        })
    }
    fn check(&self) -> Result<(), DebugError> {
        if self.owner != thread::current().id() {
            return Err(DebugError::InvalidSource);
        }
        if self.closed {
            return Err(DebugError::Closed);
        }
        Ok(())
    }
    /// Registers native source synchronously, returning its canonical immutable identity.
    ///
    /// URI/digest duplicates reuse the revision and source storage, even with no
    /// session attached. A duplicate without a map reuses its staged mapping;
    /// conflicting supplied attachments return `InvalidMap`. New content at a URI
    /// creates a new monotonic revision while retaining old content. Context/key,
    /// source/map validity, and checked capacity are validated before mutation.
    /// The supplied issuer must belong to this context; issued IDs cannot be forged.
    pub fn register(
        &mut self,
        identities: &DebugIdentities,
        metadata: NativeScriptMetadata,
    ) -> Result<ScriptRegistration, DebugError> {
        self.check()?;
        if identities.state() == super::DebugExecutionState::Closed {
            return Err(DebugError::Closed);
        }
        if identities.context() != self.context
            || self
                .issuer
                .is_some_and(|issuer| issuer != identities.issuer_epoch())
        {
            return Err(DebugError::InvalidSource);
        }
        if metadata.key.context() != self.context {
            return Err(DebugError::InvalidSource);
        }
        let source = metadata.source;
        let digest = SourceDigest::of_utf16(&source.text);
        let existing = self.records.iter().position(|record| {
            record.source.origin == source.origin && record.registration.digest == digest
        });
        if let Some((_, prior)) = self.bindings.iter().find(|(key, _)| *key == metadata.key)
            && existing.is_none_or(|index| self.records[index].registration != *prior)
        {
            return Err(DebugError::InvalidSource);
        }
        let binding_exists = self.bindings.iter().any(|(key, _)| *key == metadata.key);
        // Stage the alias table separately so a failed budget/allocation check
        // cannot grow retained state. Charge actual vector capacity, not length.
        let mut staged_bindings = if binding_exists {
            None
        } else {
            let mut next = Vec::new();
            next.try_reserve_exact(
                self.bindings
                    .len()
                    .checked_add(1)
                    .ok_or(DebugError::Capacity)?,
            )
            .map_err(|_| DebugError::Capacity)?;
            next.extend_from_slice(&self.bindings);
            Some(next)
        };
        let base_bytes = if let Some(next) = &staged_bindings {
            let old = self
                .bindings
                .capacity()
                .checked_mul(size_of::<(NativeScriptKey, ScriptRegistration)>())
                .ok_or(DebugError::Capacity)?;
            let new = next
                .capacity()
                .checked_mul(size_of::<(NativeScriptKey, ScriptRegistration)>())
                .ok_or(DebugError::Capacity)?;
            plus(
                self.bytes.checked_sub(old).ok_or(DebugError::Capacity)?,
                new,
            )?
        } else {
            self.bytes
        };
        if let Some(index) = existing {
            let record = &self.records[index];
            if let Some(supplied) = &source.source_map
                && !record
                    .source
                    .source_map
                    .as_ref()
                    .is_some_and(|old| same_attachment(old, supplied))
            {
                return Err(DebugError::InvalidMap);
            }
            let total = base_bytes;
            if total > self.limits.source_bytes {
                return Err(DebugError::Capacity);
            }
            let registration = record.registration;
            if let Some(mut next) = staged_bindings.take() {
                next.push((metadata.key, registration));
                self.bindings = next;
            }
            self.bytes = total;
            return Ok(registration);
        }
        if self.records.len() == self.limits.scripts {
            return Err(DebugError::Capacity);
        }
        // Check raw capacity first; decoded map storage is checked again before commit.
        let raw_bytes = source_bytes(&source)?;
        if plus(base_bytes, raw_bytes)? > self.limits.source_bytes {
            return Err(DebugError::Capacity);
        }
        let maps = source
            .source_map
            .as_ref()
            .map(|attachment| SourceMaps::parse(&source.origin, &source.text, attachment))
            .transpose()?;
        let decoded_bytes = maps
            .as_ref()
            .map(SourceMaps::footprint)
            .transpose()?
            .unwrap_or(0);
        let record_bytes = plus(raw_bytes, decoded_bytes)?;
        let total = plus(base_bytes, record_bytes)?;
        if total > self.limits.source_bytes {
            return Err(DebugError::Capacity);
        }
        let revision = self
            .records
            .iter()
            .filter(|record| record.source.origin == source.origin)
            .map(|record| record.registration.revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        self.records
            .try_reserve_exact(1)
            .map_err(|_| DebugError::Capacity)?;
        let id = identities.next_script()?;
        self.check_script_context(id)?;
        let registration = ScriptRegistration {
            id,
            revision,
            digest,
        };
        self.issuer = Some(identities.issuer_epoch());
        self.records.push(Record {
            registration,
            source,
            maps,
        });
        let mut next = staged_bindings.take().ok_or(DebugError::InvalidSource)?;
        next.push((metadata.key, registration));
        self.bindings = next;
        self.bytes = total;
        Ok(registration)
    }
    /// Resolves an exact native script key to its already admitted source identity.
    ///
    /// Foreign or unknown keys return `InvalidSource`; this does not register code.
    pub fn native_registration(
        &self,
        key: NativeScriptKey,
    ) -> Result<ScriptRegistration, DebugError> {
        self.check()?;
        self.bindings
            .iter()
            .find(|(old, _)| *old == key)
            .map(|(_, registration)| *registration)
            .ok_or(DebugError::InvalidSource)
    }
    /// Selects the newest retained revision for a URI, or its exact expected digest.
    ///
    /// `None` means the source has not loaded; breakpoint configuration may remain
    /// pending. Distinct host-supplied URI strings are not normalized or merged.
    pub fn select(
        &self,
        selector: &ScriptSelector,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        self.check()?;
        Ok(self
            .records
            .iter()
            .rev()
            .find(|record| {
                record.source.origin == selector.origin
                    && selector
                        .expected_digest
                        .is_none_or(|digest| digest == record.registration.digest)
            })
            .map(|record| record.registration))
    }
    /// Checks that this retained generated identity has not been superseded.
    pub fn is_current_script(&self, location: &DebugLocation) -> Result<bool, DebugError> {
        self.check()?;
        let record = self.record(location.script, location.revision)?;
        Ok(!self.records.iter().any(|newer| {
            newer.source.origin == record.source.origin
                && newer.registration.revision > record.registration.revision
        }))
    }
    fn record(&self, id: ScriptId, revision: u64) -> Result<&Record, DebugError> {
        self.check_script_context(id)?;
        self.records
            .iter()
            .find(|record| record.registration.id == id && record.registration.revision == revision)
            .ok_or(DebugError::InvalidSource)
    }
    fn check_script_context(&self, id: ScriptId) -> Result<(), DebugError> {
        if id.engine_epoch != self.context.engine_epoch()
            || id.context_generation != self.context.generation()
        {
            return Err(DebugError::InvalidSource);
        }
        Ok(())
    }
    /// Reads a bounded source metadata page with the total retained revision count.
    ///
    /// Returns `InvalidRequest` for out-of-range/overflowing pagination and
    /// `Capacity` when the resulting owned page would exceed the common reply budget.
    pub fn sources(&self, page: Page) -> Result<DebugReply, DebugError> {
        self.check()?;
        let end = self.page_end(page, self.records.len())?;
        let selected = &self.records[page.start..end];
        let mut bytes = size_of::<DebugReply>();
        for record in selected {
            bytes = plus(bytes, size_of::<ScriptInfo>())?;
            bytes = plus(bytes, record.source.origin.len())?;
        }
        if bytes > self.limits.reply_bytes {
            return Err(DebugError::Capacity);
        }
        let scripts = selected
            .iter()
            .map(|record| ScriptInfo {
                registration: record.registration,
                origin: record.source.origin.clone(),
                length_units: record.source.text.as_units().len(),
            })
            .collect();
        Ok(DebugReply::Sources {
            scripts,
            total: self.records.len(),
        })
    }
    fn page_end(&self, page: Page, total: usize) -> Result<usize, DebugError> {
        if page.start > total {
            return Err(DebugError::InvalidRequest);
        }
        Ok(page
            .start
            .checked_add(page.count)
            .ok_or(DebugError::InvalidRequest)?
            .min(total))
    }
    /// Reads a lossless bounded UTF-16 page from one exact source revision.
    ///
    /// Pages may split surrogate pairs because offsets/counts are code units.
    /// Returns `InvalidSource` for unknown identity/revision, `InvalidRequest` for
    /// invalid pagination, or `Capacity` before allocating an oversized reply.
    /// Large complete protocol responses must gather multiple successful pages.
    pub fn source(
        &self,
        script: ScriptId,
        revision: u64,
        page: Page,
    ) -> Result<DebugReply, DebugError> {
        self.check()?;
        let record = self.record(script, revision)?;
        let total_units = record.source.text.as_units().len();
        let end = self.page_end(page, total_units)?;
        let text_bytes = allocation_bytes(end - page.start, 2)?;
        let bytes = plus(size_of::<DebugReply>(), text_bytes)?;
        if bytes > self.limits.reply_bytes {
            return Err(DebugError::Capacity);
        }
        Ok(DebugReply::Source {
            text: JsString::from_units(record.source.text.as_units()[page.start..end].to_vec()),
            total_units,
        })
    }
    /// Returns generated candidates for a generated or original source breakpoint.
    ///
    /// With a column, choose mapping points at the nearest original column at or
    /// after it on that same line; without a column, include that line’s mappings.
    /// Only the newest revision of each URI participates unless a digest is given.
    /// No candidates means unloaded/unmappable; the coordinator distinguishes pending
    /// from rejection and verifies candidates against real native executable hooks.
    /// This method never claims that a TypeScript line is itself executable.
    pub fn resolve_breakpoint(
        &self,
        selector: &ScriptSelector,
        line: u32,
        column: Option<u32>,
    ) -> Result<Vec<DebugLocation>, DebugError> {
        self.check()?;
        if let Some(registration) = self.select(selector)? {
            let record = self.record(registration.id, registration.revision)?;
            validate_position(
                &record.source.text,
                Position {
                    line,
                    column: column.unwrap_or(0),
                },
            )
            .map_err(|_| DebugError::InvalidSource)?;
            return Ok(vec![DebugLocation {
                script: registration.id,
                revision: registration.revision,
                line,
                column,
            }]);
        }
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let mut nearest = None;
        for record in &self.records {
            if selector.expected_digest.is_none()
                && self.records.iter().any(|newer| {
                    newer.source.origin == record.source.origin
                        && newer.registration.revision > record.registration.revision
                })
            {
                continue;
            }
            let Some(maps) = &record.maps else {
                continue;
            };
            let mut previous = None;
            for segment in &maps.root.segments {
                if previous == Some(segment.generated) {
                    continue;
                }
                previous = Some(segment.generated);
                for mapped in maps.mapped_all(segment.generated)? {
                    if mapped.origin != selector.origin
                        || mapped.line != line
                        || selector
                            .expected_digest
                            .is_some_and(|digest| mapped.digest != Some(digest))
                        || column.is_some_and(|column| mapped.column < column)
                    {
                        continue;
                    }
                    if column.is_some() {
                        if nearest.is_some_and(|best| mapped.column > best) {
                            continue;
                        }
                        if nearest.is_none_or(|best| mapped.column < best) {
                            candidates.clear();
                            seen.clear();
                            nearest = Some(mapped.column);
                        }
                    }
                    let candidate = DebugLocation {
                        script: record.registration.id,
                        revision: record.registration.revision,
                        line: segment.generated.line,
                        column: Some(segment.generated.column),
                    };
                    if seen.insert((
                        candidate.script,
                        candidate.revision,
                        candidate.line,
                        candidate.column,
                    )) {
                        candidates.push(candidate);
                    }
                }
            }
        }
        Ok(candidates)
    }
    /// Maps a validated generated position to its first original in encoded order.
    ///
    /// This convenience chooses the first result of [`Self::map_locations`]. Use
    /// that method when multiple original sources share a generated point.
    pub fn map_location(
        &self,
        location: &DebugLocation,
    ) -> Result<Option<OriginalLocation>, DebugError> {
        Ok(self.map_locations(location)?.into_iter().next())
    }
    /// Maps a generated position to every supplied original through the map chain.
    ///
    /// Returns an empty vector for unmapped spans or an unavailable native column.
    /// Exact revision and available source bounds are checked before traversal.
    /// Missing original content produces no invented digest or script identity.
    pub fn map_locations(
        &self,
        location: &DebugLocation,
    ) -> Result<Vec<OriginalLocation>, DebugError> {
        self.check()?;
        let record = self.record(location.script, location.revision)?;
        let Some(column) = location.column else {
            return Ok(Vec::new());
        };
        let position = Position {
            line: location.line,
            column,
        };
        validate_position(&record.source.text, position).map_err(|_| DebugError::InvalidSource)?;
        record
            .maps
            .as_ref()
            .map(|maps| maps.mapped_all(position))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    /// Matches the generated URI or the original URI displayed at this marker.
    pub fn skip_source(
        &self,
        location: &DebugLocation,
        policy: &super::StepPolicy,
    ) -> Result<bool, DebugError> {
        let record = self.record(location.script, location.revision)?;
        super::stepping::validate(policy, &self.limits)?;
        if super::stepping::matches(policy, &record.source.origin)? {
            return Ok(true);
        }
        if let Some(original) = self.map_location(location)? {
            return super::stepping::matches(policy, &original.origin);
        }
        Ok(false)
    }

    /// Projects an actual generated marker to a selected retained original.
    pub fn original_locations(
        &self,
        selector: &ScriptSelector,
        location: &DebugLocation,
    ) -> Result<Vec<OriginalLocation>, DebugError> {
        if selector.expected_digest.is_none() && !self.is_current_script(location)? {
            return Ok(Vec::new());
        }
        let mut mapped = self.map_locations(location)?;
        mapped.retain(|original| {
            original.origin == selector.origin
                && selector
                    .expected_digest
                    .is_none_or(|digest| original.digest == Some(digest))
        });
        Ok(mapped)
    }

    /// Reads supplied original content by URI and optional actual content digest.
    ///
    /// Returns `None` when original text is unavailable; it never fetches or
    /// fabricates content. Without a digest the newest retained source mapping
    /// containing that URI wins. Pages and errors follow [`Self::source`].
    pub fn original_source(
        &self,
        selector: &ScriptSelector,
        page: Page,
    ) -> Result<Option<DebugReply>, DebugError> {
        self.check()?;
        for record in self.records.iter().rev() {
            let Some(maps) = &record.maps else {
                continue;
            };
            for original in maps
                .root
                .sources
                .iter()
                .chain(maps.preceding.iter().flat_map(|(_, map)| &map.sources))
            {
                if original.origin.as_ref() != Some(&selector.origin) {
                    continue;
                }
                if selector
                    .expected_digest
                    .is_some_and(|expected| original.digest != Some(expected))
                {
                    continue;
                }
                let Some(text) = &original.text else {
                    return Ok(None);
                };
                let total_units = text.as_units().len();
                let end = self.page_end(page, total_units)?;
                let text_bytes = allocation_bytes(end - page.start, 2)?;
                let bytes = plus(size_of::<DebugReply>(), text_bytes)?;
                if bytes > self.limits.reply_bytes {
                    return Err(DebugError::Capacity);
                }
                return Ok(Some(DebugReply::Source {
                    text: JsString::from_units(text.as_units()[page.start..end].to_vec()),
                    total_units,
                }));
            }
        }
        Ok(None)
    }
    /// Returns currently retained source/map/native-binding bytes charged to the budget.
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    /// Permanently closes admission/queries and drops retained source data without JS.
    ///
    /// Existing external registrations become invalid. Calling close again is harmless.
    pub fn close(&mut self) {
        self.closed = true;
        self.records = Vec::new();
        self.bindings = Vec::new();
        self.bytes = 0;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/debug_sources.rs"]
mod tests;
