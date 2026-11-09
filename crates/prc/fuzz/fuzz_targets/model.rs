//! Exercise the PRC container walk and the section decoders behind it, with a bounded inflation
//! budget. An odd first byte wraps the rest in a valid container, compressed here: a mutated
//! zlib stream rarely survives its checksum, so raw input alone seldom reaches the decoders.
#![no_main]

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use libfuzzer_sys::fuzz_target;
use pdfcraft_prc::PrcFile;

fuzz_target!(|data: &[u8]| {
    if data.len() > 128 * 1024 {
        return;
    }
    let Some((&mode, rest)) = data.split_first() else { return };
    let file = if mode & 1 == 0 { rest.to_vec() } else { container(rest) };
    if let Ok(model) = PrcFile::parse_with_limit(&file, 8 * 1024 * 1024) {
        for structure in &model.file_structures {
            let _ = structure.tessellations();
        }
        let _ = model.placements();
    }
});

/// One file structure whose five sections take the lengths in `data`'s first ten bytes (u16,
/// little-endian) from the bytes after them; whatever is left is the model file.
fn container(data: &[u8]) -> Vec<u8> {
    const HEADER_LEN: usize = 107;
    let Some((lengths, mut rest)) = data.split_first_chunk::<10>() else { return Vec::new() };
    let mut body = b"PRC".to_vec();
    put(&mut body, &[8137, 8137, 5, 6, 7, 8, 0, 0, 0, 0, 0]);
    let mut offsets = vec![HEADER_LEN];
    for &length in lengths.as_chunks::<2>().0 {
        let (section, tail) = rest.split_at(usize::from(u16::from_le_bytes(length)).min(rest.len()));
        rest = tail;
        offsets.push(HEADER_LEN + body.len());
        body.extend(zlib(section));
    }
    offsets.push(HEADER_LEN + body.len());
    body.extend(zlib(rest));
    offsets.push(HEADER_LEN + body.len());

    let mut file = b"PRC".to_vec();
    put(&mut file, &[8137, 8137, 1, 2, 3, 4, 0, 0, 0, 0, 1, 5, 6, 7, 8, 0, 6]);
    put(&mut file, &offsets.iter().map(|&o| u32::try_from(o).unwrap_or(u32::MAX)).collect::<Vec<_>>());
    put(&mut file, &[0]);
    file.extend(body);
    file
}

fn put(out: &mut Vec<u8>, values: &[u32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::none());
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}
