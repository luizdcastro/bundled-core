//! Base58 decoding for instruction data. Event data in a history page is a few hundred bytes per transaction, and
//! decoding it one character at a time over the whole number (the `bs58` crate) was most of a report build's CPU:
//! 75 of 89 ms per 1,000 transactions (2026-10-15). This takes five characters per step, in 32-bit limbs.

const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

const DIGITS: [u8; 128] = {
    let mut t = [255u8; 128];
    let mut i = 0;
    while i < 58 {
        t[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    t
};

/// The bytes `s` encodes, as `bs58::decode(s).into_vec()` gives them, or `None` for a character outside the alphabet.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    // Each leading '1' is a leading zero byte.
    let zeros = s.iter().take_while(|&&c| c == b'1').count();
    // The number, little-endian in base 2^32.
    let mut limbs: Vec<u32> = Vec::with_capacity(s.len() * 733 / 4000 + 2);
    for chunk in s[zeros..].chunks(5) {
        let mut mul: u64 = 1;
        let mut add: u64 = 0;
        for &c in chunk {
            let d = *DIGITS.get(c as usize)?;
            if d == 255 {
                return None;
            }
            mul *= 58;
            add = add * 58 + d as u64;
        }
        let mut carry = add;
        for limb in limbs.iter_mut() {
            let t = *limb as u64 * mul + carry;
            *limb = t as u32;
            carry = t >> 32;
        }
        while carry > 0 {
            limbs.push(carry as u32);
            carry >>= 32;
        }
    }
    let mut out = vec![0u8; zeros];
    let mut started = false;
    for limb in limbs.iter().rev() {
        for b in limb.to_be_bytes() {
            if started || b != 0 {
                started = true;
                out.push(b);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn matches_bs58_on_random_data() {
        // A small deterministic generator: no extra dependency, the same cases every run.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for len in 0..420 {
            for _ in 0..8 {
                let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
                // Leading zero bytes, which base58 writes as '1's.
                let lead = (next() % 4) as usize;
                for b in bytes.iter_mut().take(lead) {
                    *b = 0;
                }
                let text = bs58::encode(&bytes).into_string();
                assert_eq!(decode(&text), Some(bytes.clone()), "len {len}: {text}");
                assert_eq!(decode(&text), bs58::decode(&text).into_vec().ok());
            }
        }
    }

    #[test]
    fn edge_cases() {
        assert_eq!(decode(""), Some(vec![]));
        assert_eq!(decode("1"), Some(vec![0]));
        assert_eq!(decode("111"), Some(vec![0, 0, 0]));
        assert_eq!(decode("2"), Some(vec![1]));
        assert_eq!(decode("z"), Some(vec![57]));
        assert_eq!(decode("21"), Some(vec![58]));
        // Characters outside the alphabet: 0, O, I, l and anything non-ASCII.
        for bad in ["0", "O", "I", "l", "abc0", "é", "a b"] {
            assert_eq!(decode(bad), None, "{bad}");
            assert!(bs58::decode(bad).into_vec().is_err());
        }
    }
}
