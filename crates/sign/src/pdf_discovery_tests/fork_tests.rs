// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Discovery judges an encrypted file's objects before decrypting them, so finding the one
/// timestamp beside a large stream costs a small part of decrypting that stream once. Before, it
/// decrypted every object, which was most of the time `info` took on the RC4-encrypted ISO
/// 32000-1 PDF and its 127,000 objects. Timed against the decryption itself, so the margin does
/// not depend on the machine.
#[test]
fn discovery_does_not_decrypt_what_it_refuses() {
    let mut doc = fixture(&[STAMP]);
    let mut state = 1u32;
    let noise = (0..16 << 20)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state.to_be_bytes()[0]
        })
        .collect::<Vec<u8>>();
    let big = doc.add(Object::Stream(Stream::from_raw(Dict::new(), noise)));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"Big".to_vec(), Object::Ref(big))).unwrap();
    doc.set_encryption_legacy(&NewEncryption {
        algorithm: Algorithm::Rc4_128,
        user_password: "",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [29; 32],
    })
    .unwrap();
    let doc = Document::open(Arc::new(write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap())).unwrap();
    let big = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Big").unwrap();
    let fastest = |f: &dyn Fn()| {
        (0..3)
            .map(|_| {
                let start = std::time::Instant::now();
                f();
                start.elapsed()
            })
            .min()
            .unwrap()
    };
    let decrypt = fastest(&|| assert!(matches!(doc.scan_objects_subset(vec![big.num]).next(), Some((_, Ok(o))) if matches!(*o, Object::Stream(_)))));
    let discover = fastest(&|| assert_eq!(DigestCache::default().timestamp_candidates(&doc).len(), 1));
    assert!(discover * 4 < decrypt, "discovery took {discover:?}; decrypting the stream once took {decrypt:?}");
}
