use super::*;

#[test]
fn a_rectangle_with_missing_operands_leaves_the_image_clip_path_intact() {
    let mut state = ImageState::new(Matrix::IDENTITY, [0.0, 0.0, 100.0, 100.0], Vec::new());
    let valid = parse(b"10 20 30 40 re");
    for op in &valid.ops {
        state.apply(op).unwrap();
    }
    let malformed = parse(b"80 90 5 re W n");
    assert_eq!(malformed.ops.len(), 3);
    for op in &malformed.ops {
        state.apply(op).unwrap();
    }
    assert_eq!(state.placement.clip, [10.0, 20.0, 40.0, 60.0]);
    assert_eq!(state.placement.clips.len(), 1);
}
