//! `ppvpn`: the PeakPass VPN command-line client. It runs ppvpn-core's
//! standard instance (local HTTP/SOCKS5 proxy, no TUN) in its own process.

// musl's allocator is much slower under load than glibc's; see Cargo.toml.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    limit_malloc_arenas();
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let env = ppvpn_cli::env::Env::from_process();
    let code = ppvpn_cli::run(&args, &env, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}

/// glibc keeps per-thread arenas and does not hand their memory back when
/// engine instances stop and start again, so a long-running daemon's RSS
/// climbs (sail measured 64 → 264 MB; about 90 MB with two arenas). Called
/// before any thread exists; an explicit `MALLOC_ARENA_MAX` wins.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn limit_malloc_arenas() {
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        // SAFETY: mallopt only adjusts allocator settings; no other thread
        // is allocating yet.
        unsafe {
            libc::mallopt(libc::M_ARENA_MAX, 2);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn limit_malloc_arenas() {}
