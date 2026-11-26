//! Bounded inspection in deterministic native storage order. Cursors retain roots
//! and offsets. Pending collection records are pinned independently of JavaScript
//! iterators, and roots stay live until release. Evaluation/resume invalidates
//! each cursor before any mutation.
use super::*;
/// Owner-local dedup identity, never returned to clients or used as code.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct EditKey {
    receiver: [u64; 2],
    frame: i32,
    scope: u32,
    atom: u32,
    kind: u32,
    index: u32,
}
pub(super) struct EditTarget {
    native: std::ptr::NonNull<c_void>,
    raw: Weak<RawContext>,
}
impl Drop for EditTarget {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.upgrade()
            && let Some(ptr) = raw.pointer.get()
        {
            // SAFETY: this owner-local target owns its exact atom and native root.
            unsafe { ffi::jbq_debug_target_free(ptr.as_ptr(), self.native.as_ptr()) };
        }
    }
}
#[derive(Clone)]
pub(super) struct CursorRecord {
    parent: VariableParent,
    frame: i32,
    scope: u32,
    native: NativeCursor,
}
struct NativeCursor {
    root: Option<Rc<NativeValue>>,
    raw: Weak<RawContext>,
    cursor: ffi::DebugCursor,
}
impl Clone for NativeCursor {
    fn clone(&self) -> Self {
        let mut cursor = self.cursor;
        // SAFETY: the owner-local source cursor retains its collection root and pin.
        unsafe { ffi::jbq_debug_cursor_retain(&mut cursor) };
        Self {
            root: self.root.clone(),
            raw: self.raw.clone(),
            cursor,
        }
    }
}
impl Drop for NativeCursor {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.upgrade()
            && let Some(pointer) = raw.pointer.get()
        {
            // SAFETY: owner-local cursor owns its pending native record pin;
            // its collection root remains retained until this destructor returns.
            unsafe { ffi::jbq_debug_cursor_release(pointer.as_ptr(), &mut self.cursor) };
        }
    }
}
impl QuickJsDebug {
    pub(super) fn inspect(
        &self,
        ptr: *mut ffi::Context,
        request: InspectionRequest,
        pause: Option<PauseId>,
    ) -> Result<DebugReply, DebugError> {
        let port = self.port()?;
        let current = match request.parent {
            VariableParent::Object(object) => self.current(pause, object.pause)?,
            VariableParent::Scope(scope) => self.current(pause, scope.pause)?,
        };
        let same_parent = |a: VariableParent, b: VariableParent| match (a, b) {
            (VariableParent::Object(a), VariableParent::Object(b)) => a == b,
            (VariableParent::Scope(a), VariableParent::Scope(b)) => a == b,
            _ => false,
        };
        let (cursor_key, mut cursor) = if let Some(reference) = request.cursor {
            let k = slot(
                port.identities()
                    .resolve(DebugReference::Cursor(reference))?,
            );
            let record = self
                .state
                .borrow()
                .cursors
                .get(&k)
                .cloned()
                .ok_or(DebugError::StaleInspection)?;
            if !same_parent(record.parent, request.parent) || request.page.start != 0 {
                return Err(DebugError::InvalidReference);
            }
            (k, record)
        } else {
            let (root, frame, scope) = match request.parent {
                VariableParent::Object(object) => {
                    let i =
                        slot(port.identities().resolve(DebugReference::Object(object))?) as usize;
                    let root = self
                        .state
                        .borrow()
                        .objects
                        .get(i)
                        .cloned()
                        .ok_or(DebugError::InvalidReference)?;
                    (Some(root), -1, 0)
                }
                VariableParent::Scope(scope) => {
                    let k = slot(port.identities().resolve(DebugReference::Scope(scope))?);
                    (None, (k >> 32) as i32, k as u32)
                }
            };
            let mut state = self.state.borrow_mut();
            if state.cursors.len() >= port.limits().inspection_objects {
                return Err(DebugError::Capacity);
            }
            let k = state.next_cursor;
            state.next_cursor = k.checked_add(1).ok_or(DebugError::Capacity)?;
            (
                k,
                CursorRecord {
                    parent: request.parent,
                    frame,
                    scope,
                    native: NativeCursor {
                        root,
                        raw: self.raw.clone(),
                        cursor: ffi::DebugCursor {
                            skip_remaining: request.page.start,
                            ..ffi::DebugCursor::default()
                        },
                    },
                },
            )
        };
        let count = request
            .page
            .count
            .min(port.limits().inspection_page_items)
            .min(
                port.limits()
                    .reply_bytes
                    .saturating_sub(std::mem::size_of::<DebugReply>())
                    / std::mem::size_of::<InspectionEntry>(),
            );
        if count == 0 {
            return Err(DebugError::Capacity);
        }
        let mut output = Output {
            debug: self,
            pause: current,
            items: Vec::new(),
            error: None,
            remaining: 0,
            receiver: cursor
                .native
                .root
                .as_ref()
                .map_or(ffi::Value { words: [0; 2] }, |r| r.value),
            frame: cursor.frame,
            scope: cursor.scope,
            inserted: Vec::new(),
            committed: false,
        };
        output.remaining = port
            .limits()
            .reply_bytes
            .checked_sub(std::mem::size_of::<DebugReply>())
            .ok_or(DebugError::Capacity)?;
        let mut scanned = 0;
        let mut complete = 0;
        let object = cursor
            .native
            .root
            .as_ref()
            .map_or(ffi::Value { words: [0; 2] }, |r| r.value);
        // SAFETY: root and selected paused frame remain live on the owner thread;
        // native cursor owns its collection pin and callback borrows output synchronously.
        let result = unsafe {
            ffi::jbq_debug_inspect(
                ptr,
                object,
                cursor.frame,
                cursor.scope,
                &mut cursor.native.cursor,
                count,
                port.limits().inspection_scan_slots,
                entry,
                (&mut output as *mut Output<'_>).cast(),
                &mut scanned,
                &mut complete,
            )
        };
        if result < 0 {
            return Err(if result == -2 {
                DebugError::Capacity
            } else {
                DebugError::Unsupported
            });
        }
        if let Some(error) = output.error.take() {
            return Err(error);
        }
        let next = if complete != 0 {
            self.state.borrow_mut().cursors.remove(&cursor_key);
            if let Some(reference) = request.cursor {
                port.identities().release_cursor(reference)?;
            }
            None
        } else {
            let DebugReference::Cursor(reference) =
                port.identities()
                    .register(current, ReferenceKind::Cursor, key(cursor_key))?
            else {
                return Err(DebugError::InvalidReference);
            };
            self.state.borrow_mut().cursors.insert(cursor_key, cursor);
            Some(reference)
        };
        output.committed = true;
        Ok(DebugReply::Inspection(InspectionPage {
            items: std::mem::take(&mut output.items),
            next,
            complete: complete != 0,
            scanned_slots: scanned,
            total: None,
        }))
    }
}
struct Output<'a> {
    debug: &'a QuickJsDebug,
    pause: PauseId,
    items: Vec<InspectionEntry>,
    error: Option<DebugError>,
    remaining: usize,
    receiver: ffi::Value,
    frame: i32,
    scope: u32,
    inserted: Vec<(EditKey, DebugReference)>,
    committed: bool,
}
impl Drop for Output<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for (key, reference) in &self.inserted {
                self.debug.state.borrow_mut().edit_targets.remove(key);
                if let Ok(port) = self.debug.port() {
                    let _ = port.identities().release_target(*reference);
                }
            }
        }
    }
}
unsafe extern "C" fn entry(
    opaque: *mut c_void,
    ptr: *mut ffi::Context,
    name: ffi::Value,
    value: ffi::Value,
    flags: i32,
    atom: u32,
    kind: u32,
    index: u32,
) -> i32 {
    // SAFETY: inspect passes an exclusive live output for this synchronous visit.
    let output = unsafe { &mut *opaque.cast::<Output<'_>>() };
    if output.error.is_some() {
        return 0;
    }
    let result = contain(|| {
        let mut name_length = 0;
        // SAFETY: bridge supplies an atom's primitive string; length query copies no units.
        if unsafe { ffi::jbq_debug_copy_string(name, std::ptr::null_mut(), 0, &mut name_length) }
            < 0
        {
            return Err(DebugError::Capacity);
        }
        if name_length > 65_536 {
            return Err(DebugError::Capacity);
        }
        let name_units = name_length
            + if flags & 8 != 0 {
                48
            } else if flags & 2 != 0 {
                42
            } else {
                0
            };
        if name_units * 2 > output.remaining {
            return if output.items.is_empty() {
                Err(DebugError::Capacity)
            } else {
                Ok(false)
            };
        }
        // SAFETY: native visitor lends a live value, and kind/TDZ reads invoke no code.
        let value_kind = unsafe { ffi::jbq_kind(ptr, value) };
        let type_name = if flags & 1 != 0 {
            JsString::from_utf8("accessor")
        } else if flags & 4 != 0 {
            JsString::from_utf8("uninitialized")
        } else if let Some(info) = output.debug.raw_info(ptr, value) {
            let length = info.type_name.iter().position(|b| *b == 0).unwrap_or(32);
            JsString::from_utf8(
                std::str::from_utf8(&info.type_name[..length])
                    .map_err(|_| DebugError::Unsupported)?,
            )
        } else {
            JsString::from_utf8(match value_kind {
                0 => "undefined",
                1 => "null",
                2 => "boolean",
                3 => "number",
                4 => "string",
                5 => "symbol",
                6 => "bigint",
                8 => "function",
                _ => "object",
            })
        };
        let value = if flags & 1 != 0 {
            DebugValue {
                accessor: true,
                ..unavailable("getter not invoked")
            }
        } else if flags & 4 != 0 {
            unavailable("lazy property not initialized")
        }
        // SAFETY: tag-only inspection of the borrowed native value.
        else if unsafe { ffi::jbq_debug_uninitialized(value) } != 0 {
            unavailable("binding is uninitialized")
        } else {
            // SAFETY: retain visitor's borrowed value in its live context.
            output.debug.preview(
                ptr,
                output.pause,
                owned(unsafe { ffi::jbq_dup(ptr, value) }, &output.debug.raw)?,
            )
        };
        let bytes = name_units * 2
            + type_name.capacity_bytes()
            + value.preview.capacity_bytes()
            + value
                .unavailable
                .as_ref()
                .map_or(0, |reason| reason.capacity());
        let vector_bytes = if output.items.len() == output.items.capacity() {
            std::mem::size_of::<InspectionEntry>()
        } else {
            0
        };
        if bytes.saturating_add(vector_bytes) > output.remaining {
            return if output.items.is_empty() {
                Err(DebugError::Capacity)
            } else {
                Ok(false)
            };
        }
        let previous = output.items.capacity();
        output
            .items
            .try_reserve_exact(1)
            .map_err(|_| DebugError::Capacity)?;
        let vector_bytes =
            (output.items.capacity() - previous) * std::mem::size_of::<InspectionEntry>();
        if bytes.saturating_add(vector_bytes) > output.remaining {
            return Err(DebugError::Capacity);
        }
        let mut name = string(ptr, name, None)?;
        if flags & 2 != 0 {
            let mut units = Vec::new();
            units
                .try_reserve_exact(name_units)
                .map_err(|_| DebugError::Capacity)?;
            if flags & 8 != 0 {
                units.extend("[Symbol(<absent>)]".encode_utf16());
            } else {
                units.extend("[Symbol(\"".encode_utf16());
                units.extend_from_slice(name.as_units());
                units.extend("\")]".encode_utf16());
            }
            // Atom identity is exact and retained by writable target metadata;
            // include it in every symbol label to distinguish duplicates across pages.
            units.extend(format!(" #{}", atom).encode_utf16());
            name = JsString::from_units(units);
        }
        let key = EditKey {
            receiver: output.receiver.words,
            frame: output.frame,
            scope: output.scope,
            atom,
            kind,
            index,
        };
        let mut write_unavailable = None;
        let write = if kind == 0 {
            None
        } else {
            let existing = output
                .debug
                .state
                .borrow()
                .edit_targets
                .get(&key)
                .map(|(id, _)| *id);
            let port = output.debug.port()?;
            let id = if let Some(id) = existing {
                Some(id)
            } else {
                if output.debug.state.borrow().edit_targets.len() >= port.limits().write_targets {
                    write_unavailable = Some(WriteUnavailable::Capacity);
                    None
                } else {
                    let mut status = 0;
                    // SAFETY: callback lends exact atom/slot metadata in this paused owner context.
                    let native = unsafe {
                        ffi::jbq_debug_target_new(
                            ptr,
                            output.receiver,
                            output.frame,
                            output.scope,
                            atom,
                            kind,
                            index,
                            &mut status,
                        )
                    };
                    if status == -2 {
                        return Err(DebugError::Capacity);
                    }
                    if let Some(native) = std::ptr::NonNull::new(native) {
                        let target = Rc::new(EditTarget {
                            native,
                            raw: output.debug.raw.clone(),
                        });
                        output
                            .inserted
                            .try_reserve(1)
                            .map_err(|_| DebugError::Capacity)?;
                        output
                            .debug
                            .state
                            .borrow_mut()
                            .edit_targets
                            .try_reserve(1)
                            .map_err(|_| DebugError::Capacity)?;
                        let id = {
                            let mut state = output.debug.state.borrow_mut();
                            state.next_target = state
                                .next_target
                                .checked_add(1)
                                .filter(|id| *id < u64::MAX)
                                .ok_or(DebugError::Capacity)?;
                            state.next_target
                        };
                        let reference = port.identities().register(
                            output.pause,
                            if kind == 1 {
                                ReferenceKind::Property
                            } else {
                                ReferenceKind::Binding
                            },
                            super::key(id),
                        )?;
                        output.inserted.push((key, reference));
                        output
                            .debug
                            .state
                            .borrow_mut()
                            .edit_targets
                            .insert(key, (id, target));
                        Some(id)
                    } else {
                        None
                    }
                }
            };
            id.map(|id| {
                port.identities().register(
                    output.pause,
                    if kind == 1 {
                        ReferenceKind::Property
                    } else {
                        ReferenceKind::Binding
                    },
                    super::key(id),
                )
            })
            .transpose()?
            .map(|reference| match reference {
                DebugReference::Property(r) => WriteTarget::Property(r),
                DebugReference::Binding(r) => WriteTarget::Binding(r),
                _ => unreachable!(),
            })
        };
        output.remaining -= bytes + vector_bytes;
        output.items.push(InspectionEntry {
            variable: DebugVariable { name, value },
            type_name,
            write,
            write_unavailable,
        });
        Ok::<_, DebugError>(true)
    });
    match result {
        Ok(Ok(accepted)) => i32::from(accepted),
        Ok(Err(error)) => {
            output.error = Some(error);
            0
        }
        Err(_) => {
            output.error = Some(DebugError::Unsupported);
            0
        }
    }
}

impl QuickJsDebug {
    pub(super) fn write_target(
        &self,
        ptr: *mut ffi::Context,
        id: DebugRequestId,
        target: WriteTarget,
        frame: FrameRef,
        expression: &JsString,
        pause: Option<PauseId>,
    ) -> Result<DebugReply, DebugError> {
        let pause = self.current(pause, frame.pause)?;
        let port = self.port()?;
        let frame_index = slot(port.identities().resolve(DebugReference::Frame(frame))?) as i32;
        let reference = match target {
            WriteTarget::Binding(r) => DebugReference::Binding(r),
            WriteTarget::Property(r) => DebugReference::Property(r),
        };
        let target_id = slot(port.identities().resolve(reference)?);
        let target = self
            .state
            .borrow()
            .edit_targets
            .values()
            .find(|(id, _)| *id == target_id)
            .map(|(_, t)| t.clone())
            .ok_or(DebugError::StaleInspection)?;
        // SAFETY: resolved owner-local target retains its exact native root/atom.
        if unsafe { ffi::jbq_debug_target_validate(ptr, target.native.as_ptr()) } < 0 {
            return Err(DebugError::InvalidRequest);
        }
        let control = port
            .evaluation_control(id)
            .ok_or(DebugError::InvalidRequest)?;
        let (value, status) =
            self.evaluate_controlled_raw(ptr, frame_index, expression, control.clone())?;
        let valid = status==EvaluationStatus::Returned &&
            // SAFETY: operation retains target across inspection invalidation/RHS GC.
            unsafe {ffi::jbq_debug_target_validate(ptr,target.native.as_ptr())}==0;
        let raw = self.raw.upgrade().ok_or(DebugError::Closed)?;
        if port.is_stopping() || raw.interrupt.load(Ordering::Relaxed) {
            return Err(DebugError::Closed);
        }
        // Seal cancellation/deadline immediately before the allocation-free native
        // recheck/store. No JavaScript or host call occurs between this and store.
        match control.finish(status).ok_or(DebugError::Closed)? {
            EvaluationStatus::Cancelled => {
                return Err(DebugError::EvaluationCancelled {
                    effects_may_remain: true,
                });
            }
            EvaluationStatus::DeadlineExceeded => {
                return Err(DebugError::EvaluationDeadlineExceeded {
                    effects_may_remain: true,
                });
            }
            EvaluationStatus::Threw => return Err(DebugError::InvalidRequest),
            EvaluationStatus::Returned => (),
        }
        if !valid {
            return Err(DebugError::InvalidRequest);
        }
        let value = value.ok_or(DebugError::InvalidRequest)?;
        if port.is_stopping() || raw.interrupt.load(Ordering::Relaxed) {
            return Err(DebugError::Closed);
        }
        // SAFETY: store borrows the actual owned RHS; native freshly resolves the
        // slot/descriptor, duplicates RHS, installs it, then releases the old value.
        if unsafe { ffi::jbq_debug_target_store(ptr, target.native.as_ptr(), value.value) } < 0 {
            return Err(DebugError::InvalidRequest);
        }
        Ok(DebugReply::Write(self.preview(ptr, pause, value)))
    }
}

impl QuickJsDebug {
    pub(super) fn release_write_targets(
        &self,
        targets: Vec<WriteTarget>,
    ) -> Result<DebugReply, DebugError> {
        let port = self.port()?;
        let reference = |t: WriteTarget| match t {
            WriteTarget::Binding(r) => DebugReference::Binding(r),
            WriteTarget::Property(r) => DebugReference::Property(r),
        };
        // Validate the complete batch before releasing any resource.
        for target in &targets {
            port.identities().target_for_release(reference(*target))?;
        }
        for (at, target) in targets.iter().enumerate() {
            if targets[..at].contains(target) {
                continue;
            }
            let r = reference(*target);
            let Some(native_key) = port.identities().target_for_release(r)? else {
                continue;
            };
            let id = slot(native_key);
            self.state
                .borrow_mut()
                .edit_targets
                .retain(|_, (known, _)| *known != id);
            port.identities().release_target(r)?;
        }
        Ok(DebugReply::WriteTargetsReleased)
    }
}
