//! Versioned, self-describing serialization envelope for persisted filters.
//!
//! A serialized filter is a fixed header, the solution payload, and a trailing checksum. The
//! header records the format version, filter family (so a homogeneous blob cannot be loaded as
//! a standard one or vice versa), result width `R`, seed, and geometry; decoding validates
//! every field with checked arithmetic before any indexing occurs, so a malformed or truncated
//! buffer is rejected with a [`DecodeError`] rather than panicking or producing an unsound
//! filter.
//!
//! Layout (all integers little-endian):
//! ```text
//! offset  size  field
//! 0       4     magic  = b"PLT1"
//! 4       1     family = 0 (homogeneous w=64) | 1 (standard w=128)
//! 5       1     r      = result-bit width (1..=32)
//! 6       2     reserved (0)
//! 8       8     seed         (homogeneous: raw seed; standard: ordinal seed)
//! 16      8     num_starts
//! 24      8     segment_count
//! 32      ...   segments     (segment_count * elem_size bytes; elem 8B homog / 16B std)
//! end-8   8     checksum     = FNV-1a over bytes [0, end-8)
//! ```

/// Error returned when a serialized filter buffer is malformed, truncated, or does not match
/// the filter type it is being decoded into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Buffer is shorter than a valid header + checksum.
    TooShort,
    /// Magic bytes do not match; not a pleat filter buffer.
    BadMagic,
    /// Family byte does not match the filter type being decoded.
    WrongFamily,
    /// Result width `r` in the buffer does not match the target `R`.
    WrongResultWidth,
    /// A geometry field is inconsistent (bad `num_starts`, slot count, or segment count).
    BadGeometry,
    /// Declared segment count does not match the payload length.
    LengthMismatch,
    /// Seed is out of the valid range for the family.
    BadSeed,
    /// Checksum does not match the payload.
    BadChecksum,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            DecodeError::TooShort => "buffer too short for a valid filter",
            DecodeError::BadMagic => "not a pleat filter buffer (bad magic)",
            DecodeError::WrongFamily => "filter family does not match",
            DecodeError::WrongResultWidth => "result width does not match",
            DecodeError::BadGeometry => "inconsistent filter geometry",
            DecodeError::LengthMismatch => "payload length does not match declared segments",
            DecodeError::BadSeed => "seed out of range",
            DecodeError::BadChecksum => "checksum mismatch",
        })
    }
}

impl std::error::Error for DecodeError {}

pub(crate) const MAGIC: [u8; 4] = *b"PLT1";
pub(crate) const HEADER_LEN: usize = 32;
pub(crate) const CHECKSUM_LEN: usize = 8;
pub(crate) const FAMILY_HOMOG: u8 = 0;
pub(crate) const FAMILY_STD: u8 = 1;

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Validated header fields the caller needs (family, width, geometry are checked during
/// decode and not returned).
#[derive(Debug)]
pub(crate) struct Header {
    pub seed: u64,
    pub num_starts: u64,
}

/// Write a header into a fresh output vector; the caller appends the payload then calls
/// [`finish`].
pub(crate) fn write_header(
    family: u8,
    r: u8,
    seed: u64,
    num_starts: u64,
    segment_count: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&MAGIC);
    out.push(family);
    out.push(r);
    out.extend_from_slice(&[0u8, 0u8]); // reserved
    out.extend_from_slice(&seed.to_le_bytes());
    out.extend_from_slice(&num_starts.to_le_bytes());
    out.extend_from_slice(&segment_count.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// Append the checksum trailer over everything written so far.
pub(crate) fn finish(mut buf: Vec<u8>) -> Vec<u8> {
    let sum = fnv1a(&buf);
    buf.extend_from_slice(&sum.to_le_bytes());
    buf
}

/// Validate magic + checksum and decode the header. `expected_family` and `expected_r` are the
/// target type's constants; a mismatch is an error, never a silent reinterpretation.
/// `elem_size` is 8 (homogeneous u64) or 16 (standard u128). On success returns the header and
/// the payload byte slice.
pub(crate) fn decode(
    bytes: &[u8],
    expected_family: u8,
    expected_r: u8,
    elem_size: usize,
    w: usize,
) -> Result<(Header, &[u8]), DecodeError> {
    if bytes.len() < HEADER_LEN + CHECKSUM_LEN {
        return Err(DecodeError::TooShort);
    }
    if bytes[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let checksum_at = bytes.len() - CHECKSUM_LEN;
    let stored = u64::from_le_bytes(bytes[checksum_at..].try_into().unwrap());
    if fnv1a(&bytes[..checksum_at]) != stored {
        return Err(DecodeError::BadChecksum);
    }
    let family = bytes[4];
    if family != expected_family {
        return Err(DecodeError::WrongFamily);
    }
    let r = bytes[5];
    if r != expected_r {
        return Err(DecodeError::WrongResultWidth);
    }
    let seed = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let num_starts = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let segment_count = u64::from_le_bytes(bytes[24..32].try_into().unwrap());

    // Geometry: num_slots = num_starts + w - 1 must be a positive multiple of w, >= 2w,
    // and segment_count must equal (num_slots / w) * r exactly. All checked.
    let w = w as u64;
    let num_slots = num_starts
        .checked_add(w - 1)
        .ok_or(DecodeError::BadGeometry)?;
    if num_starts == 0 || num_slots % w != 0 || num_slots < 2 * w {
        return Err(DecodeError::BadGeometry);
    }
    let num_blocks = num_slots / w;
    let expect_segments = num_blocks
        .checked_mul(r as u64)
        .ok_or(DecodeError::BadGeometry)?;
    if segment_count != expect_segments {
        return Err(DecodeError::BadGeometry);
    }
    let payload_len = (segment_count as usize)
        .checked_mul(elem_size)
        .ok_or(DecodeError::BadGeometry)?;
    if HEADER_LEN + payload_len + CHECKSUM_LEN != bytes.len() {
        return Err(DecodeError::LengthMismatch);
    }
    Ok((
        Header { seed, num_starts },
        &bytes[HEADER_LEN..HEADER_LEN + payload_len],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_bad_magic_and_corrupt() {
        assert_eq!(
            decode(&[], FAMILY_HOMOG, 7, 8, 64).unwrap_err(),
            DecodeError::TooShort
        );
        // num_starts=129 => num_slots=192=3*64 => 3 blocks => 3*7=21 segments.
        let mut buf = write_header(FAMILY_HOMOG, 7, 0, 129, 21);
        buf.extend_from_slice(&[0u8; 21 * 8]);
        let good = finish(buf);
        assert!(decode(&good, FAMILY_HOMOG, 7, 8, 64).is_ok());
        // Wrong family / width.
        assert_eq!(
            decode(&good, FAMILY_STD, 7, 8, 64).unwrap_err(),
            DecodeError::WrongFamily
        );
        assert_eq!(
            decode(&good, FAMILY_HOMOG, 8, 8, 64).unwrap_err(),
            DecodeError::WrongResultWidth
        );
        // Corrupt a payload byte -> checksum fails.
        let mut bad = good.clone();
        bad[HEADER_LEN] ^= 0xFF;
        assert_eq!(
            decode(&bad, FAMILY_HOMOG, 7, 8, 64).unwrap_err(),
            DecodeError::BadChecksum
        );
        // Bad magic (checked before checksum).
        let mut nm = good.clone();
        nm[0] = b'X';
        assert_eq!(
            decode(&nm, FAMILY_HOMOG, 7, 8, 64).unwrap_err(),
            DecodeError::BadMagic
        );
    }

    #[test]
    fn rejects_inconsistent_geometry() {
        // segment_count says 8 but num_starts implies 7 -> BadGeometry (caught before length).
        let mut buf = write_header(FAMILY_HOMOG, 7, 0, 129, 8);
        buf.extend_from_slice(&[0u8; 8 * 8]);
        let b = finish(buf);
        assert_eq!(
            decode(&b, FAMILY_HOMOG, 7, 8, 64).unwrap_err(),
            DecodeError::BadGeometry
        );
    }
}
