//! Random tokens and key checks for pairing.

use sha2::{Digest, Sha256};

/// `bytes` random bytes as lowercase hex.
pub fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| format!("no random numbers from the system: {e}"))?;
    Ok(hex(&buf))
}

/// A six-digit code ("042917"), evenly spread.
pub fn six_digits() -> Result<String, String> {
    loop {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).map_err(|e| format!("no random numbers from the system: {e}"))?;
        let n = u32::from_le_bytes(b);
        // Skip the top slice so every code is equally likely.
        if n < 4_294_000_000 {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

pub fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Same answer for every input of the same length, in the same time.
pub fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_codes_have_the_right_shape() {
        let t = random_hex(16).unwrap();
        assert_eq!(t.len(), 32);
        assert_ne!(t, random_hex(16).unwrap());
        let c = six_digits().unwrap();
        assert_eq!(c.len(), 6);
        assert!(c.bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn compare_and_hash() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
