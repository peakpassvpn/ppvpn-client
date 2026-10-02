//! `ppvpn`: the PeakPass VPN command-line client. It runs ppvpn-core's
//! standard instance (local HTTP/SOCKS5 proxy, no TUN) in its own process.

// musl's allocator is much slower under load than glibc's; see Cargo.toml.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let env = ppvpn_cli::env::Env::from_process();
    let code = ppvpn_cli::run(&args, &env, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}
