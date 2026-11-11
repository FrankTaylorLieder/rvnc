//! Big-endian read/write primitives.

use std::io::Read;

use crate::ProtoError;

pub(crate) fn read_u8(r: &mut impl Read) -> Result<u8, ProtoError> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

pub(crate) fn read_u16(r: &mut impl Read) -> Result<u16, ProtoError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

pub(crate) fn read_u32(r: &mut impl Read) -> Result<u32, ProtoError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

pub(crate) fn read_i32(r: &mut impl Read) -> Result<i32, ProtoError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

pub(crate) fn skip(r: &mut impl Read, n: u64) -> Result<(), ProtoError> {
    let copied = std::io::copy(&mut r.take(n), &mut std::io::sink())?;
    if copied != n {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    Ok(())
}

/// Reads a `u32` length followed by that many bytes, capped at `limit`.
pub(crate) fn read_bytes_u32(
    r: &mut impl Read,
    what: &'static str,
    limit: u32,
) -> Result<Vec<u8>, ProtoError> {
    let len = read_u32(r)?;
    if len > limit {
        return Err(ProtoError::TooLong {
            what,
            len: len.into(),
            limit: limit.into(),
        });
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Reads a `u32`-length-prefixed string, decoding it as UTF-8 (lossy).
pub(crate) fn read_string(
    r: &mut impl Read,
    what: &'static str,
    limit: u32,
) -> Result<String, ProtoError> {
    let bytes = read_bytes_u32(r, what, limit)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_be_bytes());
}
