//! Iterative, bounded copies between owned host data and protected native values.
use super::{NativeValue, Scope, ffi};
use crate::engine::{
    ExportLimits, FailureCode, FailureReport, JsData, JsResult, JsString, ScopedValue, ValueKind,
};
use std::{mem::size_of, ptr, rc::Rc};

fn capacity() -> FailureReport {
    FailureReport::fault(FailureCode::Capacity, "copied data limits exceeded")
}

fn unsupported(message: &str) -> FailureReport {
    FailureReport::fault(FailureCode::UnsupportedConversion, message)
}

/// Array indices and lengths use the native unsigned 32-bit representation.
fn array_length(length: i128) -> Result<usize, FailureReport> {
    u32::try_from(length)
        .map(|length| length as usize)
        .map_err(|_| capacity())
}

/// Validate the native byte-storage invariant before allocation or raw access.
fn byte_storage(length: usize, has_pointer: bool) -> Result<(), FailureReport> {
    if length != 0 && !has_pointer {
        return Err(unsupported("invalid native byte storage"));
    }
    Ok(())
}

/// Counts output storage, including actual vector capacities, before publication.
struct Budget {
    limits: ExportLimits,
    nodes: usize,
    bytes: usize,
}

impl Budget {
    fn new(limits: ExportLimits) -> Result<Self, FailureReport> {
        let mut result = Self {
            limits,
            nodes: 0,
            bytes: 0,
        };
        result.nodes(1, 1)?;
        result.add_bytes(size_of::<JsData>())?;
        Ok(result)
    }

    fn nodes(&mut self, count: usize, depth: usize) -> Result<(), FailureReport> {
        if count != 0 && depth > self.limits.max_depth {
            return Err(capacity());
        }
        let total = self.nodes.checked_add(count).ok_or_else(capacity)?;
        if total > self.limits.max_nodes {
            return Err(capacity());
        }
        self.nodes = total;
        Ok(())
    }

    fn add_bytes(&mut self, bytes: usize) -> Result<(), FailureReport> {
        let total = self.bytes.checked_add(bytes).ok_or_else(capacity)?;
        if total > self.limits.max_bytes {
            return Err(capacity());
        }
        self.bytes = total;
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.limits.max_bytes - self.bytes
    }

    /// How many more nodes the budget can still take.
    ///
    /// # Returns
    /// `max_nodes - nodes`, saturating at zero. A container with more children than this
    /// always fails [`Self::nodes`], so its children need not be retained.
    fn node_allowance(&self) -> usize {
        self.limits.max_nodes.saturating_sub(self.nodes)
    }

    fn vector<T>(&mut self, length: usize) -> Result<Vec<T>, FailureReport> {
        let expected = length.checked_mul(size_of::<T>()).ok_or_else(capacity)?;
        // Reject before asking the allocator to create a potentially large vector.
        self.add_bytes(expected)?;
        let mut values = Vec::new();
        values.try_reserve_exact(length).map_err(|_| capacity())?;
        let actual = values
            .capacity()
            .checked_mul(size_of::<T>())
            .ok_or_else(capacity)?;
        self.add_bytes(actual.checked_sub(expected).ok_or_else(capacity)?)?;
        Ok(values)
    }
}

/// Snapshot storage belongs to the native context and is freed on every exit.
struct Keys {
    context: *mut ffi::Context,
    raw: *mut ffi::Keys,
    length: usize,
}
impl Drop for Keys {
    fn drop(&mut self) {
        // SAFETY: the exporting scope keeps the context live; raw is one owned
        // snapshot, released exactly once and never exposed outside this walk.
        unsafe { ffi::jbq_keys_free(self.context, self.raw) };
    }
}

enum ImportChildren<'a> {
    Array(std::iter::Enumerate<std::slice::Iter<'a, JsData>>),
    Record(std::slice::Iter<'a, (JsString, JsData)>),
}
struct ImportFrame<'a> {
    native: Rc<NativeValue>,
    children: ImportChildren<'a>,
}
enum ImportKey<'a> {
    Index(u32),
    String(&'a JsString),
}

/// The children of one container being exported.
enum ExportChildren {
    /// A dense array.
    Array {
        /// Element count read once when the array was entered.
        length: usize,
        /// Index of the next element to read.
        next: usize,
        /// Exported elements so far, reserved to `length` against the budget.
        values: Vec<JsData>,
    },
    /// A plain record.
    Record {
        /// Fix: every key was materialised twice (once to count, once to read) and the
        /// snapshot was kept alive for the whole walk. Decision: the counting pass keeps
        /// the native key of each enumerable string key, in order, and this iterator
        /// hands each out once; the snapshot is released as soon as the pass ends.
        keys: std::vec::IntoIter<Rc<NativeValue>>,
        /// The copied key of the child being read, waiting for its value.
        key: Option<JsString>,
        /// Exported entries so far, reserved to the enumerable key count against the budget.
        values: Vec<(JsString, JsData)>,
    },
}
struct ExportFrame {
    native: Rc<NativeValue>,
    depth: usize,
    children: ExportChildren,
}

impl Scope<'_, '_> {
    fn define_import_key<'s>(
        &'s self,
        parent: &NativeValue,
        key: &NativeValue,
        value: &NativeValue,
    ) -> JsResult<'s, ()> {
        // SAFETY: Callers retain all three native values in this context.
        // Definitions use own data properties on a fresh null-prototype record.
        self.check_status(unsafe {
            ffi::jbq_define_key(self.ptr(), parent.value, key.value, value.value)
        })
    }

    fn snapshot_key<'s>(&'s self, keys: &Keys, index: usize) -> JsResult<'s, Rc<NativeValue>> {
        // SAFETY: Callers supply an index within this live context's owned
        // snapshot. Adopt every native key copy immediately, including errors.
        self.owned(unsafe { ffi::jbq_key(self.ptr(), keys.raw, index) })
    }

    /// Copies owned data into a new native value, iteratively.
    ///
    /// # Arguments
    /// * `data` - The data to copy; it must fit the context's export limits.
    ///
    /// # Returns
    /// The scoped root of the new native value.
    ///
    /// # Errors
    /// [`FailureCode::Capacity`] when the data exceeds the limits (checked before any
    /// native allocation), or the engine's failure while building a value.
    pub(super) fn import<'s>(&'s self, data: JsData) -> JsResult<'s, ScopedValue<'s>> {
        // Fix: the limits are walked in a separate pass before the build. Decision: the
        // pre-walk stays. Folding it into the build walk accepts and rejects the same
        // inputs, but an over-limit input would then reach native allocation first, so a
        // native fault (memory limit) could replace the Capacity error and partial values
        // would be built and thrown away. The pass is pure Rust with no native calls.
        data.footprint(self.state.limits.export)?;
        let mut frames = Vec::<ImportFrame<'_>>::new();
        let mut next = Some((&data, None::<(Rc<NativeValue>, ImportKey<'_>)>));
        let mut root = None;
        loop {
            // The loop returns below whenever no child remains, so every
            // iteration starts with a scheduled node, including the initial root.
            let (node, parent) = next.take().expect("an unfinished import has a next node");
            if let JsData::Array(values) = node {
                array_length(values.len() as i128)?;
            }
            // SAFETY: the scope owns a live context. Primitive input buffers
            // are borrowed only during these copying native constructors.
            let raw = unsafe {
                match node {
                    JsData::Undefined => ffi::jbq_undefined(),
                    JsData::Null => ffi::jbq_null(),
                    JsData::Bool(value) => ffi::jbq_bool(self.ptr(), i32::from(*value)),
                    JsData::Number(value) => ffi::jbq_number(self.ptr(), *value),
                    JsData::String(value) => ffi::jbq_string(
                        self.ptr(),
                        value.as_units().as_ptr(),
                        value.as_units().len(),
                    ),
                    JsData::Bytes(value) => ffi::jbq_bytes(self.ptr(), value.as_ptr(), value.len()),
                    JsData::Array(_) => ffi::jbq_array(self.ptr()),
                    JsData::Record(_) => ffi::jbq_record(self.ptr()),
                }
            };
            let native = self.owned(raw)?;
            if let Some((parent, key)) = parent {
                match key {
                    ImportKey::Index(index) => {
                        // SAFETY: both values are protected; define uses own
                        // data properties on a newly created native array.
                        self.check_status(unsafe {
                            ffi::jbq_define_index(self.ptr(), parent.value, index, native.value)
                        })?;
                    }
                    ImportKey::String(key) => {
                        // SAFETY: the key buffer is valid for the constructor.
                        let key = self.owned(unsafe {
                            ffi::jbq_string(
                                self.ptr(),
                                key.as_units().as_ptr(),
                                key.as_units().len(),
                            )
                        })?;
                        self.define_import_key(&parent, &key, &native)?;
                    }
                }
            } else {
                root = Some(native.clone());
            }
            let children = match node {
                JsData::Array(values) => Some(ImportChildren::Array(values.iter().enumerate())),
                JsData::Record(values) => Some(ImportChildren::Record(values.iter())),
                _ => None,
            };
            if let Some(children) = children {
                frames.push(ImportFrame { native, children });
            } else {
                // A primitive is already retained by the root or its native
                // parent property; release this temporary wrapper here.
                drop(native);
            }
            while let Some(frame) = frames.last_mut() {
                let child = match &mut frame.children {
                    ImportChildren::Array(values) => values
                        .next()
                        .map(|(index, value)| (value, ImportKey::Index(index as u32))),
                    ImportChildren::Record(values) => values
                        .next()
                        .map(|(key, value)| (value, ImportKey::String(key))),
                };
                if let Some((node, key)) = child {
                    next = Some((node, Some((frame.native.clone(), key))));
                    break;
                }
                frames.pop();
            }
            if next.is_none() {
                let root = root.expect("the import walk always visits its root");
                // SAFETY: duplicate the protected root for the scope store, which
                // consumes its new ownership while this local guard is released.
                return self.put(unsafe { ffi::jbq_dup(self.ptr(), root.value) });
            }
        }
    }

    pub(super) fn export<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData> {
        let root = self.read(value)?;
        let mut budget = Budget::new(limits)?;
        let mut frames = Vec::<ExportFrame>::new();
        let mut next = Some((root, 1usize));
        let mut completed = None;
        loop {
            if let Some((native, depth)) = next.take() {
                completed = match self.native_kind(native.value) {
                    ValueKind::Undefined => Some(JsData::Undefined),
                    ValueKind::Null => Some(JsData::Null),
                    ValueKind::Boolean => {
                        // SAFETY: classification proved a primitive Boolean.
                        Some(JsData::Bool(
                            unsafe { ffi::jbq_to_bool(self.ptr(), native.value) } != 0,
                        ))
                    }
                    ValueKind::Number => Some(JsData::Number(self.number(native.value)?)),
                    ValueKind::String => {
                        let string = self.string(native.value, budget.remaining() / 2)?;
                        budget.add_bytes(string.capacity_bytes())?;
                        Some(JsData::String(string))
                    }
                    ValueKind::Symbol | ValueKind::BigInt | ValueKind::Function => {
                        return Err(unsupported("value is not supported by copied data").into());
                    }
                    ValueKind::Object => {
                        // SAFETY: all ancestors and the current object have live
                        // native guards; equality does not execute JavaScript.
                        if frames.iter().any(|frame| unsafe { ffi::jbq_same_object(frame.native.value, native.value) } != 0) {
                            return Err(unsupported("copied data contains a cycle").into());
                        }
                        let mut bytes = ptr::null();
                        let mut length = 0;
                        // SAFETY: native is protected, and output slots are valid.
                        // A successful borrowed pointer is copied before any
                        // subsequent native entry, including a destructor.
                        let status = unsafe {
                            ffi::jbq_bytes_data(self.ptr(), native.value, &mut bytes, &mut length)
                        };
                        if status < 0 {
                            self.check_status(status)?;
                        }
                        if status == 1 {
                            byte_storage(length, !bytes.is_null())?;
                            let mut output = budget.vector::<u8>(length)?;
                            if length != 0 {
                                // SAFETY: the shim returned length valid bytes;
                                // no native entry has occurred since that call.
                                output.extend_from_slice(unsafe {
                                    std::slice::from_raw_parts(bytes, length)
                                });
                            }
                            Some(JsData::Bytes(output))
                        } else {
                            // SAFETY: native is a protected object; classification
                            // rejects proxy/exotic arrays without invoking traps.
                            let children = if unsafe { ffi::jbq_is_array(native.value) } != 0 {
                                let mut length = 0i64;
                                // SAFETY: ordinary native array and valid output.
                                self.check_status(unsafe {
                                    ffi::jbq_length(self.ptr(), native.value, &mut length)
                                })?;
                                let length = array_length(i128::from(length))?;
                                budget.nodes(length, depth.checked_add(1).ok_or_else(capacity)?)?;
                                ExportChildren::Array {
                                    length,
                                    next: 0,
                                    values: budget.vector(length)?,
                                }
                            } else {
                                // SAFETY: this probe uses only intrinsic native
                                // object/prototype metadata, with a live guard.
                                let plain =
                                    unsafe { ffi::jbq_is_plain_record(self.ptr(), native.value) };
                                self.check_status(plain)?;
                                if plain == 0 {
                                    return Err(unsupported(
                                        "object semantics are not supported by copied data",
                                    )
                                    .into());
                                }
                                let mut length = 0;
                                // SAFETY: the object is protected; the returned
                                // snapshot is immediately placed under a guard.
                                let raw = unsafe {
                                    ffi::jbq_own_keys(self.ptr(), native.value, &mut length)
                                };
                                if raw.is_null() {
                                    // Negative native status always returns its
                                    // original exception or the pending typed fault.
                                    return Err(self.check_status(-1).unwrap_err());
                                }
                                let keys = Keys {
                                    context: self.ptr(),
                                    raw,
                                    length,
                                };
                                // Fix: record keys were materialised once to count and
                                // reject symbols and again to read each value. Decision:
                                // one pass materialises every key once (symbols must be
                                // rejected before any getter runs, as before) and keeps
                                // the enumerable ones for the walk. Retention is capped
                                // at the node allowance: beyond it `budget.nodes` below
                                // fails with the same error, after the same symbol check.
                                let allowance = budget.node_allowance();
                                let mut count = 0usize;
                                let mut enumerable = Vec::<Rc<NativeValue>>::new();
                                for index in 0..keys.length {
                                    let key = self.snapshot_key(&keys, index)?;
                                    if self.native_kind(key.value) == ValueKind::Symbol {
                                        return Err(unsupported(
                                            "symbol-keyed records cannot be copied",
                                        )
                                        .into());
                                    }
                                    // SAFETY: index is within the live snapshot.
                                    if unsafe { ffi::jbq_key_enumerable(keys.raw, index) } != 0 {
                                        count = count.checked_add(1).ok_or_else(capacity)?;
                                        if enumerable.len() < allowance {
                                            enumerable.try_reserve(1).map_err(|_| capacity())?;
                                            enumerable.push(key);
                                        }
                                    }
                                }
                                // The snapshot is no longer needed; the kept keys are
                                // independent protected values.
                                drop(keys);
                                budget.nodes(count, depth.checked_add(1).ok_or_else(capacity)?)?;
                                // `nodes` succeeded, so count <= allowance and every
                                // enumerable key was kept.
                                ExportChildren::Record {
                                    keys: enumerable.into_iter(),
                                    key: None,
                                    values: budget.vector(count)?,
                                }
                            };
                            frames.push(ExportFrame {
                                native,
                                depth,
                                children,
                            });
                            None
                        }
                    }
                };
            }
            if let Some(data) = completed.take() {
                if let Some(frame) = frames.last_mut() {
                    match &mut frame.children {
                        ExportChildren::Array { values, .. } => values.push(data),
                        ExportChildren::Record { key, values, .. } => {
                            values
                                .push((key.take().expect("record child has its copied key"), data));
                        }
                    }
                } else {
                    // Fix: a second full footprint walk ran over the finished tree.
                    // Decision: removed. `Budget` already enforced the same three limits
                    // while building: depth (checked for every child of a non-empty
                    // container, and for the root), nodes (root plus every child count),
                    // and bytes (JsData size, string/byte/array/record capacities as
                    // allocated, and each key's capacity), all on the capacities that
                    // `footprint` reads. No vector is reallocated after it is reserved.
                    return Ok(data);
                }
            }
            let frame = frames
                .last_mut()
                .expect("unfinished export has an active container");
            let child = match &mut frame.children {
                ExportChildren::Array { length, next, .. } if *next < *length => {
                    let index = *next as u32;
                    // SAFETY: protected ordinary array, intrinsic own-property
                    // probe distinguishes a hole from present undefined.
                    let present =
                        unsafe { ffi::jbq_has_own_index(self.ptr(), frame.native.value, index) };
                    self.check_status(present)?;
                    if present == 0 {
                        return Err(unsupported("sparse arrays cannot be copied").into());
                    }
                    *next += 1;
                    // SAFETY: protected array; ordinary Get preserves getter
                    // semantics and owned propagates its original exception.
                    Some(self.owned(unsafe {
                        ffi::jbq_get_index(self.ptr(), frame.native.value, index)
                    })?)
                }
                ExportChildren::Record {
                    keys,
                    key: output_key,
                    ..
                } => {
                    // Only enumerable keys were kept, so the next one is the next child.
                    if let Some(key) = keys.next() {
                        let string = self.string(key.value, budget.remaining() / 2)?;
                        budget.add_bytes(string.capacity_bytes())?;
                        *output_key = Some(string);
                        // SAFETY: object/key are protected; Get invokes exactly
                        // this getter before later keys, preserving native order.
                        Some(self.owned(unsafe {
                            ffi::jbq_get(self.ptr(), frame.native.value, key.value)
                        })?)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(child) = child {
                next = Some((child, frame.depth.checked_add(1).ok_or_else(capacity)?));
            } else {
                let frame = frames.pop().expect("active container exists");
                completed = Some(match frame.children {
                    ExportChildren::Array { values, .. } => JsData::Array(values),
                    ExportChildren::Record { values, .. } => JsData::Record(values),
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/quickjs_ng_data_coverage.rs"]
mod tests;
