//! Cellular data as the pages show it: a switch, an interface the product
//! already named by prefix, and an address the modem already holds or DHCP
//! then gives.

use crate::dhcp::Lease;
use crate::reach::Reach;
use crate::wlan::Power;
use std::net::Ipv4Addr;

/// Prefixes tried when the device table names none of its own.
pub const DEFAULT_PREFIXES: [&str; 6] = ["rmnet_data", "rmnet_ipa", "rmnet", "ccmni", "wwan", "mbim"];

/// Names that are never a cellular datapath, even when a prefix would match.
const SKIP: [&str; 10] = ["lo", "dummy", "sit", "ip6tnl", "tun", "wlan", "p2p", "ap", "rndis", "usb"];

/// Cellular data, as the pages show it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mobile {
    pub power: Power,
    pub interface: Option<String>,
    pub address: Option<Ipv4Addr>,
    pub mask: Option<Ipv4Addr>,
    pub router: Option<Ipv4Addr>,
    pub reach: Option<Reach>,
}

impl Mobile {
    /// Take the address a DHCP lease gave, keeping the interface already shown.
    pub fn from_lease(lease: &Lease, interface: String) -> Mobile {
        Mobile {
            power: Power::On,
            interface: Some(interface),
            address: Some(lease.address),
            mask: Some(lease.mask),
            router: lease.router,
            reach: None,
        }
    }
}

/// The first interface whose name starts with one of `prefixes`, skipping
/// loopbacks, Wi-Fi, and USB gadgets.
pub fn pick_interface<'a, I>(names: I, prefixes: &[&str]) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let prefixes = if prefixes.is_empty() { &DEFAULT_PREFIXES[..] } else { prefixes };
    let mut names: Vec<&str> = names.into_iter().collect();
    names.sort_unstable();
    for prefix in prefixes {
        for name in &names {
            if skipped(name) {
                continue;
            }
            if name == prefix || name.starts_with(&format!("{prefix}_")) || name.starts_with(prefix) {
                return Some((*name).to_owned());
            }
        }
    }
    None
}

fn skipped(name: &str) -> bool {
    SKIP.iter().any(|skip| name == *skip || name.starts_with(&format!("{skip}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualcomm_data_call_wins_over_wlan_and_loopback() {
        let names = ["lo", "wlan0", "rmnet_ipa0", "rmnet_data1", "dummy0"];
        assert_eq!(pick_interface(names, &DEFAULT_PREFIXES).as_deref(), Some("rmnet_data1"));
    }

    #[test]
    fn a_mediatek_call_is_found_without_a_board_toml() {
        let names = ["lo", "wlan0", "ccmni0"];
        assert_eq!(pick_interface(names, &[]).as_deref(), Some("ccmni0"));
    }

    #[test]
    fn usb_tether_is_not_taken_for_cellular() {
        assert_eq!(pick_interface(["lo", "usb0", "rndis0"], &DEFAULT_PREFIXES), None);
    }
}
