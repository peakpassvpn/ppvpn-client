//! Read-only Darwin NET_RT_DUMP2 queries, without spawning a command per route.
//! ABI: SDK net/route.h; sockaddr slots use Apple's netstat 32-bit alignment.
use anyhow::{bail, Result};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::{Duration, Instant},
};
#[derive(Debug)]
struct Entry {
    destination: IpAddr,
    mask: IpAddr,
    gateway: Option<String>,
    index: u32,
    flags: i32,
}
fn address(bytes: &[u8], v6: bool, compressed: bool) -> Result<IpAddr> {
    let offset = if v6 { 8 } else { 4 };
    let width = if v6 { 16 } else { 4 };
    if !compressed && bytes.len() < offset + width {
        bail!("ROUTE_SNAPSHOT_MALFORMED");
    }
    let mut out = [0u8; 16];
    if bytes.len() > offset {
        let n = (bytes.len() - offset).min(width);
        out[..n].copy_from_slice(&bytes[offset..offset + n]);
    }
    Ok(if v6 {
        IpAddr::V6(Ipv6Addr::from(out))
    } else {
        IpAddr::V4(Ipv4Addr::new(out[0], out[1], out[2], out[3]))
    })
}
fn parse(mut bytes: &[u8]) -> Result<Vec<Entry>> {
    let mut entries = vec![];
    let header = std::mem::size_of::<libc::rt_msghdr2>();
    while !bytes.is_empty() {
        if bytes.len() < header {
            bail!("ROUTE_SNAPSHOT_MALFORMED");
        }
        let h = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<libc::rt_msghdr2>()) };
        let length = h.rtm_msglen as usize;
        if length < header
            || length > bytes.len()
            || h.rtm_version != libc::RTM_VERSION as u8
            || h.rtm_type != libc::RTM_GET2 as u8
            || h.rtm_addrs & !255 != 0
        {
            bail!("ROUTE_SNAPSHOT_MALFORMED");
        }
        let mut offset = header;
        let mut slots: [Option<&[u8]>; 8] = [None; 8];
        for (i, slot) in slots.iter_mut().enumerate() {
            if h.rtm_addrs & (1 << i) == 0 {
                continue;
            }
            if offset >= length {
                bail!("ROUTE_SNAPSHOT_MALFORMED");
            }
            let n = bytes[offset] as usize;
            let stride = if n == 0 { 4 } else { (n + 3) & !3 };
            if stride > length - offset || n > length - offset || (n != 0 && n < 2) {
                bail!("ROUTE_SNAPSHOT_MALFORMED");
            }
            *slot = Some(&bytes[offset..offset + n]);
            offset += stride;
        }
        bytes = &bytes[length..];
        let Some(dst) = slots[0] else {
            bail!("ROUTE_SNAPSHOT_MALFORMED");
        };
        if dst.len() < 2 {
            bail!("ROUTE_SNAPSHOT_MALFORMED");
        }
        let v6 = match dst[1] as i32 {
            libc::AF_INET => false,
            libc::AF_INET6 => true,
            _ => bail!("ROUTE_SNAPSHOT_UNSUPPORTED_FAMILY"),
        };
        let destination = address(dst, v6, false)?;
        let mask = if let Some(mask) = slots[2] {
            // Darwin compact netmasks do not carry a meaningful sa_family;
            // decode their address bytes using the destination family.
            address(mask, v6, true)?
        } else if h.rtm_flags & libc::RTF_HOST != 0 {
            if v6 {
                IpAddr::V6(Ipv6Addr::from([255; 16]))
            } else {
                IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255))
            }
        } else if v6 {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };
        let gateway = match slots[1] {
            Some(gw) if gw.len() >= 2 && gw[1] as i32 == libc::AF_INET => {
                Some(address(gw, false, false)?.to_string())
            }
            Some(gw) if gw.len() >= 2 && gw[1] as i32 == libc::AF_INET6 => {
                let IpAddr::V6(ip) = address(gw, true, false)? else {
                    unreachable!()
                };
                let mut octets = ip.octets();
                let mut zone = if gw.len() >= 28 {
                    u32::from_ne_bytes(gw[24..28].try_into().unwrap())
                } else {
                    0
                };
                if ip.is_unicast_link_local() {
                    let embedded = u16::from_be_bytes([octets[2], octets[3]]) as u32;
                    if zone != 0 && embedded != 0 && zone != embedded {
                        bail!("ROUTE_SNAPSHOT_MALFORMED");
                    }
                    if zone == 0 {
                        zone = embedded;
                    }
                    octets[2] = 0;
                    octets[3] = 0;
                }
                let ip = Ipv6Addr::from(octets);
                Some(if zone == 0 {
                    ip.to_string()
                } else {
                    format!("{ip}%{zone}")
                })
            }
            _ => None,
        };
        let mut index = h.rtm_index as u32;
        if let Some(ifp) = slots[4] {
            if ifp.len() >= 4 && ifp[1] as i32 == libc::AF_LINK {
                let link = u16::from_ne_bytes([ifp[2], ifp[3]]) as u32;
                if index != 0 && link != 0 && index != link {
                    bail!("ROUTE_SNAPSHOT_MALFORMED");
                }
                if index == 0 {
                    index = link;
                }
            }
        }
        entries.push(Entry {
            destination,
            mask,
            gateway,
            index,
            flags: h.rtm_flags,
        });
    }
    Ok(entries)
}
fn read() -> Result<Vec<Entry>> {
    let start = Instant::now();
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, libc::AF_UNSPEC, 7, 0]; // NET_RT_DUMP2 (SDK sys/socket.h)
    let mut size = 0;
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    const LIMIT: usize = 16 * 1024 * 1024;
    if size > LIMIT {
        bail!("ROUTE_SNAPSHOT_TOO_LARGE");
    }
    let mut bytes = vec![0u8; size.saturating_add(65536).min(LIMIT)];
    size = bytes.len();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if size > bytes.len() {
        bail!("ROUTE_SNAPSHOT_MALFORMED");
    }
    bytes.truncate(size);
    let result = parse(&bytes)?;
    if start.elapsed() > Duration::from_millis(300) {
        bail!("ROUTE_QUERY_TIMEOUT");
    }
    Ok(result)
}
fn select(entries: &[Entry], dst: IpAddr, mask: IpAddr, scope: Option<u32>) -> Option<&Entry> {
    entries.iter().find(|r| {
        r.destination == dst
            && r.mask == mask
            && match scope {
                Some(i) => r.index == i && r.flags & libc::RTF_IFSCOPE != 0,
                None => r.flags & libc::RTF_IFSCOPE == 0,
            }
    })
}
fn require_usable_uplink(entry: &Entry) -> Result<()> {
    if entry.flags & libc::RTF_UP == 0
        || entry.flags & (libc::RTF_REJECT | libc::RTF_BLACKHOLE) != 0
    {
        bail!("ROUTE_QUERY_UNUSABLE");
    }
    Ok(())
}
pub fn query(args: &[&str]) -> Result<Option<String>> {
    let v6 = match (args.contains(&"-inet"), args.contains(&"-inet6")) {
        (true, false) => false,
        (false, true) => true,
        _ => bail!("ROUTE_QUERY_INVALID"),
    };
    let prefix = *args
        .last()
        .ok_or_else(|| anyhow::anyhow!("ROUTE_QUERY_INVALID"))?;
    let (dst, mask): (IpAddr, IpAddr) = match (v6, prefix) {
        (false, "default") => ("0.0.0.0".parse()?, "0.0.0.0".parse()?),
        (true, "default") => ("::".parse()?, "::".parse()?),
        (false, "0.0.0.0/1") => ("0.0.0.0".parse()?, "128.0.0.0".parse()?),
        (false, "128.0.0.0/1") => ("128.0.0.0".parse()?, "128.0.0.0".parse()?),
        (true, "::/1") => ("::".parse()?, "8000::".parse()?),
        (true, "8000::/1") => ("8000::".parse()?, "8000::".parse()?),
        _ => bail!("ROUTE_QUERY_INVALID"),
    };
    let scope = args
        .iter()
        .position(|x| *x == "-ifscope")
        .map(|i| {
            args.get(i + 1)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("ROUTE_QUERY_INVALID"))
        })
        .transpose()?;
    let scope_index = scope
        .map(|name| {
            let name = std::ffi::CString::new(name)?;
            let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
            if index == 0 {
                bail!("ROUTE_QUERY_INTERFACE_MISSING");
            }
            Ok(index)
        })
        .transpose()?;
    let entries = read()?;
    let Some(entry) = select(&entries, dst, mask, scope_index) else {
        return Ok(None);
    };
    if scope_index.is_some() {
        require_usable_uplink(entry)?;
    }
    let mut name = [0i8; libc::IFNAMSIZ];
    if unsafe { libc::if_indextoname(entry.index, name.as_mut_ptr()) }.is_null() {
        bail!("ROUTE_QUERY_INTERFACE_MISSING");
    }
    let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_str()?;
    Ok(Some(format!(
        "destination: {}\nmask: {}\ninterface: {name}\ngateway: {}\nflags: <{}>\n",
        entry.destination,
        entry.mask,
        entry.gateway.as_deref().unwrap_or("link"),
        if entry.flags & libc::RTF_IFSCOPE != 0 {
            "IFSCOPE"
        } else {
            "UP"
        }
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn message(v6: bool, scoped: bool, index: u16) -> Vec<u8> {
        let mut h: libc::rt_msghdr2 = unsafe { std::mem::zeroed() };
        h.rtm_version = libc::RTM_VERSION as u8;
        h.rtm_type = libc::RTM_GET2 as u8;
        h.rtm_index = index;
        h.rtm_addrs = 7;
        h.rtm_flags = libc::RTF_UP | if scoped { libc::RTF_IFSCOPE } else { 0 };
        let mut dst = vec![0u8; if v6 { 28 } else { 16 }];
        dst[0] = dst.len() as u8;
        dst[1] = if v6 { libc::AF_INET6 } else { libc::AF_INET } as u8;
        let mut gw = dst.clone();
        if v6 {
            gw[8] = 0xfe;
            gw[9] = 0x80;
            gw[10..12].copy_from_slice(&index.to_be_bytes());
            gw[23] = 1;
        } else {
            gw[4..8].copy_from_slice(&[192, 0, 2, 1]);
        }
        let mut mask = if scoped {
            vec![0; 4]
        } else {
            vec![0; if v6 { 12 } else { 8 }]
        };
        if !scoped {
            mask[0] = if v6 { 9 } else { 8 };
            mask[if v6 { 8 } else { 4 }] = 128;
        }
        h.rtm_msglen = (std::mem::size_of_val(&h) + dst.len() + gw.len() + mask.len()) as u16;
        let mut bytes = unsafe {
            std::slice::from_raw_parts(
                (&h as *const libc::rt_msghdr2).cast::<u8>(),
                std::mem::size_of_val(&h),
            )
        }
        .to_vec();
        bytes.extend(dst);
        bytes.extend(gw);
        bytes.extend(mask);
        bytes
    }
    #[test]
    fn kernel_fixtures_cover_exact_half_routes_zero_masks_scope_and_ipv6_zone() {
        for v6 in [false, true] {
            for scoped in [false, true] {
                let rows = parse(&message(v6, scoped, 7)).unwrap();
                assert_eq!(rows.len(), 1);
                let e = &rows[0];
                let dst = if v6 { "::" } else { "0.0.0.0" }.parse().unwrap();
                let mask = if scoped {
                    if v6 {
                        "::"
                    } else {
                        "0.0.0.0"
                    }
                } else if v6 {
                    "8000::"
                } else {
                    "128.0.0.0"
                }
                .parse()
                .unwrap();
                assert!(select(&rows, dst, mask, if scoped { Some(7) } else { None }).is_some());
                assert!(select(&rows, dst, mask, if scoped { Some(8) } else { Some(7) }).is_none());
                if v6 {
                    assert_eq!(e.gateway.as_deref(), Some("fe80::1%7"));
                }
            }
        }
    }
    #[test]
    fn missing_exact_prefix_is_absent_only_after_successful_complete_parse() {
        let rows = parse(&message(false, false, 7)).unwrap();
        assert!(select(
            &rows,
            "128.0.0.0".parse().unwrap(),
            "128.0.0.0".parse().unwrap(),
            None
        )
        .is_none());
        let bytes = message(true, false, 7);
        for n in 0..bytes.len() {
            if n != 0 {
                assert!(parse(&bytes[..n]).is_err());
            }
        }
    }
    #[test]
    fn blocked_legacy_child_does_not_block_the_kernel_query() {
        use std::io::Read;
        use std::os::unix::ffi::OsStrExt;
        use std::process::{Command, Stdio};
        let path = std::env::temp_dir().join(format!("pv-{}", uuid::Uuid::new_v4().simple()));
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let child = Command::new("/bin/sh")
            .args(["-c", "printf ready; exec /bin/cat \"$1\"", "sh"])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        struct Held {
            child: std::process::Child,
            path: std::path::PathBuf,
        }
        impl Drop for Held {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let _ = std::fs::remove_file(&self.path);
            }
        }
        let mut held = Held {
            child,
            path: path.clone(),
        };
        let child = &mut held.child;
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");
        assert!(child.try_wait().unwrap().is_none());
        // The legacy production algorithm waits for child settlement using
        // this unchanged post-spawn 300ms budget. The FIFO is the barrier.
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            assert!(child.try_wait().unwrap().is_none());
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(child.try_wait().unwrap().is_none());
        // The replacement production reader succeeds while that same external
        // query dependency remains blocked, without relaxing any deadline.
        query(&["-n", "get", "-inet", "-net", "0.0.0.0/1"]).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn explicit_foreign_ipv6_zone_is_rejected_and_link_gateway_is_not_numeric() {
        let mut data = message(true, true, 7);
        let h = std::mem::size_of::<libc::rt_msghdr2>();
        let gw = h + 28;
        data[gw + 24..gw + 28].copy_from_slice(&8u32.to_ne_bytes());
        assert!(parse(&data).is_err());
        let mut data = message(false, true, 7);
        data[h + 16 + 1] = libc::AF_LINK as u8;
        let rows = parse(&data).unwrap();
        assert!(rows[0].gateway.is_none());
        let rows = parse(&message(false, true, 7)).unwrap();
        assert!(select(
            &rows,
            "0.0.0.0".parse().unwrap(),
            "128.0.0.0".parse().unwrap(),
            Some(7)
        )
        .is_none());
    }
    #[test]
    fn unusable_scoped_defaults_are_errors_and_high_half_prefixes_are_exact() {
        for bad in [
            0,
            libc::RTF_UP | libc::RTF_REJECT,
            libc::RTF_UP | libc::RTF_BLACKHOLE,
        ] {
            let mut rows = parse(&message(false, true, 7)).unwrap();
            rows[0].flags = bad;
            assert!(require_usable_uplink(&rows[0]).is_err());
        }
        for v6 in [false, true] {
            let mut data = message(v6, false, 7);
            let h = std::mem::size_of::<libc::rt_msghdr2>();
            data[h + if v6 { 8 } else { 4 }] = 128;
            let rows = parse(&data).unwrap();
            let high = if v6 { "8000::" } else { "128.0.0.0" }.parse().unwrap();
            assert!(select(&rows, high, high, None).is_some());
        }
        let mut unknown = message(false, false, 7);
        unknown[std::mem::size_of::<libc::rt_msghdr2>() + 1] = 255;
        assert!(parse(&unknown).is_err());
    }
    #[test]
    fn production_query_bypasses_a_blocked_external_helper() {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir =
            Fixture(std::env::temp_dir().join(format!("pq-{}", uuid::Uuid::new_v4().simple())));
        std::fs::create_dir(&dir.0).unwrap();
        let fifo = dir.0.join("gate");
        let marker = dir.0.join("started");
        let helper = dir.0.join("query");
        let raw = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        fn quoted(path: &std::path::Path) -> String {
            format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
        }
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\nif [ \"${{1-}}\" = --warmup ]; then exit 0; fi\n: > {}\nexec /bin/cat {}\n",
                quoted(&marker),
                quoted(&fifo)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Warm the test helper's executable launch before arming its FIFO;
        // the old-query control must reach the gate, not AMFI cold assessment.
        let mut warm = {
            let _guard = ppvpn_core::capture_host::fd_spawn_guard();
            std::process::Command::new(&helper)
                .arg("--warmup")
                .spawn()
                .unwrap()
        };
        assert!(warm.wait().unwrap().success());
        let result = super::super::native::query_test(
            helper.to_str().unwrap(),
            &["-n", "get", "-inet", "-ifscope", "lo0", "-net", "default"],
        );
        if let Err(error) = &result {
            eprintln!(
                "production-route-query control_error={error} helper_started={}",
                marker.exists()
            );
        }
        result.unwrap();
        assert!(
            !marker.exists(),
            "production query must not execute the external helper"
        );
    }
    #[test]
    fn actual_kernel_snapshot_is_read_only_and_well_formed() {
        read().unwrap();
    }
    #[test]
    fn malformed_unknown_tables_are_errors_not_absence() {
        for data in [vec![0], vec![0; std::mem::size_of::<libc::rt_msghdr2>()]] {
            assert!(parse(&data).is_err());
        }
    }
}
