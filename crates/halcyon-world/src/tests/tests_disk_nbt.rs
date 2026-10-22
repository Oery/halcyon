use super::*;

/// `{"a": 1(int), "s": "hi", "nested": {"b": 2(short)}}` root.
fn sample() -> Vec<u8> {
    let mut v = vec![10, 0, 4, b'r', b'o', b'o', b't'];
    v.extend_from_slice(&[3, 0, 1, b'a', 0, 0, 0, 1]);
    v.extend_from_slice(&[8, 0, 1, b's', 0, 2, b'h', b'i']);
    v.extend_from_slice(&[10, 0, 6, b'n', b'e', b's', b't', b'e', b'd']);
    v.extend_from_slice(&[2, 0, 1, b'b', 0, 2, 0]);
    v.extend_from_slice(&[12, 0, 1, b'L', 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7]);
    v.push(0);
    v.push(0);
    v
}

#[test]
fn decodes_compound_and_lookups() {
    let bytes = sample();
    let root = DiskNbt::read_root(&mut &bytes[..]).unwrap();
    assert_eq!(root.get("a").and_then(|v| v.as_i32()), Some(1));
    assert_eq!(root.get("s").and_then(|v| v.as_str()), Some("hi"));
    let nested = root.get("nested").unwrap();
    assert_eq!(nested.get("b").map(|v| matches!(v, DiskNbt::Short(2))), Some(true));
    assert_eq!(root.get("L").and_then(|v| v.as_long_array()), Some([7i64].as_slice()));
    assert!(root.get("missing").is_none());
    assert_eq!(root.get("a").and_then(|v| v.as_str()), None);
}

#[test]
fn truncated_input_errors() {
    let bytes = sample();
    let mut slice = &bytes[..bytes.len() - 3];
    assert!(matches!(DiskNbt::read_root(&mut slice), Err(LoadError::BadNbt { .. })));
    let mut empty = &[][..];
    assert!(matches!(DiskNbt::read_root(&mut empty), Err(LoadError::BadNbt { .. })));
}
