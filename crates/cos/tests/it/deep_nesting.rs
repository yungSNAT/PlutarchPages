//! An object nested about as deep as the parser reads is read, saved and read back on a 1 MiB
//! thread stack, the size of a Windows main thread's.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full};
use pdfcraft_testkit::Pdf;

/// Dictionaries and arrays nested `levels` deep in turn, `(deep)` at the bottom.
fn nested(levels: usize) -> String {
    let (mut open, mut close) = (String::new(), String::new());
    for i in 0..levels {
        if i % 2 == 0 {
            open.push_str("<< /A ");
            close.insert_str(0, " >>");
        } else {
            open.push('[');
            close.insert(0, ']');
        }
    }
    format!("{open}(deep){close}")
}

/// The string at the bottom of [`nested`], or `None` where the nesting breaks off.
fn bottom(mut o: &Object) -> Option<&[u8]> {
    loop {
        o = match o {
            Object::Dict(d) => d.get(b"A")?,
            Object::Array(a) => a.first()?,
            Object::String(s) => return Some(&s.bytes),
            _ => return None,
        };
    }
}

fn on_a_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(1 << 20).spawn(f).expect("a thread starts").join().expect("no stack overflow");
}

#[test]
fn a_deeply_nested_object_is_read_saved_and_read_back_on_a_small_stack() {
    on_a_small_stack(|| {
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R /Deep 3 0 R >>")
            .obj("<< /Type /Pages /Kids [] /Count 0 >>")
            .obj(nested(126))
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let deep = ObjRef::new(3, 0);
        assert_eq!(bottom(&doc.get(deep)), Some(&b"deep"[..]), "the parser reads it");
        for object_streams in [false, true] {
            let saved = write_full(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap();
            let back = Document::open(Arc::new(saved)).unwrap();
            let root = back.resolve(back.trailer().get(b"Root").unwrap());
            let held = back.resolve(root.as_dict().unwrap().get(b"Deep").unwrap());
            assert_eq!(bottom(&held), Some(&b"deep"[..]), "saved and read back (object streams {object_streams})");
        }
    });
}
