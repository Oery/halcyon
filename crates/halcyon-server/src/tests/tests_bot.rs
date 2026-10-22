use super::*;

#[test]
fn identical_inputs_are_not_resubmitted() {
    let input = MovementInput { forward: true, yaw: 10.0, ..Default::default() };
    assert!(same_input(input, input));
    assert!(!same_input(input, MovementInput { yaw: 10.5, ..input }));
}

#[test]
fn level_ranges_parse_and_clamp() {
    assert_eq!(profile::parse_levels(None), (20, 90));
    assert_eq!(profile::parse_levels(Some("70")), (70, 70));
    assert_eq!(profile::parse_levels(Some("90-10")), (10, 90));
    assert_eq!(profile::parse_levels(Some("0-250")), (0, 100));
    assert_eq!(profile::parse_levels(Some("x")), (20, 90));
    assert_eq!(parse_cell("1, -2,3"), Some([1, -2, 3]));
    assert_eq!(parse_cell("1,2"), None);
}
