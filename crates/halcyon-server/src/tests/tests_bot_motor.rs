use super::*;

#[test]
fn rng_is_reproducible_and_bounded() {
    let mut a = Rng::new(7);
    let mut b = Rng::new(7);
    for _ in 0..1_000 {
        let value = a.range(-3.0, 3.0);
        assert_eq!(value, b.range(-3.0, 3.0));
        assert!((-3.0..3.0).contains(&value));
    }
}
