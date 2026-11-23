// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A 16-bit PNG embeds as 8-bit, each sample its high byte.
#[test]
fn a_16_bit_png_embeds_each_samples_high_byte() {
    let samples = [0x1234, 0xABCD, 0x00FF, 0xFF00, 0x8001, 0x7FFE];
    let img = craft_image_codecs::Image::from_u16(2, 1, craft_image_codecs::ChannelLayout::Rgb, &samples).unwrap();
    let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    let decoded = craft_image_codecs::decode_as_with(craft_image_codecs::Format::Png, &png, &craft_image_codecs::DecodeOptions::default()).unwrap();
    assert_eq!(decoded.sample_type(), craft_image_codecs::SampleType::U16, "the fixture stays 16-bit");
    let embedded = png_image("deep.png", &png).unwrap();
    assert_eq!(embedded.data, [0x12, 0xAB, 0x00, 0xFF, 0x80, 0x7F]);
    assert_eq!(embedded.dict.get(b"BitsPerComponent"), Some(&Object::Int(8)));
}

#[test]
fn indexed_colour_spaces_that_refer_to_each_other_are_refused() {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    // [/Indexed b 255 <00>] and [/Indexed a 255 <00>]: a cycle through two objects.
    let (a, b) = (doc.add(Object::Null), doc.add(Object::Null));
    let indexed =
        |base| Object::Array(vec![Object::name("Indexed"), Object::Ref(base), Object::Int(255), Object::String(PdfString::literal(vec![0]))]);
    doc.set(a, indexed(b));
    doc.set(b, indexed(a));
    with_image(&mut doc, |_| {
        let mut dict = image_dict(1, 1, "DeviceGray", 8);
        dict.set(b"ColorSpace".to_vec(), Object::Ref(a));
        Stream::from_raw(dict, vec![0])
    });
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty());
    assert_eq!(out.skipped.len(), 1, "{:?}", out.skipped);
    assert_eq!(out.skipped[0].2, "nested indexed colour spaces aren't valid");
}
