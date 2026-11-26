use super::fixture;
use crate::snap;

#[test]
fn a_truncated_form_stops_the_parent_content_stream() {
    for (saves, truncated) in [(256, false), (257, true)] {
        let body = "q ".repeat(saves);
        let form = format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{body}\nendstream", body.len());
        let doc = fixture("/Nested Do 0 0 m 10 0 l S", "", &[&form]);

        let geometry = snap::geometry(&doc, 0).unwrap();
        assert_eq!(geometry.truncated, truncated, "{saves} graphics-state saves");
        assert_eq!(geometry.segment_count(), usize::from(!truncated), "{saves} graphics-state saves");
    }
}
