//! What the core needs to know of a binary rule set (`.srs`, sing-box's
//! format): that sail can read it, and whether it may be mirrored into DNS
//! rules. A minimal reader after sing-box's `common/srs` and sail's
//! `app/router/rule_set/srs.rs`: it walks the items without building
//! matchers.
//!
//! `SRS`, a version byte, then zlib-compressed rules: a uvarint count, each
//! rule a type byte (0 plain, 1 logical) and its items.

/// The newest version sail reads (sing-box 1.14's; Go's 1.13 read 4).
const MAX_VERSION: u8 = 5;
/// The most a rule set may inflate to (sail's limit).
const MAX_INFLATED: usize = 64 << 20;
const MAX_DEPTH: usize = 100;

// Item types, as sing-box numbers them.
const QUERY_TYPE: u8 = 0;
const NETWORK: u8 = 1;
const DOMAIN: u8 = 2;
const DOMAIN_KEYWORD: u8 = 3;
const DOMAIN_REGEX: u8 = 4;
const SOURCE_IP_CIDR: u8 = 5;
const IP_CIDR: u8 = 6;
const SOURCE_PORT: u8 = 7;
const SOURCE_PORT_RANGE: u8 = 8;
const PORT: u8 = 9;
const PORT_RANGE: u8 = 10;
const PROCESS_NAME: u8 = 11;
const PROCESS_PATH: u8 = 12;
const PACKAGE_NAME: u8 = 13;
const WIFI_SSID: u8 = 14;
const WIFI_BSSID: u8 = 15;
const ADGUARD_DOMAIN: u8 = 16;
const PROCESS_PATH_REGEX: u8 = 17;
const NETWORK_TYPE: u8 = 18;
const NETWORK_IS_EXPENSIVE: u8 = 19;
const NETWORK_IS_CONSTRAINED: u8 = 20;
const NETWORK_INTERFACE_ADDRESS: u8 = 21;
const DEFAULT_INTERFACE_ADDRESS: u8 = 22;
const PACKAGE_NAME_REGEX: u8 = 23;
const FINAL: u8 = 0xff;

/// Reads a binary rule set and reports whether it may be mirrored into DNS
/// rules: it matches domains and no destination IP CIDRs. A set sail cannot
/// read is an error, so that it is never handed to the kernel.
pub(super) fn inspect(data: &[u8]) -> Result<bool, String> {
    let rest = data
        .strip_prefix(b"SRS".as_slice())
        .ok_or("not a binary rule set")?;
    let (&version, compressed) = rest.split_first().ok_or("no version")?;
    if version > MAX_VERSION {
        return Err(format!("unsupported version {version}"));
    }
    let inflated =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(compressed, MAX_INFLATED)
            .map_err(|e| format!("inflate: {:?}", e.status))?;
    let mut reader = Reader(&inflated);
    let mut found = Found::default();
    for _ in 0..reader.count(1)? {
        rule(&mut reader, 0, &mut found)?;
    }
    Ok(found.domains && !found.cidrs)
}

#[derive(Default)]
struct Found {
    domains: bool,
    cidrs: bool,
}

fn rule(reader: &mut Reader, depth: usize, found: &mut Found) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("logical rules nested too deep".into());
    }
    match reader.u8()? {
        0 => plain(reader, found),
        1 => {
            let mode = reader.u8()?;
            if mode > 1 {
                return Err(format!("unknown logical mode {mode}"));
            }
            for _ in 0..reader.count(1)? {
                rule(reader, depth + 1, found)?;
            }
            reader.u8()?; // invert
            Ok(())
        }
        kind => Err(format!("unknown rule type {kind}")),
    }
}

fn plain(reader: &mut Reader, found: &mut Found) -> Result<(), String> {
    loop {
        match reader.u8()? {
            QUERY_TYPE | SOURCE_PORT | PORT => {
                let count = reader.count(2)?;
                reader.take(count * 2)?;
            }
            NETWORK | SOURCE_PORT_RANGE | PORT_RANGE | PROCESS_NAME | PROCESS_PATH
            | PACKAGE_NAME | WIFI_SSID | WIFI_BSSID | PROCESS_PATH_REGEX | PACKAGE_NAME_REGEX => {
                reader.strings()?;
            }
            DOMAIN => {
                reader.succinct()?;
                found.domains = true;
            }
            DOMAIN_KEYWORD | DOMAIN_REGEX => {
                if reader.strings()? > 0 {
                    found.domains = true;
                }
            }
            SOURCE_IP_CIDR => reader.ip_set()?,
            IP_CIDR => {
                reader.ip_set()?;
                found.cidrs = true;
            }
            NETWORK_TYPE => {
                // wifi, cellular, ethernet, other
                if reader.bytes()?.iter().any(|&t| t > 3) {
                    return Err("unknown network type".into());
                }
            }
            NETWORK_IS_EXPENSIVE | NETWORK_IS_CONSTRAINED => {}
            FINAL => {
                reader.u8()?; // invert
                return Ok(());
            }
            ADGUARD_DOMAIN | NETWORK_INTERFACE_ADDRESS | DEFAULT_INTERFACE_ADDRESS => {
                return Err("an item sail does not match yet".into());
            }
            item => return Err(format!("unknown item type {item}")),
        }
    }
}

/// sing's `varbin`: uvarint counts, big-endian integers.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("unexpected end of data".into());
        }
        let (taken, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn uvarint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("uvarint overflows".into())
    }

    /// A count of items of `item_len` bytes at least, which must fit in
    /// what is left.
    fn count(&mut self, item_len: usize) -> Result<usize, String> {
        let count = self.uvarint()?;
        if count > (self.0.len() / item_len.max(1)) as u64 {
            return Err(format!("count {count} past the end of data"));
        }
        Ok(count as usize)
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = self.count(1)?;
        self.take(len)
    }

    /// A list of UTF-8 strings; returns how many.
    fn strings(&mut self) -> Result<usize, String> {
        let count = self.count(1)?;
        for _ in 0..count {
            std::str::from_utf8(self.bytes()?).map_err(|_| "invalid utf-8")?;
        }
        Ok(count)
    }

    fn u64s(&mut self) -> Result<Vec<u64>, String> {
        let count = self.count(8)?;
        let (words, _) = self.take(count * 8)?.as_chunks::<8>();
        Ok(words.iter().map(|b| u64::from_be_bytes(*b)).collect())
    }

    /// The domain matcher: a version byte, the leaves, the label bitmap and
    /// the labels of a succinct trie, checked for shape as sail does.
    fn succinct(&mut self) -> Result<(), String> {
        self.u8()?;
        let leaves = self.count(8)?;
        self.take(leaves * 8)?;
        let bitmap = self.u64s()?;
        let labels = self.bytes()?.len();
        let ones: usize = bitmap.iter().map(|w| w.count_ones() as usize).sum();
        let last_one = bitmap
            .iter()
            .enumerate()
            .rev()
            .find(|(_, w)| **w != 0)
            .map(|(i, w)| (i << 6) | (63 - w.leading_zeros() as usize));
        let zeros = last_one.map_or(0, |last| last + 1 - ones);
        if last_one.is_none() || ones != zeros + 1 || labels != zeros {
            return Err("malformed domain set".into());
        }
        Ok(())
    }

    /// An address set: version 1, a big-endian u64 count, then each range's
    /// first and last address (a length, 4 or 16, and the bytes).
    fn ip_set(&mut self) -> Result<(), String> {
        if self.u8()? != 1 {
            return Err("unknown address set version".into());
        }
        let count = u64::from_be_bytes(self.take(8)?.try_into().expect("8 bytes"));
        for _ in 0..count {
            for _ in 0..2 {
                match self.uvarint()? {
                    len @ (4 | 16) => {
                        self.take(len as usize)?;
                    }
                    len => return Err(format!("invalid address length {len}")),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/rulesets/testdata")
                .join(name),
        )
        .unwrap()
    }

    /// Every item a version 4 set can hold, and a logical rule.
    #[test]
    fn reads_every_item_and_logical_rules() {
        // A CIDR inside a logical rule counts.
        assert_eq!(inspect(&fixture("logical.srs")), Ok(false));
    }

    #[test]
    fn versions_up_to_sails_are_read() {
        let mut data = fixture("domains.srs");
        data[3] = MAX_VERSION;
        assert_eq!(inspect(&data), Ok(true));
        data[3] = MAX_VERSION + 1;
        assert!(inspect(&data).is_err());
    }

    /// AdGuard items load in Go's sing-box but not in sail: never ready.
    #[test]
    fn items_sail_cannot_match_are_invalid() {
        assert!(inspect(&fixture("adguard.srs")).is_err());
    }

    #[test]
    fn damaged_sets_are_invalid() {
        let data = fixture("mixed.srs");
        assert!(inspect(&data[..data.len() - 6]).is_err());
        assert!(inspect(b"not a rule set").is_err());
        assert!(inspect(b"SRS").is_err());
        let mut flipped = data.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xff;
        assert!(inspect(&flipped).is_err());
    }
}
