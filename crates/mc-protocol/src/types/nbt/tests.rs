use super::*;

#[test]
fn synchronous_nbt_encoding_preserves_nested_wire_layout() {
    futures_lite::future::block_on(async {
        let value = Nbt::Compound(vec![("x", Nbt::List(vec![Nbt::Int(42)]))]);
        let mut bytes = Vec::new();
        value.write(&mut bytes).await.unwrap();
        assert_eq!(bytes, [10, 9, 0, 1, b'x', 3, 0, 0, 0, 1, 0, 0, 0, 42, 0]);
        assert_eq!(value.body_len(), bytes.len());
        let invalid = Nbt::List(vec![Nbt::Int(1), Nbt::Byte(2)]);
        assert!(invalid.write(&mut Vec::new()).await.is_err());
    });
}
