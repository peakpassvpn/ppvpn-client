//! Fixed fixture header observer. No payload/source address is read or retained.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct Counters {
    pub offered: AtomicU64,
    pub accepted: AtomicU64,
    pub rejected: AtomicU64,
}

impl Counters {
    pub fn offer(&self, packet: &[u8]) -> bool {
        let matched = matches(packet);
        if matched {
            self.offered.fetch_add(1, Ordering::Relaxed);
        }
        matched
    }

    pub fn settle(&self, accepted: bool) {
        let counter = if accepted {
            &self.accepted
        } else {
            &self.rejected
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn matches(packet: &[u8]) -> bool {
    if packet.len() < 28 || packet[0] >> 4 != 4 {
        return false;
    }
    let ihl = usize::from(packet[0] & 15) * 4;
    if !(20..=60).contains(&ihl) || packet.len() < ihl + 8 || packet[9] != 17 {
        return false;
    }
    let total = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    // Never inspect transport fields beyond the declared IPv4 packet.
    if total != packet.len() || total < ihl + 8 {
        return false;
    }
    let fragments = u16::from_be_bytes([packet[6], packet[7]]);
    if fragments & 0x9fff != 0 || packet[16..20] != [47, 115, 203, 193] {
        return false; // reserved bit or noninitial fragment
    }
    if packet[ihl + 2..ihl + 4] != 51280_u16.to_be_bytes() {
        return false;
    }
    let udp_len = usize::from(u16::from_be_bytes([packet[ihl + 4], packet[ihl + 5]]));
    if udp_len < 8 {
        return false;
    }
    if fragments & 0x2000 != 0 {
        // Initial fragment only: remaining UDP bytes may be in later fragments.
        fragments & 0x4000 == 0 && udp_len > total - ihl && (total - ihl) % 8 == 0
    } else {
        udp_len == total - ihl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn packet(ihl: usize) -> Vec<u8> {
        let mut p = vec![0; ihl + 8];
        p[0] = 0x40 | (ihl / 4) as u8;
        let n = p.len() as u16;
        p[2..4].copy_from_slice(&n.to_be_bytes());
        p[9] = 17;
        p[16..20].copy_from_slice(&[47, 115, 203, 193]);
        p[ihl + 2..ihl + 4].copy_from_slice(&51280_u16.to_be_bytes());
        p[ihl + 4..ihl + 6].copy_from_slice(&8_u16.to_be_bytes());
        p
    }
    #[test]
    fn matches_ihl_bounds_and_initial_fragments_only() {
        for ihl in (20..=60).step_by(4) {
            assert!(matches(&packet(ihl)));
        }
        let mut p = packet(20);
        p[6] = 0x20;
        p[24..26].copy_from_slice(&16_u16.to_be_bytes());
        assert!(matches(&p));
        p[7] = 1;
        assert!(!matches(&p));
    }
    #[test]
    fn rejects_nearby_target_and_malformed_headers() {
        let original = packet(20);
        for (i, v) in [
            (0, 0x65),
            (0, 0x44),
            (0, 0x4f),
            (9, 6),
            (16, 48),
            (19, 194),
            (22, 0),
            (23, 0),
            (2, 1),
            (3, 27),
            (6, 0x80),
            (7, 1),
            (24, 1),
            (25, 7),
            (25, 9),
        ] {
            let mut p = original.clone();
            p[i] = v;
            assert!(!matches(&p), "index {i} value {v}");
        }
        for n in 0..original.len() {
            assert!(!matches(&original[..n]));
        }
        let mut p = original.clone();
        p.push(0);
        assert!(!matches(&p));
    }
    #[test]
    fn counters_only_observe_matching_headers() {
        let c = Counters::default();
        assert!(c.offer(&packet(20)));
        c.settle(true);
        assert!(c.offer(&packet(60)));
        c.settle(false);
        assert!(!c.offer(&[0; 28]));
        assert_eq!(c.offered.load(Ordering::Relaxed), 2);
        assert_eq!(c.accepted.load(Ordering::Relaxed), 1);
        assert_eq!(c.rejected.load(Ordering::Relaxed), 1);
    }
}
