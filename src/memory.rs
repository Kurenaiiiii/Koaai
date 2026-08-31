/// Best-effort return of freed heap pages to the OS.
///
/// On glibc (Linux) the allocator keeps freed blocks in freelists and RSS
/// stays high until `malloc_trim(0)` is called. Rust's global allocator
/// uses glibc malloc by default, so long-running bots creep in RSS even
/// though the heap is logically free. Periodic trimming keeps RSS flat.
///
/// On non-Linux targets this is a no-op.
#[inline]
pub fn trim() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: malloc_trim never fails in a way we need to handle; 0 = trim all possible.
        unsafe {
            libc::malloc_trim(0);
        }
    }
}


