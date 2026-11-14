//! Bounded base64-encoded little-endian signed PCM for the project format.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode<const CHANNELS: usize>(frames: &[[i16; CHANNELS]]) -> String {
    let bytes = frames
        .iter()
        .flatten()
        .flat_map(|sample| sample.to_le_bytes());
    let count = frames.len() * CHANNELS * 2;
    let mut output = String::with_capacity(count.div_ceil(3) * 4);
    let mut bytes = bytes;
    while let Some(a) = bytes.next() {
        let b = bytes.next();
        let c = bytes.next();
        let word =
            (u32::from(a) << 16) | (u32::from(b.unwrap_or(0)) << 8) | u32::from(c.unwrap_or(0));
        for shift in [18, 12] {
            output.push(char::from(ALPHABET[(word >> shift & 63) as usize]));
        }
        output.push(if b.is_some() {
            char::from(ALPHABET[(word >> 6 & 63) as usize])
        } else {
            '='
        });
        output.push(if c.is_some() {
            char::from(ALPHABET[(word & 63) as usize])
        } else {
            '='
        });
    }
    output
}

pub fn decode<const CHANNELS: usize>(
    data: &str,
    frames: usize,
    max_frames: usize,
) -> Result<Vec<[i16; CHANNELS]>, String> {
    if frames == 0 || frames > max_frames {
        return Err("PCM frame count exceeds its budget or is empty".into());
    }
    let count = frames * CHANNELS * 2;
    if data.len() != count.div_ceil(3) * 4 {
        return Err("PCM encoded size does not match its frame count".into());
    }
    let digit = |b| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut bytes = Vec::with_capacity(count);
    for group in data.as_bytes().chunks_exact(4) {
        let remaining = count - bytes.len();
        let error = || "Invalid PCM base64 or padding".to_string();
        let a = digit(group[0]).ok_or_else(error)?;
        let b = digit(group[1]).ok_or_else(error)?;
        let c = if remaining > 1 {
            digit(group[2]).ok_or_else(error)?
        } else if group[2] == b'=' && (b & 15) == 0 {
            0
        } else {
            return Err(error());
        };
        let d = if remaining > 2 {
            digit(group[3]).ok_or_else(error)?
        } else if group[3] == b'=' && (remaining != 2 || c & 3 == 0) {
            0
        } else {
            return Err(error());
        };
        bytes.push(a << 2 | b >> 4);
        if remaining > 1 {
            bytes.push(b << 4 | c >> 2);
        }
        if remaining > 2 {
            bytes.push(c << 6 | d);
        }
    }
    Ok(bytes
        .chunks_exact(CHANNELS * 2)
        .map(|frame| {
            std::array::from_fn(|ch| i16::from_le_bytes([frame[2 * ch], frame[2 * ch + 1]]))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_padding_lengths_and_rejects_invalid_data() {
        let stereo = [[-32768, 32767], [-71, -1], [0, 10]];
        for len in 1..=stereo.len() {
            let text = encode(&stereo[..len]);
            assert_eq!(decode::<2>(&text, len, 3).unwrap(), stereo[..len]);
            assert!(decode::<2>(&text, len, len - 1).is_err());
        }
        assert!(decode::<1>("AA8=", 2, 4).is_err());
        assert!(decode::<1>("AB?=", 1, 4).is_err());
        assert!(decode::<1>("AAB=", 1, 4).is_err());
    }
}
