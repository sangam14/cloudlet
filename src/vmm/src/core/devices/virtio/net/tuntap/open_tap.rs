// Copyright (c) 2020 Intel Corporation. All rights reserved.
//
// SPDX-License-Identifier: Apache-2.0 AND BSD-3-Clause

use super::mac::MacAddr;
use super::tap::Tap;
use super::{tap, vnet_hdr_len};
use std::io;
use std::net::Ipv4Addr;
use std::path::Path;
use tracing::warn;

#[derive(Debug)]
pub enum Error {
    ConvertHexStringToInt(std::num::ParseIntError),
    MultiQueueNoTapSupport,
    MultiQueueNoDeviceSupport,
    ReadSysfsTunFlags(io::Error),
    TapOpen(tap::Error),
    TapSetIp(tap::Error),
    TapSetNetmask(tap::Error),
    TapSetMac(tap::Error),
    TapGetMac(tap::Error),
    TapSetVnetHdrSize(tap::Error),
    TapSetMtu(tap::Error),
    TapEnable(tap::Error),
}

/// Create a new virtio network device with the given IP address and
/// netmask.
pub fn open_tap(
    if_name: Option<&str>,
    ip_addr: Option<Ipv4Addr>,
    netmask: Option<Ipv4Addr>,
    host_mac: &mut Option<MacAddr>,
    mtu: Option<u16>,
    flags: Option<i32>,
) -> Result<Tap, Error> {
    let vnet_hdr_size = vnet_hdr_len() as i32;
    // Check if the given interface exists before we create it.
    let tap_existed = if_name.is_some_and(|n| Path::new(&format!("/sys/class/net/{n}")).exists());

    let tap: Tap = match if_name {
        Some(name) => Tap::open_named(name, 1, flags).map_err(Error::TapOpen)?,
        None => Tap::new(1).map_err(Error::TapOpen)?,
    };
    // Don't overwrite ip configuration of existing interfaces:
    if !tap_existed {
        if let Some(ip) = ip_addr {
            tap.set_ip_addr(ip).map_err(Error::TapSetIp)?;
        }
        if let Some(mask) = netmask {
            tap.set_netmask(mask).map_err(Error::TapSetNetmask)?;
        }
    } else {
        warn!(
            "Tap {} already exists. IP configuration will not be overwritten.",
            if_name.unwrap_or_default()
        );
    }
    if let Some(mac) = host_mac {
        tap.set_mac_addr(*mac).map_err(Error::TapSetMac)?
    } else {
        *host_mac = Some(tap.get_mac_addr().map_err(Error::TapGetMac)?)
    }
    if let Some(mtu) = mtu {
        tap.set_mtu(mtu as i32).map_err(Error::TapSetMtu)?;
    }
    tap.enable().map_err(Error::TapEnable)?;

    tap.set_vnet_hdr_size(vnet_hdr_size)
        .map_err(Error::TapSetVnetHdrSize)?;

    Ok(tap)
}
