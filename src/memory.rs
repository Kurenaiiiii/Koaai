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

/// RSS snapshot: (bot_self_mb, child_procs_mb, child_count, child_names).
///
/// Reads /proc directly — no new dependencies. Splits "the bot is leaking"
/// from "a yt-dlp/node child is alive" from "the panel is counting page
/// cache", since Pterodactyl reports the whole cgroup (RSS + cache) as one
/// number. A bare `yt-dlp --version` peaks near 60MB RSS, so a live streaming
/// child dwarfs the bot itself — that is normal, not a leak.
///
/// On non-Linux returns zeros.
pub fn snapshot() -> (u64, u64, usize, String) {
    #[cfg(target_os = "linux")]
    {
        snapshot_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        (0, 0, 0, String::new())
    }
}

#[cfg(target_os = "linux")]
fn page_kb() -> u64 {
    // SAFETY: sysconf with _SC_PAGESIZE is always valid.
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps <= 0 {
        4
    } else {
        (ps as u64) / 1024
    }
}

#[cfg(target_os = "linux")]
fn snapshot_linux() -> (u64, u64, usize, String) {
    let pk = page_kb();
    let self_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0);
    let self_mb = self_pages * pk / 1024;

    let me = std::process::id();
    let mut child_kb = 0u64;
    let mut count = 0usize;
    let mut names: Vec<String> = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for e in dir.flatten() {
            let pid_str = e.file_name().to_string_lossy().into_owned();
            if pid_str.parse::<u32>().is_err() {
                continue;
            }
            let stat = match std::fs::read_to_string(format!("/proc/{pid_str}/stat")) {
                Ok(s) => s,
                Err(_) => continue, // raced a process exit
            };
            // comm may contain spaces/parens — split after the LAST ')'.
            // fields after comm: state(0) ppid(1) ... rss_pages(21).
            let (comm, after) = match (stat.find('('), stat.rfind(')')) {
                (Some(a), Some(b)) if b > a => (stat[a + 1..b].to_string(), stat[b + 1..].to_string()),
                _ => continue,
            };
            let f: Vec<&str> = after.split_whitespace().collect();
            if f.len() < 22 {
                continue;
            }
            if f[1].parse::<u32>().ok() != Some(me) {
                continue;
            }
            child_kb += f[21].parse::<u64>().unwrap_or(0) * pk;
            count += 1;
            if names.len() < 6 && !names.iter().any(|n| n == &comm) {
                names.push(comm);
            }
        }
    }
    (self_mb, child_kb / 1024, count, names.join(","))
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshot_sane() {
        let (self_mb, _, _, _) = super::snapshot();
        #[cfg(target_os = "linux")]
        assert!(self_mb > 0, "the bot itself must have RSS");
    }
}


