//! IP addresses and networks (CIDR): the range a network covers, its mask,
//! how many addresses it has, and what kind of address it is.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use anyhow::{Context as _, Result, bail};

use crate::Input;

/// A network: an address and how many of its leading bits are fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    pub address: IpAddr,
    pub prefix: u8,
}

/// Reads `10.0.0.0/24`, `fd00::/8`, an address alone (a network of one),
/// or an IPv4 address with its mask (`192.168.1.0 255.255.255.0`).
pub fn read(text: &str) -> Result<Network> {
    let text = text.trim();
    let (address, prefix) = match text.split_once('/') {
        Some((address, prefix)) => (address.trim(), Some(prefix.trim())),
        None => match text.split_once(char::is_whitespace) {
            Some((address, mask)) => (address.trim(), Some(mask.trim())),
            None => (text, None),
        },
    };
    let address: IpAddr = address
        .parse()
        .with_context(|| format!("`{address}` is not an IP address"))?;
    let most = if address.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        None => most,
        Some(prefix) => match prefix.parse::<u8>() {
            Ok(prefix) => prefix,
            Err(_) => {
                let mask: Ipv4Addr = prefix
                    .parse()
                    .with_context(|| format!("`{prefix}` is not a prefix length or a mask"))?;
                let bits = u32::from(mask);
                if bits.leading_ones() + bits.trailing_zeros() != 32 {
                    bail!("{mask} is not a mask: its ones are not all at the start");
                }
                bits.leading_ones() as u8
            }
        },
    };
    if prefix > most {
        bail!(
            "An IPv{} prefix is at most /{most}",
            if most == 32 { 4 } else { 6 }
        );
    }
    Ok(Network { address, prefix })
}

impl Network {
    fn bits(self) -> u32 {
        if self.address.is_ipv4() { 32 } else { 128 }
    }

    fn mask(self) -> u128 {
        let bits = self.bits();
        if self.prefix == 0 {
            0
        } else {
            (u128::MAX << (128 - self.prefix as u32)) >> (128 - bits)
        }
    }

    fn value(self) -> u128 {
        match self.address {
            IpAddr::V4(address) => u32::from(address) as u128,
            IpAddr::V6(address) => u128::from(address),
        }
    }

    fn at(self, value: u128) -> IpAddr {
        match self.address {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from(value as u32)),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(value)),
        }
    }

    pub fn first(self) -> IpAddr {
        self.at(self.value() & self.mask())
    }

    pub fn last(self) -> IpAddr {
        let all = if self.bits() == 32 {
            u32::MAX as u128
        } else {
            u128::MAX
        };
        self.at((self.value() & self.mask()) | (!self.mask() & all))
    }

    /// Addresses in the network, as text (an IPv6 one can be 2^128).
    pub fn size(self) -> String {
        let free = self.bits() - self.prefix as u32;
        if free >= 64 {
            format!("2^{free}")
        } else {
            group((1u128 << free).to_string())
        }
    }

    pub fn contains(self, address: IpAddr) -> bool {
        let value = match (self.address, address) {
            (IpAddr::V4(_), IpAddr::V4(address)) => u32::from(address) as u128,
            (IpAddr::V6(_), IpAddr::V6(address)) => u128::from(address),
            _ => return false,
        };
        value & self.mask() == self.value() & self.mask()
    }
}

fn group(digits: String) -> String {
    let mut out = String::new();
    for (ix, c) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What kind of address it is: private, loopback, link-local…
pub fn kind(address: IpAddr) -> &'static str {
    match address {
        IpAddr::V4(address) => {
            let [a, b, ..] = address.octets();
            if address.is_loopback() {
                "loopback (this computer)"
            } else if address.is_private() {
                "private (a home or office network)"
            } else if a == 100 && (64..128).contains(&b) {
                "shared (carrier-grade NAT)"
            } else if address.is_link_local() {
                "link-local (no DHCP server answered)"
            } else if address.is_multicast() {
                "multicast"
            } else if address.is_documentation() {
                "for documentation"
            } else if address.is_unspecified() {
                "unspecified (any address)"
            } else if address.is_broadcast() {
                "broadcast"
            } else {
                "public"
            }
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            if address.is_loopback() {
                "loopback (this computer)"
            } else if address.is_unspecified() {
                "unspecified (any address)"
            } else if first & 0xfe00 == 0xfc00 {
                "unique local (private)"
            } else if first & 0xffc0 == 0xfe80 {
                "link-local"
            } else if address.is_multicast() {
                "multicast"
            } else if first == 0x2001 && address.segments()[1] == 0x0db8 {
                "for documentation"
            } else if address.to_ipv4_mapped().is_some() {
                "an IPv4 address written as IPv6"
            } else {
                "public"
            }
        }
    }
}

/// What a network covers, line by line, by name.
pub fn facts(network: Network) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("Network", format!("{}/{}", network.first(), network.prefix)),
        ("First", network.first().to_string()),
        ("Last", network.last().to_string()),
        ("Addresses", network.size()),
    ];
    if network.address.is_ipv4() {
        let mask = Ipv4Addr::from(network.mask() as u32);
        let wildcard = Ipv4Addr::from(!(network.mask() as u32));
        out.insert(1, ("Mask", mask.to_string()));
        out.push(("Wildcard", wildcard.to_string()));
        if network.prefix <= 30 {
            let first = network.value() & network.mask();
            out.push((
                "Hosts",
                format!(
                    "{} to {} ({} usable)",
                    network.at(first + 1),
                    network.at(first + (1u128 << (32 - network.prefix as u32)) - 2),
                    group(((1u128 << (32 - network.prefix as u32)) - 2).to_string())
                ),
            ));
        }
    }
    out.push(("Kind", kind(network.address).to_string()));
    out
}

/// What a network covers; with an address as the second input, whether it
/// is in it.
pub fn ip(input: &Input) -> Result<String> {
    let network = read(&input.text)?;
    let mut lines: Vec<String> = facts(network)
        .into_iter()
        .map(|(name, text)| format!("{name}: {text}"))
        .collect();
    let other = input.second.trim();
    if !other.is_empty() {
        let address: IpAddr = other
            .parse()
            .with_context(|| format!("`{other}` is not an IP address"))?;
        lines.push(format!(
            "{address} is {}in it.",
            if network.contains(address) {
                ""
            } else {
                "not "
            }
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_networks() {
        let network = read("192.168.1.77/24").unwrap();
        assert_eq!(network.first().to_string(), "192.168.1.0");
        assert_eq!(network.last().to_string(), "192.168.1.255");
        assert_eq!(network.size(), "256");
        assert_eq!(read("10.0.0.0 255.255.0.0").unwrap().prefix, 16);
        assert!(read("10.0.0.0 255.0.255.0").is_err());
        assert!(read("10.0.0.0/33").is_err());
        assert!(network.contains("192.168.1.200".parse().unwrap()));
        assert!(!network.contains("192.168.2.1".parse().unwrap()));
        let six = read("fd00::/8").unwrap();
        assert_eq!(six.size(), "2^120");
        assert_eq!(kind(six.address), "unique local (private)");
        let hosts = facts(read("10.1.2.3/30").unwrap());
        assert!(
            hosts
                .iter()
                .any(|(name, text)| *name == "Hosts" && text == "10.1.2.1 to 10.1.2.2 (2 usable)")
        );
        assert_eq!(read("8.8.8.8").unwrap().size(), "1");
        assert!(
            ip(&Input {
                second: "10.0.0.6".into(),
                ..Input::new("10.0.0.0/29")
            })
            .unwrap()
            .ends_with("is in it.")
        );
        assert!(
            ip(&Input {
                second: "10.0.0.9".into(),
                ..Input::new("10.0.0.0/29")
            })
            .unwrap()
            .ends_with("is not in it.")
        );
    }
}
