//! lunarstorm patch: jetbunny's `quickjs_ng_copy_alloc.rs`, unchanged, as its own
//! test binary (it installs a global allocator, which a unit-test build already
//! has). The alias makes its `use coldmind_runtime::...` resolve.
extern crate lunarstorm_debugger as coldmind_runtime;
#[path = "quickjs_ng_copy_alloc.rs"]
mod copy_alloc;
