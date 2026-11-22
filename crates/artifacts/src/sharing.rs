//! Sharing an artifact with people on the same network: the link they are
//! given, and who may come in by it.
//!
//! An artifact answers on this computer only, until its owner shares it.
//! Then a second address opens on the network, for as long as the app runs
//! and the owner leaves it on. Whoever comes there is a guest, and needs
//! the key the link carries: a first visit with the key in its address
//! leaves it in a cookie and moves on to the plain address, so the key
//! does not stay on show in the address bar.
//!
//! A guest is given the page's files, the shared storage and the room.
//! What the owner stored for themselves is never given to a guest.

use std::net::Ipv4Addr;

/// The name the key travels under in the link: `/?invite=<key>`.
pub const INVITE: &str = "invite";

/// How an artifact is shared.
#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    /// What the link carries; without it nobody comes in.
    pub key: String,
    /// Whether guests may send to the room and write what is shared, or
    /// only look.
    pub take_part: bool,
}

/// A key nobody guesses: 130 bits from the system, in 26 letters and
/// digits that read the same in every font.
pub fn new_key() -> anyhow::Result<String> {
    const LETTERS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 26];
    getrandom::fill(&mut bytes)
        .map_err(|err| anyhow::anyhow!("the system gave no random numbers: {err}"))?;
    Ok(bytes
        .iter()
        .map(|byte| LETTERS[(byte & 31) as usize] as char)
        .collect())
}

/// Whether `offered` is the key, taking the same time whatever is wrong
/// with it, so the time an answer takes tells a guesser nothing.
pub fn opens(share: &Share, offered: &str) -> bool {
    let (key, offered) = (share.key.as_bytes(), offered.as_bytes());
    let mut different = key.len() ^ offered.len();
    for (ix, byte) in key.iter().enumerate() {
        different |= usize::from(byte ^ offered.get(ix).copied().unwrap_or(0));
    }
    different == 0 && !key.is_empty()
}

/// The cookie an artifact's key is kept in. Named for the artifact: a
/// browser sends a cookie to every port of a computer, and two artifacts
/// shared from one have two keys.
pub fn cookie_name(id: &str) -> String {
    format!("artifact_{id}")
}

/// The value of cookie `name` in a `Cookie` header.
pub fn cookie<'a>(header: Option<&'a str>, name: &str) -> Option<&'a str> {
    header?.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

/// The link to give: this computer's address on the network, the port the
/// artifact is shared on, and the key.
pub fn link(address: Ipv4Addr, port: u16, key: &str) -> String {
    format!("http://{address}:{port}/?{INVITE}={key}")
}

/// One of this computer's addresses, and what tells whether colleagues on
/// the same network reach it.
#[derive(Debug, Clone, PartialEq)]
struct Reachable {
    /// The interface's name (`en0`, `eth0`, `utun3`, `Wi-Fi`).
    interface: String,
    address: Ipv4Addr,
    /// A line to one other end (a VPN's tunnel), not a network of many.
    point_to_point: bool,
}

/// Where this computer stands on the network, for sharing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Network {
    /// Its address on the network its colleagues are on, if it is on one:
    /// what a shared artifact's link carries.
    pub address: Option<Ipv4Addr>,
    /// A VPN's tunnel is up. Many keep the computer off its own office
    /// network meanwhile, so that nobody there reaches it: worth a word
    /// where a link is given.
    pub tunnel: bool,
}

/// Reads [`Network`] from the interfaces themselves. Asking which address
/// the default route leaves from names a VPN's tunnel on every computer
/// that has one up, an address nobody in the room reaches.
pub fn network() -> Network {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Network::default();
    };
    let found: Vec<Reachable> = interfaces
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
        .filter_map(|interface| match &interface.addr {
            if_addrs::IfAddr::V4(v4) => Some(Reachable {
                address: v4.ip,
                point_to_point: interface.is_p2p(),
                interface: interface.name,
            }),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect();
    Network {
        address: pick(&found),
        tunnel: found
            .iter()
            .any(|reachable| reachable.point_to_point && !reachable.address.is_link_local()),
    }
}

/// The address to give, of those `found`: on a network of many rather than
/// at the end of a tunnel, of a real card rather than one a program made,
/// and in a private range, where an office's addresses are. A tunnel's
/// address only when there is no other (it is still better than no link:
/// those on the same VPN reach it).
fn pick(found: &[Reachable]) -> Option<Ipv4Addr> {
    // What programs make for themselves: containers, virtual machines,
    // VPNs, and Apple's links between its own devices.
    const MADE: &[&str] = &[
        "docker",
        "veth",
        "br-",
        "virbr",
        "vmnet",
        "vboxnet",
        "bridge",
        "utun",
        "tun",
        "tap",
        "wg",
        "tailscale",
        "zt",
        "ipsec",
        "ppp",
        "awdl",
        "llw",
        "anpi",
        "vethernet",
        "wsl",
        "hyper-v",
    ];
    found
        .iter()
        // Nobody else reaches an address a computer gave itself.
        .filter(|reachable| !reachable.address.is_link_local() && !reachable.address.is_loopback())
        .min_by_key(|reachable| {
            let name = reachable.interface.to_lowercase();
            (
                reachable.point_to_point,
                MADE.iter().any(|made| name.contains(made)),
                !reachable.address.is_private(),
            )
        })
        .map(|reachable| reachable.address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_long_and_its_own() {
        let (one, two) = (new_key().unwrap(), new_key().unwrap());
        assert_eq!(one.len(), 26);
        assert_ne!(one, two);
        assert!(
            one.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
    }

    #[test]
    fn only_the_whole_key_opens() {
        let share = Share {
            key: "abc234".into(),
            take_part: true,
        };
        assert!(opens(&share, "abc234"));
        for wrong in ["", "abc23", "abc2345", "abc235", "ABC234"] {
            assert!(!opens(&share, wrong), "{wrong}");
        }
        let unset = Share {
            key: String::new(),
            take_part: true,
        };
        assert!(!opens(&unset, ""), "no key opens nothing");
    }

    #[test]
    fn the_link_carries_the_office_networks_address_not_a_tunnels() {
        let at = |interface: &str, address: [u8; 4], point_to_point: bool| Reachable {
            interface: interface.into(),
            address: Ipv4Addr::from(address),
            point_to_point,
        };
        // A laptop on Wi-Fi with a VPN up and containers running.
        let laptop = [
            at("ipsec0", [10, 6, 2, 32], true),
            at("docker0", [172, 17, 0, 1], false),
            at("en0", [192, 168, 1, 6], false),
            at("awdl0", [169, 254, 20, 4], false),
        ];
        assert_eq!(pick(&laptop), Some(Ipv4Addr::new(192, 168, 1, 6)));
        // Windows names its cards in words, and its virtual ones too.
        let desktop = [
            at("vEthernet (WSL)", [172, 28, 16, 1], false),
            at("Ethernet", [10, 0, 0, 14], false),
        ];
        assert_eq!(pick(&desktop), Some(Ipv4Addr::new(10, 0, 0, 14)));
        // Only a tunnel: better than no link at all.
        assert_eq!(
            pick(&[at("utun4", [100, 64, 0, 9], true)]),
            Some(Ipv4Addr::new(100, 64, 0, 9))
        );
        // On no network: nothing to give.
        assert_eq!(pick(&[at("en0", [169, 254, 7, 7], false)]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn the_key_is_read_from_its_own_cookie() {
        let header = Some("theme=dark; artifact_ab12=k3y; artifact_cd34=other");
        assert_eq!(cookie(header, &cookie_name("ab12")), Some("k3y"));
        assert_eq!(cookie(header, &cookie_name("zz")), None);
        assert_eq!(cookie(None, "artifact_ab12"), None);
        assert_eq!(
            link(Ipv4Addr::new(192, 168, 1, 20), 23_105, "k3y"),
            "http://192.168.1.20:23105/?invite=k3y"
        );
    }
}
