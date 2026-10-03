//! Windows: the Tcpip6 DisabledComponents policy and the IPv6 adapters
//! (GetAdaptersAddresses).

use std::net::Ipv6Addr;

use super::global_unicast;

/// The DisabledComponents bit that disables IPv6 on every non-tunnel
/// interface, Wintun included (0xFF sets it).
pub(crate) const DISABLED_COMPONENTS_NON_TUNNEL: u32 = 0x10;

/// One IPv6 adapter, as far as `route` goes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Adapter {
    pub up: bool,
    /// It has an IPv6 default gateway.
    pub gateway: bool,
    pub addrs: Vec<Ipv6Addr>,
}

/// `disabled`: the DisabledComponents policy, None when unset (the default:
/// IPv6 enabled); `has_adapter`: AF_INET6 lists an adapter at all (with
/// the protocol uninstalled or unbound everywhere it lists none).
pub(crate) fn available_from(disabled: Option<u32>, has_adapter: bool) -> bool {
    if disabled.is_some_and(|value| value & DISABLED_COMPONENTS_NON_TUNNEL != 0) {
        return false;
    }
    has_adapter
}

/// One adapter is up with a global unicast IPv6 address and an IPv6 default
/// gateway. Wintun with our ULA never qualifies.
pub(crate) fn route_from(adapters: &[Adapter]) -> bool {
    adapters
        .iter()
        .any(|a| a.up && a.gateway && a.addrs.iter().any(global_unicast))
}

#[cfg(windows)]
pub(crate) fn available() -> bool {
    // Any failure but "no adapter" keeps IPv6, so a leak stays impossible
    // and a stack really missing fails the start visibly.
    let has_adapter = !matches!(sys::adapters(false), Ok(ref a) if a.is_empty());
    available_from(sys::disabled_components(), has_adapter)
}

#[cfg(windows)]
pub(crate) fn route() -> Result<bool, String> {
    Ok(route_from(&sys::adapters(true)?))
}

#[cfg(windows)]
mod sys {
    use std::net::Ipv6Addr;

    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST,
        GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET6, SOCKADDR_IN6};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
    };

    use super::Adapter;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The Tcpip6 DisabledComponents policy; None when unset.
    pub(super) fn disabled_components() -> Option<u32> {
        let key = wide(r"SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters");
        let name = wide("DisabledComponents");
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: NUL-terminated names; value holds size bytes.
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        (rc == ERROR_SUCCESS).then_some(value)
    }

    /// The IPv6 adapters, with their unicast addresses and (`gateways`)
    /// whether they have a default gateway; empty when there is none.
    pub(super) fn adapters(gateways: bool) -> Result<Vec<Adapter>, String> {
        let mut flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        if gateways {
            flags |= GAA_FLAG_INCLUDE_GATEWAYS;
        }
        let mut size: u32 = 16 * 1024;
        for _ in 0..3 {
            // u64 words keep the buffer aligned for the structures.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            // SAFETY: buffer holds size bytes, aligned.
            let rc = unsafe {
                GetAdaptersAddresses(AF_INET6 as u32, flags, std::ptr::null(), first, &mut size)
            };
            match rc {
                ERROR_NO_DATA => return Ok(Vec::new()),
                ERROR_BUFFER_OVERFLOW => continue,
                ERROR_SUCCESS => {}
                other => return Err(format!("GetAdaptersAddresses: error {other}")),
            }
            let mut out = Vec::new();
            let mut cur = first;
            while !cur.is_null() {
                // SAFETY: a node of the list the call wrote into buffer.
                let a = unsafe { &*cur };
                let mut adapter = Adapter {
                    up: a.OperStatus == IfOperStatusUp,
                    gateway: !a.FirstGatewayAddress.is_null(),
                    addrs: Vec::new(),
                };
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    // SAFETY: a node of the unicast list, in buffer.
                    let unicast = unsafe { &*u };
                    let sa = unicast.Address.lpSockaddr;
                    // SAFETY: a non-null sockaddr of the list.
                    if !sa.is_null() && unsafe { (*sa).sa_family } == AF_INET6 {
                        // SAFETY: an AF_INET6 sockaddr is a SOCKADDR_IN6.
                        let sin6 = unsafe { &*(sa as *const SOCKADDR_IN6) };
                        // SAFETY: the address as bytes.
                        adapter
                            .addrs
                            .push(Ipv6Addr::from(unsafe { sin6.sin6_addr.u.Byte }));
                    }
                    u = unicast.Next;
                }
                out.push(adapter);
                cur = a.Next;
            }
            return Ok(out);
        }
        Err("GetAdaptersAddresses: the buffer kept being too small".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: TestWindowsAvailability.
    #[test]
    fn availability() {
        for (name, disabled, adapters, want) in [
            ("default", None, true, true),
            ("prefer ipv4 only", Some(0x20), true, true),
            ("tunnel interfaces only", Some(0x01), true, true),
            ("non-tunnel disabled", Some(0x10), true, false),
            ("all disabled", Some(0xFF), true, false),
            ("stack missing", None, false, false),
        ] {
            assert_eq!(available_from(disabled, adapters), want, "{name}");
        }
    }

    // Go: TestWindowsRoute.
    #[test]
    fn route_needs_an_up_adapter_with_a_global_address_and_a_gateway() {
        let global: Ipv6Addr = "2001:db8::50".parse().unwrap();
        let link_local: Ipv6Addr = "fe80::1".parse().unwrap();
        let ula: Ipv6Addr = "fde2:ec40:9312:c7fd::1".parse().unwrap();
        let adapter = |up, gateway, addrs: &[Ipv6Addr]| Adapter {
            up,
            gateway,
            addrs: addrs.to_vec(),
        };
        for (name, adapters, want) in [
            (
                "ethernet with global address and gateway",
                vec![adapter(true, true, &[link_local, global])],
                true,
            ),
            (
                "link-local only, Wintun ULA",
                vec![
                    adapter(true, false, &[link_local]),
                    adapter(true, true, &[ula]),
                ],
                false,
            ),
            (
                "global address without gateway",
                vec![adapter(true, false, &[global])],
                false,
            ),
            ("adapter down", vec![adapter(false, true, &[global])], false),
            ("no adapters", vec![], false),
        ] {
            assert_eq!(route_from(&adapters), want, "{name}");
        }
    }
}
