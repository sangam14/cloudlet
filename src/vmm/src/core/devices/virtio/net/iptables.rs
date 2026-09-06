use std::net::Ipv4Addr;

use super::xx_netmask_width;
use tracing::warn;

pub fn iptables_ip_masq(network: Ipv4Addr, netmask: Ipv4Addr, link_name: String) {
    let prefix_len = xx_netmask_width(netmask.octets());
    let source = format!("{}/{}", network, prefix_len);

    let ipt = match iptables::new(false) {
        Ok(ipt) => ipt,
        Err(error) => {
            warn!(%error, "could not initialise iptables for explicitly enabled guest egress");
            return;
        }
    };
    let rule = format!("-s {} ! -o {} -j MASQUERADE", source, link_name);

    match ipt.exists("nat", "POSTROUTING", rule.as_str()) {
        Ok(false) => {
            if let Err(error) = ipt.insert_unique("nat", "POSTROUTING", rule.as_str(), 1) {
                warn!(%error, "could not install Cloudlet guest egress rule");
            }
        }
        Ok(true) => {}
        Err(error) => warn!(%error, "could not inspect Cloudlet guest egress rule"),
    }
}
