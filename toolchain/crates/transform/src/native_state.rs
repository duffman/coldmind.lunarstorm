//! Tagged napi externals: plugin state shared between JS and concurrent hook invocations.
//!
//! Bun passes the `external` given to `build.onBeforeParse` to the hook as the raw data pointer
//! of a napi external. Every state box starts with a per-type tag so a hook (or a JS call) handed
//! the wrong external rejects it instead of reading foreign memory as its own type.

use std::ffi::c_void;
use std::ptr;

use napi::bindgen_prelude::{FromNapiValue, ToNapiValue, TypeName, ValidateNapiValue};
use napi::{check_status, sys, Status, ValueType};

/// State owned by one plugin instance. `TAG` must be unique per type.
pub trait PluginState: Sync + 'static {
    const TAG: u64;
}

#[repr(C)]
struct Tagged<T> {
    tag: u64,
    value: T,
}

/// A state box moving to JS; the external's finalizer frees it.
pub struct Owned<T: PluginState>(Box<Tagged<T>>);

impl<T: PluginState> Owned<T> {
    pub fn new(value: T) -> Self {
        Self(Box::new(Tagged { tag: T::TAG, value }))
    }
}

/// A state box borrowed from a JS argument for the duration of one call.
pub struct Shared<T: PluginState>(*const T);

impl<T: PluginState> Shared<T> {
    pub fn get(&self) -> &T {
        // SAFETY: the JS argument keeps the external, and so the box, alive for this call.
        unsafe { &*self.0 }
    }
}

/// Reads a state of type `T` from a raw external pointer; `None` for null or another type's box.
///
/// # Safety
/// `data` must be null or the data pointer of a napi external created by this module.
/// The external must remain alive for the entire returned borrow, including every
/// concurrent hook using it. The type tag detects a mismatched owned box; it does
/// not make an arbitrary or already-finalized pointer safe to dereference.
pub unsafe fn from_raw<'a, T: PluginState>(data: *mut c_void) -> Option<&'a T> {
    let tagged = (data as *const Tagged<T>).as_ref()?;
    (tagged.tag == T::TAG).then_some(&tagged.value)
}

unsafe extern "C" fn finalize<T: PluginState>(
    _env: sys::napi_env,
    data: *mut c_void,
    _hint: *mut c_void,
) {
    drop(Box::from_raw(data as *mut Tagged<T>));
}

impl<T: PluginState> TypeName for Owned<T> {
    fn type_name() -> &'static str {
        "External"
    }
    fn value_type() -> ValueType {
        ValueType::External
    }
}

/// Transfers ownership to the JavaScript external only after successful creation.
/// A failed N-API allocation immediately reclaims the box; successful allocations
/// are freed exactly once by the external finalizer.
impl<T: PluginState> ToNapiValue for Owned<T> {
    unsafe fn to_napi_value(env: sys::napi_env, value: Self) -> napi::Result<sys::napi_value> {
        let data = Box::into_raw(value.0) as *mut c_void;
        let mut external = ptr::null_mut();
        let status = sys::napi_create_external(
            env,
            data,
            Some(finalize::<T>),
            ptr::null_mut(),
            &mut external,
        );
        if status != sys::Status::napi_ok {
            drop(Box::from_raw(data as *mut Tagged<T>));
        }
        check_status!(status, "failed to create devermind plugin state")?;
        Ok(external)
    }
}

impl<T: PluginState> TypeName for Shared<T> {
    fn type_name() -> &'static str {
        "External"
    }
    fn value_type() -> ValueType {
        ValueType::External
    }
}

impl<T: PluginState> ValidateNapiValue for Shared<T> {}

impl<T: PluginState> FromNapiValue for Shared<T> {
    unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> napi::Result<Self> {
        let mut data = ptr::null_mut();
        check_status!(
            sys::napi_get_value_external(env, value, &mut data),
            "expected a devermind plugin state"
        )?;
        match from_raw::<T>(data) {
            Some(state) => Ok(Self(state)),
            None => Err(napi::Error::new(
                Status::InvalidArg,
                "external is not a devermind plugin state",
            )),
        }
    }
}
