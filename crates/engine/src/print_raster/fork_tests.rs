use super::*;

#[test]
fn print_raster_reports_the_page_number_and_parser_error() {
    let source = ExportSource { bytes: Arc::new(b"invalid PDF".to_vec()), config: Default::default(), pages: 1, sizes: vec![(200.0, 200.0)] };
    let mut printer = renderer(&source, source.bytes.clone(), None);
    let error = render(&mut printer, &source, 0, 72.0, false).err().expect("parser error");
    assert_eq!(error, "page 1: the document could not be parsed");
}
