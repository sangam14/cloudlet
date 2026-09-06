// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::{io, mem, net, os::fd::FromRawFd};

use virtio_bindings::virtio_net::virtio_net_hdr_v1;

pub(crate) mod mac;
pub(crate) mod net_gen;
pub(crate) mod open_tap;
pub(crate) mod tap;

#[derive(Debug)]
pub enum Error {
    CreateSocket(io::Error),
}

/// Create a sockaddr_in from an IPv4 address, and expose it as
/// an opaque sockaddr suitable for usage by socket ioctls.
fn create_sockaddr(ip_addr: net::Ipv4Addr) -> net_gen::sockaddr {
    // IPv4 addresses big-endian (network order), but Ipv4Addr will give us
    // a view of those bytes directly so we can avoid any endian trickiness.
    let addr_in = net_gen::sockaddr_in {
        sin_family: net_gen::AF_INET as u16,
        sin_port: 0,
        // SAFETY: ip_addr can be safely transmute to in_addr
        sin_addr: unsafe { mem::transmute::<[u8; 4], net_gen::inn::in_addr>(ip_addr.octets()) },
        __pad: [0; 8usize],
    };

    // SAFETY: addr_in can be safely transmute to sockaddr
    unsafe { mem::transmute(addr_in) }
}

fn create_inet_socket() -> Result<net::UdpSocket, Error> {
    // SAFETY: we check the return value.
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return Err(Error::CreateSocket(io::Error::last_os_error()));
    }

    // SAFETY: nothing else will use or hold onto the raw sock fd.
    Ok(unsafe { net::UdpSocket::from_raw_fd(sock) })
}

fn create_unix_socket() -> Result<net::UdpSocket, Error> {
    // SAFETY: we check the return value.
    let sock = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        return Err(Error::CreateSocket(io::Error::last_os_error()));
    }

    // SAFETY: nothing else will use or hold onto the raw sock fd.
    Ok(unsafe { net::UdpSocket::from_raw_fd(sock) })
}

fn vnet_hdr_len() -> usize {
    std::mem::size_of::<virtio_net_hdr_v1>()
}
