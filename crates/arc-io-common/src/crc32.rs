//! CRC-32 with the reflected polynomial `0xEDB88320`, the checksum of ZIP entries.

const POLYNOMIAL: u32 = 0xEDB8_8320;

/// The remainder of every byte value, built at compile time. The loop bound keeps every index
/// below 256; a violated bound would fail the compilation, never a run.
const TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index: u8 = 0;
    loop {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                POLYNOMIAL ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index as usize] = value;
        if index == u8::MAX {
            break;
        }
        index += 1;
    }
    table
}

/// An incremental CRC-32 computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// A computation over no bytes yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { state: u32::MAX }
    }

    /// Adds `data` to the computation.
    pub fn update(&mut self, data: &[u8]) {
        let mut state = self.state;
        for &byte in data {
            let [index, ..] = (state ^ u32::from(byte)).to_le_bytes();
            let entry = TABLE.get(usize::from(index)).copied().unwrap_or(0);
            state = entry ^ (state >> 8);
        }
        self.state = state;
    }

    /// The checksum of the bytes added so far.
    #[must_use]
    pub const fn value(&self) -> u32 {
        self.state ^ u32::MAX
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// The CRC-32 of `data`.
#[must_use]
pub fn checksum(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.value()
}

#[cfg(test)]
mod tests {
    use super::{Crc32, checksum};

    #[test]
    fn check_value_of_the_standard_test_string() {
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_input_has_checksum_zero() {
        assert_eq!(checksum(&[]), 0);
    }

    #[test]
    fn incremental_update_equals_one_shot() {
        let mut crc = Crc32::new();
        crc.update(b"12345");
        crc.update(b"6789");
        assert_eq!(crc.value(), checksum(b"123456789"));
    }
}
