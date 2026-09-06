// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::fmt;
use std::io;
use std::str::FromStr;

pub const MAC_ADDR_LEN: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacAddr {
    bytes: [u8; MAC_ADDR_LEN],
}

impl MacAddr {
    pub fn parse_str<S>(s: &S) -> Result<MacAddr, io::Error>
    where
        S: AsRef<str> + ?Sized,
    {
        let v: Vec<&str> = s.as_ref().split(':').collect();
        let mut bytes = [0u8; MAC_ADDR_LEN];
        let common_err = Err(io::Error::other(format!(
            "parsing of {} into a MAC address failed",
            s.as_ref()
        )));

        if v.len() != MAC_ADDR_LEN {
            return common_err;
        }

        for i in 0..MAC_ADDR_LEN {
            if v[i].len() != 2 {
                return common_err;
            }
            bytes[i] = u8::from_str_radix(v[i], 16).map_err(|e| {
                io::Error::other(format!(
                    "parsing of {} into a MAC address failed: {}",
                    s.as_ref(),
                    e
                ))
            })?;
        }

        Ok(MacAddr { bytes })
    }

    // Does not check whether src.len() == MAC_ADDR_LEN.
    #[inline]
    pub fn from_bytes_unchecked(src: &[u8]) -> MacAddr {
        // TODO: using something like std::mem::uninitialized could avoid the extra initialization,
        // if this ever becomes a performance bottleneck.
        let mut bytes = [0u8; MAC_ADDR_LEN];
        bytes[..].copy_from_slice(src);

        MacAddr { bytes }
    }

    // An error can only occur if the slice length is different from MAC_ADDR_LEN.
    #[inline]
    pub fn from_bytes(src: &[u8]) -> Result<MacAddr, io::Error> {
        if src.len() != MAC_ADDR_LEN {
            return Err(io::Error::other(format!(
                "invalid length of slice: {} vs {}",
                src.len(),
                MAC_ADDR_LEN
            )));
        }
        Ok(MacAddr::from_bytes_unchecked(src))
    }

    #[inline]
    pub fn get_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let b = &self.bytes;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

#[allow(dead_code)]
pub enum MacAddrParseError {
    InvalidValue(String),
}

impl FromStr for MacAddr {
    type Err = MacAddrParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        MacAddr::parse_str(s).map_err(|_| MacAddrParseError::InvalidValue(s.to_owned()))
    }
}
