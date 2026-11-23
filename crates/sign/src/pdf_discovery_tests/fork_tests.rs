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

/// A file whose bytes name no `/ByteRange` has no timestamp among the objects stored in it, so
/// discovery reads none of them: an unreadable one does not make the result incomplete.
#[test]
fn a_file_naming_no_byte_range_reads_none_of_its_stored_objects() {
    let doc = fixture(&["<< /Bounds [0 1] >>", "<< >>\nstream\nunterminated"]);
    assert!(doc.try_get(4).is_err());
    let cache = DigestCache::default();
    assert!(cache.timestamp_candidates(&doc).is_empty());
    assert!(cache.discovery.lock().unwrap().is_some(), "no stored object was read, so none failed");
    let mut edited = doc.clone();
    edited.set(ObjRef::new(3, 0), stamp());
    assert_eq!(DigestCache::default().timestamp_candidates(&edited), vec![ObjRef::new(3, 0)], "an unsaved timestamp is still read");
}

/// However the file escapes the key's characters, a timestamp it stores is still found.
/// It passes before the byte walk too: it guards the walk against matching only the literal bytes.
#[test]
fn an_escaped_byte_range_key_still_marks_a_stored_timestamp() {
    for key in ["/Byte#52ange", "/#42yteRange", "/#42#79#74#65#52#61#6e#67#65"] {
        let doc = fixture(&[&STAMP.replace("/ByteRange", key)]);
        assert!(!doc.bytes().windows(9).any(|w| w == b"ByteRange"), "{key}");
        assert_eq!(DigestCache::default().timestamp_candidates(&doc), vec![ObjRef::new(3, 0)], "{key}");
    }
}

/// Discovery finds what reading every object finds: in the repository's signed files, and with a
/// timestamp stored plainly, under an escaped key or packed in a compressed object stream.
#[test]
fn discovery_finds_what_reading_every_object_finds() {
    let every = |doc: &Document| -> Vec<ObjRef> {
        doc.scan_objects_checked().filter_map(|(r, o)| o.ok().filter(|o| is_document_timestamp(o)).map(|_| r)).collect()
    };
    let data = |name: &str| std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let signed = ["openssl-signed.pdf", "x509-rsa-sha1.pdf"].map(|name| Document::open(Arc::new(data(name))).unwrap());
    let escaped = STAMP.replace("/ByteRange", "/Byte#52ange");
    let packed = Document::open(Arc::new(write_full(&fixture(&[STAMP]), &SaveOptions::default()).unwrap())).unwrap();
    assert!(!packed.bytes().windows(9).any(|w| w == b"ByteRange"), "the fixture must hide the key in a compressed stream");
    let stamped = [fixture(&[STAMP]), fixture(&[escaped.as_str()]), packed];
    for doc in signed.iter().chain(&stamped) {
        assert_eq!(DigestCache::default().timestamp_candidates(doc), every(doc));
    }
    assert!(stamped.iter().all(|doc| every(doc).len() == 1));
}
