//! Arabic lines for the ocrs engine (cargo feature `arabic`, off by default).
//!
//! PaddleOCR's Arabic text-line recogniser (`arabic_PP-OCRv5_mobile_rec`, Apache-2.0, ONNX, run
//! by rten) reads every line ocrs's detector finds. A line it reads as mostly Arabic letters takes
//! that reading; every other line keeps ocrs's. Its two files, [`ARABIC_MODEL`] and
//! [`ARABIC_CHARACTERS`], are `[[fetched]]` entries in ATTRIBUTION.toml (`cargo xtask models
//! --arabic` downloads them from the URL there, or its `mirror_url`), and are looked for beside ocrs's
//! models; without them every line keeps ocrs's reading.

use std::path::Path;

use rten::ctc::CtcDecoder;
use rten_imageproc::RectF;
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, Tensor};

use super::ocrs::sha256_hex;
use super::{OcrError, Word};

/// The model files, as named in ATTRIBUTION.toml.
pub const ARABIC_MODEL: &str = "arabic-recognition.onnx";
/// The model's settings, whose `character_dict` lists its output classes in order.
pub const ARABIC_CHARACTERS: &str = "arabic-recognition.yml";
/// Their SHA-256s, as in ATTRIBUTION.toml: nothing else is loaded (see `Ocr::from_bytes`).
pub const ARABIC_MODEL_SHA256: &str = "799113ebf267fbe742deb99eb36e8d42c9ddc5291ceacf92add41b4d52a59110";
pub const ARABIC_CHARACTERS_SHA256: &str = "21368419e6c016c31db55d316d59e11c128e1913e6e6fe10287084710043d3a6";

/// The model reads line images this many pixels high.
const LINE_HEIGHT: usize = 48;
/// The widest line image it is given (its published input range ends here); a longer line is
/// squeezed to this width.
const MAX_LINE_WIDTH: usize = 3200;

/// The loaded Arabic recogniser.
pub(crate) struct ArabicReader {
    model: rten::Model,
    /// Output class `n` (after the CTC blank, class 0) is `classes[n - 1]`.
    classes: Vec<char>,
}

impl ArabicReader {
    /// The reader whose two files are in `dir`, or `None` when either is missing.
    pub(crate) fn in_dir(dir: &Path) -> Result<Option<ArabicReader>, OcrError> {
        let (model, characters) = (dir.join(ARABIC_MODEL), dir.join(ARABIC_CHARACTERS));
        if !model.is_file() || !characters.is_file() {
            return Ok(None);
        }
        let read = |p: &Path| std::fs::read(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        Self::from_bytes(read(&model)?, &read(&characters)?).map(Some)
    }

    fn from_bytes(model: Vec<u8>, characters: &[u8]) -> Result<ArabicReader, OcrError> {
        for (name, sha256, data) in [(ARABIC_MODEL, ARABIC_MODEL_SHA256, model.as_slice()), (ARABIC_CHARACTERS, ARABIC_CHARACTERS_SHA256, characters)]
        {
            if sha256_hex(data) != sha256 {
                return Err(OcrError::Load(name.into(), "not the expected model file (SHA-256 differs)".into()));
            }
        }
        let classes =
            character_dict(characters).ok_or_else(|| OcrError::Load(ARABIC_CHARACTERS.into(), "no character_dict of single characters".into()))?;
        let model = rten::Model::load(model).map_err(|e| OcrError::Load(ARABIC_MODEL.into(), e.to_string()))?;
        Ok(ArabicReader { model, classes })
    }

    /// Read the line inside `rect` of a `width` × `height` grey image, in logical (typed) order.
    pub(crate) fn read_line(&self, grey: &[u8], width: usize, height: usize, rect: RectF) -> Result<String, OcrError> {
        let clamp = |v: f32, max: usize| if v.is_finite() { (v.max(0.0) as usize).min(max) } else { 0 };
        let (left, right) = (clamp(rect.left().floor(), width), clamp(rect.right().ceil(), width));
        let (top, bottom) = (clamp(rect.top().floor(), height), clamp(rect.bottom().ceil(), height));
        let (w, h) = (right.saturating_sub(left), bottom.saturating_sub(top));
        if w == 0 || h == 0 {
            return Ok(String::new());
        }
        // Scaled to the model's height, keeping the line's proportions.
        let scaled_width = (w * LINE_HEIGHT).div_ceil(h).clamp(LINE_HEIGHT / 3, MAX_LINE_WIDTH);
        let plane = scale_line(grey, width, [left, top, w, h], scaled_width);
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(format!("Arabic line: {e}"));
        // The model takes three colour channels; a grey line is the same in each.
        let input: Tensor<f32> = NdTensor::from_data([1, 3, LINE_HEIGHT, scaled_width], plane.repeat(3)).into();
        let output = self.model.run_one(input.view().into(), None).map_err(|e| err(&e))?;
        let output: NdTensor<f32, 3> = output.try_into().map_err(|e| err(&e))?;
        if output.size(0) != 1 || output.size(2) != self.classes.len() + 1 {
            return Err(err(&format!("expected [1, steps, {}] class scores, got {:?}", self.classes.len() + 1, output.shape())));
        }
        let steps = CtcDecoder::new().decode_greedy(output.slice([0]));
        let visual: Vec<char> = steps.steps().iter().filter_map(|s| self.classes.get((s.label as usize).checked_sub(1)?).copied()).collect();
        Ok(to_logical(&visual))
    }
}

/// The `w` × `h` box at (`left`, `top`) of `grey` (`width` pixels a row), scaled bilinearly to
/// `out_width` × [`LINE_HEIGHT`], with values -1 (black) to 1. Only the scaled line is allocated
/// (at most [`MAX_LINE_WIDTH`] × [`LINE_HEIGHT`]); a pixel outside `grey` reads as white.
fn scale_line(grey: &[u8], width: usize, [left, top, w, h]: [usize; 4], out_width: usize) -> Vec<f32> {
    let pixel = |x: usize, y: usize| {
        let at = top.saturating_add(y).saturating_mul(width).saturating_add(left.saturating_add(x));
        grey.get(at).map_or(1.0, |&v| f32::from(v) / 127.5 - 1.0)
    };
    // Where output position `i` of `n` samples from in a run of `len` pixels: two neighbours and
    // the weight of the second.
    let source = |i: usize, n: usize, len: usize| {
        let at = ((i as f32 + 0.5) * len as f32 / n as f32 - 0.5).clamp(0.0, len.saturating_sub(1) as f32);
        let first = at as usize;
        (first, (first + 1).min(len.saturating_sub(1)), at - first as f32)
    };
    let mut out = Vec::with_capacity(out_width * LINE_HEIGHT);
    for oy in 0..LINE_HEIGHT {
        let (y0, y1, fy) = source(oy, LINE_HEIGHT, h);
        for ox in 0..out_width {
            let (x0, x1, fx) = source(ox, out_width, w);
            let top_row = pixel(x0, y0) * (1.0 - fx) + pixel(x1, y0) * fx;
            let bottom_row = pixel(x0, y1) * (1.0 - fx) + pixel(x1, y1) * fx;
            out.push(top_row * (1.0 - fy) + bottom_row * fy);
        }
    }
    out
}

/// The `character_dict` list of a PaddleOCR `inference.yml`, one character per entry, with the
/// space class the model adds after it. `None` if there is no such list or an entry is not one
/// character.
fn character_dict(yml: &[u8]) -> Option<Vec<char>> {
    let text = std::str::from_utf8(yml).ok()?;
    let mut lines = text.lines().skip_while(|l| l.trim_end() != "  character_dict:").skip(1);
    let mut classes = Vec::new();
    for entry in lines.by_ref().map_while(|l| l.strip_prefix("  - ")) {
        let entry = match entry.strip_prefix('\'').and_then(|e| e.strip_suffix('\'')) {
            Some(quoted) => quoted.replace("''", "'"),
            None => entry.to_owned(),
        };
        let mut chars = entry.chars();
        classes.push(chars.next().filter(|_| chars.next().is_none())?);
    }
    (!classes.is_empty()).then(|| {
        classes.push(' ');
        classes
    })
}

/// A line read left to right (as drawn) in the order it is typed: right to left, except that a
/// run of Latin letters, digits and the punctuation inside numbers and codes keeps its order.
/// Spaces inside such a run join it, so `A B` drawn between Arabic words still reads `A B`; the
/// spaces at its ends stay between it and its neighbours.
fn to_logical(visual: &[char]) -> String {
    let keeps_order = |c: char| c.is_ascii_alphanumeric() || c.is_numeric() || ":*./%+-".contains(c);
    let mut runs: Vec<&[char]> = Vec::new();
    let mut rest = visual;
    while let Some(&first) = rest.first() {
        let len = if keeps_order(first) {
            // The run, with the spaces inside it but not those after it.
            let reach = rest.iter().take_while(|&&c| c == ' ' || keeps_order(c)).count();
            rest.get(..reach).and_then(|run| run.iter().rposition(|&c| c != ' ')).map_or(1, |last| last + 1)
        } else {
            1
        };
        let (run, tail) = rest.split_at(len.min(rest.len()));
        runs.push(run);
        rest = tail;
    }
    runs.iter().rev().flat_map(|run| run.iter()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a reading is an Arabic line: more Arabic letters than Latin ones.
pub(crate) fn is_arabic(text: &str) -> bool {
    let arabic = text.chars().filter(|c| matches!(c, '\u{0620}'..='\u{064A}' | '\u{066E}'..='\u{06D3}' | '\u{0750}'..='\u{077F}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')).count();
    arabic > text.chars().filter(char::is_ascii_alphabetic).count()
}

/// The words of an Arabic line read as `text` (typed order), placed on the boxes ocrs found for
/// it (`found`, left to right as drawn, so its last box holds the first word). When the counts
/// differ, the line is one word spanning those boxes, or `line` (the box the line was found in)
/// when ocrs read no word in it.
pub(crate) fn line_words(text: &str, found: &[Word], line: [f32; 4]) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    if words.len() == found.len() {
        return words.iter().zip(found.iter().rev()).map(|(w, f)| Word::new(*w, f.rect, "ocrs")).collect();
    }
    let span = found.iter().map(|w| w.rect).reduce(|[l, t, r, b], [l2, t2, r2, b2]| [l.min(l2), t.min(t2), r.max(r2), b.max(b2)]);
    vec![Word::new(words.join(" "), span.unwrap_or(line), "ocrs")]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of `tests/data/arabic-scan.png` (its README says how it was made): two verses of
    /// al-Mutanabbi and two proverbs, all public domain.
    const SCAN_TEXT: &str = "الخيل والليل والبيداء تعرفني\nوالسيف والرمح والقرطاس والقلم\nأنا الذي نظر الأعمى إلى أدبي\nوأسمعت كلماتي من به صمم\nالعلم في الصغر كالنقش على الحجر\nمن جد وجد ومن زرع حصد";
    /// The lowest character accuracy the scan may be read with: 0.9697 measured on 19 November
    /// 2026 (crates/ocr/README.md has the figure for ocrs alone).
    const SCAN_ACCURACY: f64 = 0.95;

    fn edit_distance(a: &[char], b: &[char]) -> usize {
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut diagonal = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let above = row[j + 1];
                row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != cb));
                diagonal = above;
            }
        }
        row[b.len()]
    }

    /// Character accuracy, 1 − edit distance ÷ the text's length, on a 200 dpi scan of six
    /// lines of Arabic: the lines in order, words joined by spaces.
    #[test]
    fn reads_an_arabic_scan() {
        let Some(models) = crate::Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        if !models.recognition.parent().is_some_and(|d| d.join(ARABIC_MODEL).is_file() && d.join(ARABIC_CHARACTERS).is_file()) {
            eprintln!("skipped: Arabic OCR models not installed (cargo xtask models --arabic)");
            return;
        }
        let ocr = crate::Ocr::load(&models).unwrap();
        let image = craft_image_codecs::decode(include_bytes!("../tests/data/arabic-scan.png")).unwrap();
        let lines = ocr.recognize(&image.to_rgba8(), image.width(), image.height()).unwrap();
        let read = lines.iter().map(crate::Line::text).collect::<Vec<_>>().join("\n");
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        let accuracy = 1.0 - edit_distance(&chars(&read), &chars(SCAN_TEXT)) as f64 / chars(SCAN_TEXT).len() as f64;
        eprintln!("Arabic scan: character accuracy {accuracy:.4}; read:\n{read}");
        assert!(accuracy >= SCAN_ACCURACY, "accuracy {accuracy:.4} is under {SCAN_ACCURACY}");
    }

    #[test]
    fn edit_distance_counts_insertions_deletions_and_substitutions() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(edit_distance(&chars("كتاب"), &chars("كتاب")), 0);
        assert_eq!(edit_distance(&chars("كتب"), &chars("كتاب")), 1);
        assert_eq!(edit_distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(edit_distance(&[], &chars("abc")), 3);
    }

    #[test]
    fn the_character_list_is_read_from_the_settings() {
        let yml = "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - $\n  - ''''\n  - '0'\n  - ب\n";
        assert_eq!(character_dict(yml.as_bytes()), Some(vec!['!', '$', '\'', '0', 'ب', ' ']));
        // A list that ends is not run on into the next key.
        assert_eq!(character_dict(b"  character_dict:\n  - a\nGlobal:\n  - b\n"), Some(vec!['a', ' ']));
        assert_eq!(character_dict(b"  character_dict:\n  - ab\n"), None, "an entry of two characters");
        assert_eq!(character_dict(b"PostProcess:\n  name: x\n"), None, "no list");
        assert_eq!(character_dict(&[0xff, 0xfe]), None, "not UTF-8");
    }

    #[test]
    fn a_line_drawn_left_to_right_reads_in_typed_order() {
        let visual = |s: &str| s.chars().collect::<Vec<_>>();
        // "سلام عليكم" as drawn: the last letter first.
        assert_eq!(to_logical(&visual("مكيلع مالس")), "سلام عليكم");
        // Latin words and numbers keep their order where they stand.
        assert_eq!(to_logical(&visual("PDF 2.0 فلم")), "ملف PDF 2.0");
        assert_eq!(to_logical(&visual("ب A B ا")), "ا A B ب");
        assert_eq!(to_logical(&visual("١٢ ةحفص")), "صفحة ١٢");
        assert_eq!(to_logical(&[]), "");
    }

    #[test]
    fn a_line_is_arabic_when_most_of_its_letters_are() {
        assert!(is_arabic("ملفات PDF"));
        assert!(!is_arabic("PDF ملف file"));
        assert!(!is_arabic("12 34"));
        assert!(!is_arabic(""));
    }

    #[test]
    fn words_take_the_boxes_found_for_them_from_the_right() {
        let found = [Word::new("x", [0.0, 0.0, 10.0, 5.0], "ocrs"), Word::new("y", [20.0, 0.0, 30.0, 5.0], "ocrs")];
        let line = [-2.0, -1.0, 32.0, 6.0];
        let words = line_words("أول ثاني", &found, line);
        assert_eq!(words.iter().map(|w| (w.text.as_str(), w.rect[0])).collect::<Vec<_>>(), [("أول", 20.0), ("ثاني", 0.0)]);
        // A count that differs makes one word over the words found.
        let one = line_words("أ ب ج", &found, line);
        assert_eq!(one, [Word::new("أ ب ج", [0.0, 0.0, 30.0, 5.0], "ocrs")]);
        assert!(line_words("", &found, line).is_empty());
        // A line ocrs read no word in keeps the Arabic reading, over the line's box.
        assert_eq!(line_words("سلام عليكم", &[], line), [Word::new("سلام عليكم", line, "ocrs")]);
    }

    #[test]
    fn a_line_is_scaled_to_the_model_height_within_its_box() {
        // 4 × 2 pixels: the left half black, the right half white.
        let grey = [0, 0, 255, 255, 0, 0, 255, 255];
        let out = scale_line(&grey, 4, [0, 0, 4, 2], 4);
        assert_eq!(out.len(), 4 * LINE_HEIGHT);
        assert!(out.chunks(4).all(|row| row[0] == -1.0 && row[3] == 1.0), "{out:?}");
        // A box reaching past the image reads white there, never out of bounds.
        let out = scale_line(&grey, 4, [3, 1, 5, 5], 2);
        assert_eq!(out.len(), 2 * LINE_HEIGHT);
        assert!(out.iter().all(|v| (-1.0..=1.0).contains(v)));
    }

    #[test]
    fn files_that_are_not_the_pinned_model_are_refused() {
        assert!(matches!(ArabicReader::from_bytes(vec![0xff; 64], b"x"), Err(OcrError::Load(name, _)) if name == ARABIC_MODEL));
        let dir = tempfile::tempdir().unwrap();
        assert!(ArabicReader::in_dir(dir.path()).unwrap().is_none(), "no files: no reader");
        std::fs::write(dir.path().join(ARABIC_MODEL), b"model").unwrap();
        assert!(ArabicReader::in_dir(dir.path()).unwrap().is_none(), "one file: no reader");
        std::fs::write(dir.path().join(ARABIC_CHARACTERS), b"chars").unwrap();
        assert!(ArabicReader::in_dir(dir.path()).is_err(), "both, but not the pinned files");
    }
}
