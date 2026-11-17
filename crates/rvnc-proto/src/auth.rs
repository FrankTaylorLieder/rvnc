//! Security-type exchanges: VNC Authentication (type 2, RFC 6143 §7.2.2) and
//! Apple Remote Desktop Diffie-Hellman authentication (type 30).
//!
//! These are pure functions. Callers do the I/O and supply the random bytes,
//! so the exchanges are deterministic under test.

use std::io::Read;

use aes::Aes128;
use des::Des;
use des::cipher::generic_array::GenericArray;
use des::cipher::{BlockEncrypt, KeyInit};
use md5::{Digest, Md5};
use num_bigint::BigUint;

use crate::ProtoError;
use crate::wire::read_u16;

/// Length of the VNC Authentication challenge and response.
pub const VNC_CHALLENGE_LEN: usize = 16;
/// VNC Authentication uses at most this many bytes of the password.
pub const VNC_PASSWORD_MAX: usize = 8;

/// Computes the response to a VNC Authentication challenge.
///
/// The challenge is encrypted with DES in ECB mode. The key is the first 8
/// bytes of the password, zero-padded, with the bits of each byte reversed
/// (a quirk of the original implementation that every server copies).
/// Bytes after the eighth are ignored.
pub fn vnc_auth_response(
    challenge: &[u8; VNC_CHALLENGE_LEN],
    password: &[u8],
) -> [u8; VNC_CHALLENGE_LEN] {
    let mut key = [0u8; VNC_PASSWORD_MAX];
    for (k, p) in key.iter_mut().zip(password) {
        *k = p.reverse_bits();
    }
    let des = Des::new(&key.into());
    key.fill(0);
    let mut out = *challenge;
    for block in out.chunks_exact_mut(8) {
        des.encrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

/// Upper bound on the ARD Diffie-Hellman key length we accept (bytes).
/// macOS uses 128 (a 1024-bit group).
pub const ARD_MAX_KEY_LEN: u16 = 1024;
/// Size of each of the username and password fields in the ARD
/// credentials block. Each holds up to 63 bytes plus a NUL terminator.
pub const ARD_FIELD_LEN: usize = 64;
/// Size of the ARD credentials block (username field + password field).
pub const ARD_CREDENTIALS_LEN: usize = 2 * ARD_FIELD_LEN;

/// The Diffie-Hellman parameters an ARD server sends after the client picks
/// security type 30.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArdParams {
    pub generator: u16,
    /// The prime modulus, big-endian. Its length is the key length.
    pub prime: Vec<u8>,
    /// The server's public key, big-endian, the same length as `prime`.
    pub server_public: Vec<u8>,
}

impl ArdParams {
    /// Reads `generator: u16, key_len: u16, prime[key_len], public[key_len]`.
    pub fn read(r: &mut impl Read) -> Result<Self, ProtoError> {
        let generator = read_u16(r)?;
        let key_len = read_u16(r)?;
        if key_len > ARD_MAX_KEY_LEN {
            return Err(ProtoError::TooLong {
                what: "ARD key",
                len: key_len.into(),
                limit: ARD_MAX_KEY_LEN.into(),
            });
        }
        if key_len == 0 {
            return Err(ProtoError::BadAuth("ARD key length is zero"));
        }
        let mut prime = vec![0u8; key_len.into()];
        r.read_exact(&mut prime)?;
        let mut server_public = vec![0u8; key_len.into()];
        r.read_exact(&mut server_public)?;
        Ok(Self {
            generator,
            prime,
            server_public,
        })
    }

    pub fn key_len(&self) -> usize {
        self.prime.len()
    }
}

/// Builds the client's reply to [`ArdParams`]: the 128-byte AES-encrypted
/// credentials block followed by the client's public key.
///
/// - `private` is random bytes for the client's DH private key (any length;
///   [`ArdParams::key_len`] bytes is appropriate).
/// - `padding` fills the unused parts of the credentials block, so it
///   should be random too.
///
/// The AES-128 key is the MD5 hash of the shared secret. Only the
/// credentials are encrypted; the session that follows is not.
pub fn ard_response(
    params: &ArdParams,
    username: &[u8],
    password: &[u8],
    private: &[u8],
    padding: &[u8; ARD_CREDENTIALS_LEN],
) -> Result<Vec<u8>, ProtoError> {
    for (what, field) in [("ARD username", username), ("ARD password", password)] {
        if field.len() >= ARD_FIELD_LEN {
            return Err(ProtoError::TooLong {
                what,
                len: field.len() as u64,
                limit: (ARD_FIELD_LEN - 1) as u64,
            });
        }
    }

    let p = BigUint::from_bytes_be(&params.prime);
    let g = BigUint::from(params.generator);
    let server_public = BigUint::from_bytes_be(&params.server_public);
    let one = BigUint::from(1u8);
    if p <= one || g <= one || g >= p {
        return Err(ProtoError::BadAuth("invalid ARD Diffie-Hellman group"));
    }
    if server_public <= one || server_public >= &p - &one {
        return Err(ProtoError::BadAuth("invalid ARD server public key"));
    }

    // Keep the exponent in [2, p-2].
    let x = BigUint::from_bytes_be(private) % (&p - 3u8) + 2u8;
    let public = g.modpow(&x, &p);
    let shared = server_public.modpow(&x, &p);

    let key_len = params.key_len();
    let mut shared_bytes = pad_be(&shared, key_len);
    let key = Md5::digest(&shared_bytes);
    shared_bytes.fill(0);

    let mut block = *padding;
    block[..username.len()].copy_from_slice(username);
    block[username.len()] = 0;
    block[ARD_FIELD_LEN..ARD_FIELD_LEN + password.len()].copy_from_slice(password);
    block[ARD_FIELD_LEN + password.len()] = 0;

    let aes = Aes128::new(&key);
    for chunk in block.chunks_exact_mut(16) {
        aes.encrypt_block(GenericArray::from_mut_slice(chunk));
    }

    let mut out = Vec::with_capacity(ARD_CREDENTIALS_LEN + key_len);
    out.extend_from_slice(&block);
    out.extend_from_slice(&pad_be(&public, key_len));
    Ok(out)
}

/// Big-endian bytes of `n`, left-padded with zeros to `len`. `n` must be
/// less than the modulus, so it always fits.
fn pad_be(n: &BigUint, len: usize) -> Vec<u8> {
    let b = n.to_bytes_be();
    let mut out = vec![0u8; len.saturating_sub(b.len())];
    out.extend_from_slice(&b);
    out
}

#[cfg(test)]
mod tests {
    use aes::cipher::BlockDecrypt;

    use super::*;

    /// Cross-checked with `openssl enc -des-ecb -nopad` using the
    /// bit-reversed key.
    #[test]
    fn vnc_auth_known_answer() {
        let challenge: [u8; 16] = *b"0123456789abcdef";
        let r = vnc_auth_response(&challenge, b"password");
        assert_eq!(hex(&r), VNC_PASSWORD_VECTOR);
        // Only the first 8 bytes matter.
        assert_eq!(vnc_auth_response(&challenge, b"password-long"), r);
        // Short and empty passwords are zero-padded.
        assert_ne!(vnc_auth_response(&challenge, b"pass"), r);
        let _ = vnc_auth_response(&challenge, b"");
    }

    const VNC_PASSWORD_VECTOR: &str = "5645abeb5f1e6475e8feb11beb66ea19";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// 2^127 - 1, a Mersenne prime: a small but valid group for tests.
    fn test_prime() -> Vec<u8> {
        let mut p = vec![0xffu8; 16];
        p[0] = 0x7f;
        p
    }

    /// Plays the server side: checks the client's reply decrypts to the
    /// credentials.
    #[test]
    fn ard_round_trip() {
        let prime = test_prime();
        let p = BigUint::from_bytes_be(&prime);
        let g = BigUint::from(2u8);
        let y = BigUint::from(0x1234_5678_9abc_def0u64);
        let server_public = g.modpow(&y, &p);
        let params = ArdParams {
            generator: 2,
            prime: prime.clone(),
            server_public: pad_be(&server_public, 16),
        };

        let padding = [0xa5u8; ARD_CREDENTIALS_LEN];
        let reply = ard_response(&params, b"alice", b"s3cret", &[7u8; 16], &padding).unwrap();
        assert_eq!(reply.len(), ARD_CREDENTIALS_LEN + 16);

        let client_public = BigUint::from_bytes_be(&reply[ARD_CREDENTIALS_LEN..]);
        let shared = client_public.modpow(&y, &p);
        let key = Md5::digest(pad_be(&shared, 16));
        let aes = Aes128::new(&key);
        let mut block = reply[..ARD_CREDENTIALS_LEN].to_vec();
        for chunk in block.chunks_exact_mut(16) {
            aes.decrypt_block(GenericArray::from_mut_slice(chunk));
        }
        assert_eq!(&block[..6], b"alice\0");
        assert_eq!(&block[64..71], b"s3cret\0");
        assert!(block[6..64].iter().all(|&b| b == 0xa5));
    }

    #[test]
    fn ard_params_read() {
        let mut buf = vec![0, 2, 0, 16];
        buf.extend_from_slice(&test_prime());
        buf.extend_from_slice(&[1u8; 16]);
        let p = ArdParams::read(&mut buf.as_slice()).unwrap();
        assert_eq!((p.generator, p.key_len()), (2, 16));

        assert!(ArdParams::read(&mut [0u8, 2, 0, 0].as_slice()).is_err());
        assert!(matches!(
            ArdParams::read(&mut [0u8, 2, 0xff, 0xff].as_slice()),
            Err(ProtoError::TooLong { .. })
        ));
    }

    #[test]
    fn ard_rejects_bad_input() {
        let ok = ArdParams {
            generator: 2,
            prime: test_prime(),
            server_public: vec![1; 16],
        };
        let pad = [0u8; ARD_CREDENTIALS_LEN];
        assert!(ard_response(&ok, &[b'u'; 64], b"p", &[1], &pad).is_err());
        assert!(ard_response(&ok, b"u", &[b'p'; 64], &[1], &pad).is_err());
        assert!(ard_response(&ok, &[b'u'; 63], &[b'p'; 63], &[1], &pad).is_ok());

        let bad_pub = ArdParams {
            server_public: vec![0; 16],
            ..ok.clone()
        };
        assert!(ard_response(&bad_pub, b"u", b"p", &[1], &pad).is_err());
        let bad_g = ArdParams { generator: 1, ..ok };
        assert!(ard_response(&bad_g, b"u", b"p", &[1], &pad).is_err());
    }
}
