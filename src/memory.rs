//! Keeping the app's memory to what it uses — two measured costs that had nothing to do with what it
//! shows.
//!
//! **A graphics stack nobody draws with.** wgpu, iced's GPU renderer, probes every backend it has
//! by default, OpenGL included — and on Linux the OpenGL probe loads a second driver stack beside the
//! Vulkan one it renders with: the vendor's EGL, and Mesa's with LLVM behind it. On an NVIDIA
//! desktop that was 50 MB of a 260 MB process. [`gpu_backends`] keeps wgpu to each platform's own
//! API — Vulkan, DirectX 12, Metal — and a machine where none of those works still opens a window:
//! iced falls back to its software renderer.
//!
//! **Memory freed and kept.** glibc's allocator holds what a burst of work frees in per-thread pools
//! rather than handing it back — and the library refresh (a few thousand games parsed, rebuilt and
//! written to the cache) and a batch of art decoded on worker threads are exactly such bursts. After
//! a refresh the app held some 40 MB it was not using. [`tune`] caps the pools at two, and
//! [`release`] hands back what is free once such a burst is over.

/// Keep wgpu to the platform's own GPU API — see the module notes. `WGPU_BACKEND` set by hand wins.
/// Call before anything starts a thread.
pub fn gpu_backends() {
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // SAFETY: called first thing in `main`, before any thread exists to read the environment.
        unsafe { std::env::set_var("WGPU_BACKEND", "vulkan,dx12,metal") };
    }
}

/// Cap glibc's per-thread allocation pools at two. Call before anything starts a thread.
pub fn tune() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `mallopt` only sets an allocator parameter; M_ARENA_MAX takes any positive count.
    unsafe {
        glibc::mallopt(glibc::M_ARENA_MAX, 2);
    }
}

/// Hand what the allocator holds free back to the system — after work that allocated a lot and is
/// done with it. Costs a few milliseconds; a no-op where the allocator is not glibc's.
pub fn release() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` only returns free memory to the system, and takes the allocator's locks.
    unsafe {
        glibc::malloc_trim(0);
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::ffi::c_int;

    /// `M_ARENA_MAX` from glibc's `malloc.h`.
    pub const M_ARENA_MAX: c_int = -8;

    unsafe extern "C" {
        pub fn mallopt(param: c_int, value: c_int) -> c_int;
        pub fn malloc_trim(pad: usize) -> c_int;
    }
}
