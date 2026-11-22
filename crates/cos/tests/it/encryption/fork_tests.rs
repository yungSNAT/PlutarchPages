// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// The fixture under RC4-128 with an empty user password and every object in the file, as the
/// ISO 32000-1 PDF stores them.
fn rc4_in_file() -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption_legacy(&NewEncryption {
        algorithm: Algorithm::Rc4_128,
        user_password: "",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

/// A filtered scan of an encrypted file judges each object first as stored, and decrypts and
/// judges again only what it keeps; an edit is judged once, as it reads now, and a read error
/// comes back whatever the filter says.
#[test]
fn a_filtered_scan_decrypts_only_what_it_keeps() {
    let bytes = rc4_in_file();
    let mut doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let info = doc.trailer().reference(b"Info").unwrap();
    let seen = std::cell::RefCell::new(Vec::new());
    let titled = |o: &Object| {
        let title = o.as_dict().and_then(|d| d.get(b"Title")).and_then(Object::as_string);
        seen.borrow_mut().extend(title.map(|t| t.bytes.clone()));
        title.is_some()
    };
    let title = |kept: Vec<(ObjRef, Result<Arc<Object>, CosError>)>| match &kept[..] {
        [(r, Ok(o))] if *r == info => o.as_dict().and_then(|d| d.get(b"Title")).and_then(Object::as_string).map(|t| t.bytes.clone()),
        _ => None,
    };
    assert_eq!(title(doc.scan_objects_where(titled).collect()).as_deref(), Some(&b"Quarterly numbers"[..]));
    let stored = seen.take();
    assert_eq!(stored.len(), 2, "as stored, then decrypted");
    assert_ne!(stored[0], b"Quarterly numbers");
    assert_eq!(stored[1], b"Quarterly numbers");

    doc.update_dict(info, |d| d.set(b"Title".to_vec(), Object::String(PdfString::literal("Revised")))).unwrap();
    assert_eq!(title(doc.scan_objects_where(titled).collect()).as_deref(), Some(&b"Revised"[..]));
    assert_eq!(seen.take(), vec![b"Revised".to_vec()]);

    // The Info dictionary's `obj` keyword broken in place: it reads nowhere, before or after decryption.
    let header = format!("\n{} 0 obj", info.num).into_bytes();
    let at = bytes.windows(header.len()).position(|w| w == header).unwrap() + header.len() - 1;
    let mut damaged = bytes;
    damaged[at] = b'x';
    let damaged = Document::open(Arc::new(damaged)).unwrap();
    let errors: Vec<ObjRef> = damaged.scan_objects_where(|_| false).filter(|(_, o)| o.is_err()).map(|(r, _)| r).collect();
    assert_eq!(errors, vec![info]);
}

/// The stored parse resolves an indirect `/Length` as a full read does, so a filtered scan returns
/// what filtering a full scan returns and lists the same unreadable objects: here a stream whose
/// `/Length` names an object that does not parse, and one whose `/Length` names itself.
#[test]
fn a_filtered_scan_reads_what_filtering_a_full_scan_reads() {
    let mut bytes = rc4_in_file();
    // An update appended by hand, its trailer the saved one's entries with a larger /Size and a /Prev.
    let saved = Document::open(Arc::new(bytes.clone())).unwrap();
    let entry = |key: &[u8]| saved.trailer().reference(key).map(|r| format!("{} {} R", r.num, r.generation)).unwrap();
    let hex = |s: &Object| s.as_string().unwrap().bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
    let ids = saved.trailer().get(b"ID").and_then(Object::as_array).unwrap().iter().map(|s| format!("<{}>", hex(s))).collect::<Vec<_>>().join(" ");
    let startxref = bytes.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
    let prev = String::from_utf8_lossy(&bytes[startxref..]).split_whitespace().next().unwrap().to_string();
    let size = u32::try_from(saved.trailer().int(b"Size").unwrap()).unwrap();
    let (broken_length, broken, cycle) = (size, size + 1, size + 2);
    let mut offsets = Vec::new();
    for object in [
        format!("{broken_length} 0 obj\n<< /Length {broken} 0 R >>\nstream\nabc\nendstream\nendobj"),
        "not an object".to_string(),
        format!("{cycle} 0 obj\n<< /Length {cycle} 0 R >>\nstream\nxyz\nendstream\nendobj"),
    ] {
        offsets.push(bytes.len() + 1);
        bytes.extend_from_slice(format!("\n{object}\n").as_bytes());
    }
    let xref = bytes.len();
    let mut update = format!("xref\n{size} 3\n");
    for offset in offsets {
        update.push_str(&format!("{offset:010} 00000 n \n"));
    }
    update.push_str(&format!(
        "trailer\n<< /Size {} /Root {} /Info {} /Encrypt {} /ID [{ids}] /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n",
        size + 3,
        entry(b"Root"),
        entry(b"Info"),
        entry(b"Encrypt"),
    ));
    bytes.extend_from_slice(update.as_bytes());
    let bytes = Arc::new(bytes);
    let read = |scan: &mut dyn Iterator<Item = (ObjRef, Result<Arc<Object>, CosError>)>| {
        scan.map(|(r, o)| (r, o.map_err(|e| e.to_string()))).collect::<Vec<_>>()
    };
    let keeps: [fn(&Object) -> bool; 3] = [|o| !matches!(o, Object::Stream(_)), |_| false, |_| true];
    for keep in keeps {
        let full = Document::open(bytes.clone()).unwrap();
        let expected = read(&mut full.scan_objects_checked().filter(|(_, o)| !matches!(o, Ok(o) if !keep(o))));
        let filtered = Document::open(bytes.clone()).unwrap();
        assert_eq!(read(&mut filtered.scan_objects_where(keep)), expected);
        assert_eq!(filtered.unreadable_objects(), full.unreadable_objects());
        assert!(full.unreadable_objects().iter().any(|line| line.starts_with(&format!("object {broken} "))), "{:?}", full.unreadable_objects());
    }
    let full = Document::open(bytes.clone()).unwrap();
    assert!(full.scan_objects_checked().any(|(r, o)| r.num == broken_length && matches!(o.as_deref(), Ok(Object::Stream(_)))));
    assert!(full.scan_objects_checked().any(|(r, o)| r.num == cycle && matches!(o.as_deref(), Ok(Object::Stream(_)))));
}

/// What a file can do with those references: point a stream's selector at the stream itself, at
/// nothing, or at what is no selector. That says nothing about the stream. Upstream reads it as
/// absent, so a stream with `/Crypt` is `/Identity` and saved in the clear. Here it follows the
/// document's filter, as one with no `/Crypt` does, and a save takes `/Crypt` out, or a full save
/// would leave a null there that is `/Identity`. Loading ends in every case.
#[test]
fn a_crypt_filter_reference_that_cannot_be_read_is_the_documents_own_filter() {
    let named = |n: Object| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Name".to_vec(), n);
        Object::Dict(d)
    };
    for what in [
        "/Filter to itself",
        "/Filter nowhere",
        "/DecodeParms to itself",
        "/DecodeParms nowhere",
        "/DecodeParms to a number",
        "a /DecodeParms entry nowhere",
        "/Name to itself",
        "/Name nowhere",
        "/Name to a number",
        "/Name that /CF does not hold",
    ] {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let me = Object::Ref(contents_of(&doc));
        let nowhere = Object::Ref(ObjRef::new(9_999, 0));
        let number = Object::Ref(doc.add(Object::Int(7)));
        let crypt = Object::name("Crypt");
        select(
            &mut doc,
            match what {
                "/Filter to itself" => (me, Object::Null),
                "/Filter nowhere" => (nowhere, Object::Null),
                "/DecodeParms to itself" => (crypt, me),
                "/DecodeParms nowhere" => (crypt, nowhere),
                "/DecodeParms to a number" => (crypt, number),
                "a /DecodeParms entry nowhere" => (Object::Array(vec![crypt]), Object::Array(vec![nowhere])),
                "/Name to itself" => (crypt, named(me)),
                "/Name nowhere" => (crypt, named(nowhere)),
                "/Name that /CF does not hold" => (crypt, named(Object::name("Missing"))),
                _ => (crypt, named(number)),
            },
        );
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(!holds(&saved, "Secret page"), "{what}: saved encrypted");
        let back = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(raw_content(&back), SECRET, "{what}: read back");
        let contents = back.get(contents_of(&back));
        assert_eq!(contents.as_dict().unwrap().contains(b"Filter"), what.starts_with("/Filter"), "{what}: /Crypt taken out");
        let full = write_full(&back, &SaveOptions::default()).unwrap();
        assert!(!holds(&full, "Secret page"), "{what}: renumbered, encrypted");
        assert_eq!(raw_content(&Document::open(Arc::new(full)).unwrap()), SECRET, "{what}: renumbered");
    }
}

/// `/StmF`, `/StrF` and `/EFF` name `/Identity` or an entry of `/CF`. A name that isn't there would
/// be `/Identity`, so the file doesn't open: its streams or strings would be saved in the clear.
#[test]
fn a_selector_naming_a_filter_cf_does_not_hold_refuses_the_file() {
    for key in ["StmF", "StrF", "EFF"] {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
        let mut dict = doc.get(encrypt).as_dict().cloned().unwrap();
        dict.set(key.as_bytes().to_vec(), Object::name("Missing"));
        doc.set(encrypt, Object::Dict(dict));
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(matches!(Document::open(Arc::new(saved)), Err(CosError::Security(_))), "{key}");
    }
}

/// Where `/StmF` is `/Identity` the document's filter is no filter, so a `/Name` that `/CF` doesn't
/// hold leaves the stream's bytes as the file holds them. Nothing was decrypted, so a save shows
/// nothing the file didn't.
#[test]
fn an_unknown_name_under_an_identity_stmf_keeps_the_bytes_the_file_holds() {
    // Object streams would be encrypted, and unreadable once `/StmF` changes.
    let doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let unpacked = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let mut doc = Document::open(Arc::new(unpacked)).unwrap();
    let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
    doc.update_dict(encrypt, |e| e.set(b"StmF".to_vec(), Object::name("Identity"))).unwrap();
    let mut doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let held = raw_content(&doc);
    let mut parms = pdfcraft_cos::Dict::new();
    parms.set(b"Name".to_vec(), Object::name("Missing"));
    select(&mut doc, (Object::name("Crypt"), Object::Dict(parms)));
    for saved in [write_incremental(&doc, &SaveOptions::default()).unwrap(), write_full(&doc, &SaveOptions::default()).unwrap()] {
        assert!(!holds(&saved, "Secret page"), "no plaintext appears");
        assert_eq!(raw_content(&Document::open(Arc::new(saved)).unwrap()), held, "the bytes stay");
    }
}

/// The same in a file from elsewhere, whose stream keeps such a `/Crypt`: its ciphertext isn't read
/// as data, and a full save neither writes it in the clear nor leaves it to read as `/Identity`.
#[test]
fn a_stored_stream_whose_crypt_filter_cannot_be_read_is_decrypted() {
    let file = classic_file();
    let doc = Document::open(Arc::new(file.bytes.clone())).unwrap();
    let contents = contents_of(&doc);
    let find = |from: usize, what: &[u8]| from + file.bytes[from..].windows(what.len()).position(|w| w == what).unwrap();
    let object = find(0, format!("\n{} 0 obj", contents.num).as_bytes());
    let data = find(object, b"stream\n") + b"stream\n".len();
    let ciphertext = &file.bytes[data..find(data, b"\nendstream")];
    let mut body = format!("<< /Length {} /Filter /Crypt /DecodeParms << /Name 9999 0 R >> >>\nstream\n", ciphertext.len()).into_bytes();
    body.extend_from_slice(ciphertext);
    body.extend_from_slice(b"\nendstream");
    let trailer = format!("/Root {} 0 R /Encrypt {} 0 R /ID [<{id}> <{id}>]", file.root.num, file.encrypt.num, id = hex(&file.id));
    let source = Document::open(Arc::new(updated(&file.bytes, &[(contents.num, body)], &trailer))).unwrap();
    assert_eq!(raw_content(&source), SECRET, "read");
    let full = write_full(&source, &SaveOptions::default()).unwrap();
    assert!(!holds(&full, "Secret page"), "renumbered, encrypted");
    assert_eq!(raw_content(&Document::open(Arc::new(full)).unwrap()), SECRET, "renumbered");
}

/// What selects a crypt filter in `/Encrypt` can be given by reference, and one that can't be read
/// says nothing about how the file is protected. Upstream reads it as absent, which is `/Identity`,
/// so the file's streams read as they are and are saved in the clear. Here the file doesn't open.
/// Below `/V 4` those entries mean nothing, and it does.
#[test]
fn an_encrypt_entry_by_a_reference_that_cannot_be_read_refuses_the_file() {
    let saved = |alg: Algorithm, edit: &dyn Fn(&mut Document, &mut pdfcraft_cos::Dict)| {
        let mut doc = Document::open(Arc::new(protect(alg, "", "owner", -1))).unwrap();
        let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
        let mut dict = doc.get(encrypt).as_dict().cloned().unwrap();
        edit(&mut doc, &mut dict);
        doc.set(encrypt, Object::Dict(dict));
        write_incremental(&doc, &SaveOptions::default()).unwrap()
    };
    let refused = |file: Vec<u8>| matches!(Document::open(Arc::new(file)), Err(CosError::Security(_)));
    let nowhere = Object::Ref(ObjRef::new(9_999, 0));
    let std_cf = |d: &mut pdfcraft_cos::Dict, entry: Object| {
        let mut cf = d.get(b"CF").unwrap().as_dict().cloned().unwrap();
        cf.set(b"StdCF".to_vec(), entry);
        d.set(b"CF".to_vec(), Object::Dict(cf));
    };
    for key in ["StmF", "StrF", "EFF", "CF"] {
        assert!(refused(saved(Algorithm::Aes128, &|_, d| d.set(key.as_bytes().to_vec(), nowhere.clone()))), "/{key} nowhere");
    }
    assert!(refused(saved(Algorithm::Aes128, &|_, d| std_cf(d, nowhere.clone()))), "the filter nowhere");
    let method_nowhere = |_: &mut Document, d: &mut pdfcraft_cos::Dict| {
        let mut filter = d.get(b"CF").unwrap().as_dict().unwrap().get(b"StdCF").unwrap().as_dict().cloned().unwrap();
        filter.set(b"CFM".to_vec(), nowhere.clone());
        std_cf(d, Object::Dict(filter));
    };
    assert!(refused(saved(Algorithm::Aes128, &method_nowhere)), "/CFM nowhere");
    assert!(refused(saved(Algorithm::Aes128, &|doc, d| d.set(b"StmF".to_vec(), Object::Ref(doc.add(Object::Int(7)))))), "/StmF to a number");
    let to_itself = |doc: &mut Document, d: &mut pdfcraft_cos::Dict| {
        let a = doc.add(Object::Null);
        doc.set(a, Object::Ref(a));
        d.set(b"StmF".to_vec(), Object::Ref(a));
    };
    assert!(refused(saved(Algorithm::Aes128, &to_itself)), "/StmF to itself");
    let v2 = saved(Algorithm::Rc4_128, &|_, d| d.set(b"StmF".to_vec(), nowhere.clone()));
    assert_eq!(content(&Document::open(Arc::new(v2)).unwrap()), SECRET, "below /V 4");
}

/// `/Encrypt` can refer to more than any file needs. Stopping there would leave an indirect `/O` or
/// `/U` out of what reads as stored, and a full save would corrupt it, so the file doesn't open.
#[test]
fn an_encrypt_dictionary_of_too_many_references_refuses_the_file() {
    let saved = |refer: &dyn Fn(&mut Document, u32) -> Object, count: u32| {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
        let mut dict = doc.get(encrypt).as_dict().cloned().unwrap();
        let extra = (0..count).map(|i| refer(&mut doc, i)).collect();
        dict.set(b"Extra".to_vec(), Object::Array(extra));
        doc.set(encrypt, Object::Dict(dict));
        write_incremental(&doc, &SaveOptions::default()).unwrap()
    };
    let objects = |doc: &mut Document, i: u32| Object::Ref(doc.add(Object::Int(i64::from(i))));
    let nowhere = |_: &mut Document, i: u32| Object::Ref(ObjRef::new(10_000 + i, 0));
    let refused = |file: Vec<u8>| matches!(Document::open(Arc::new(file)), Err(CosError::Security(_)));
    assert!(refused(saved(&objects, 4_097)), "objects");
    assert!(refused(saved(&nowhere, 4_097)), "references to nothing, each a scan of the file");
    assert_eq!(content(&Document::open(Arc::new(saved(&objects, 32))).unwrap()), SECRET, "within the limit");
}
