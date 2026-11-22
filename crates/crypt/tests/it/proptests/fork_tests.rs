// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// The hand-written RC4 that `rc4` replaced with RustCrypto's, kept as the oracle for the test below.
fn reference_rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|b| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            b ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

proptest! {
    /// Byte for byte the output of the implementation it replaced, for every key length a damaged
    /// file could pass: empty, the 5 to 16 bytes the handler derives, and past the 256 RC4 reads.
    #[test]
    fn rc4_matches_the_implementation_it_replaced(key in prop::collection::vec(any::<u8>(), 0..300), data in prop::collection::vec(any::<u8>(), 0..600)) {
        prop_assert_eq!(rc4(&key, &data), reference_rc4(&key, &data));
    }
}
