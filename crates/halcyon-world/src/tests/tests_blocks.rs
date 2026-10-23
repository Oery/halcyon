use super::*;
use crate::blocks::entities::{MAX_SIGN_BYTES, Sign, SignFace};
use crate::disk_nbt::DiskNbt;

#[test]
fn placement_facing_handles_large_finite_angles_without_overflow() {
    for (yaw, direction) in [
        (0.0, "south"),
        (90.0, "west"),
        (-90.0, "east"),
        (180.0, "north"),
        (450.0, "west"),
        (f32::MAX, "east"),
        (-f32::MAX, "south"),
    ] {
        assert_eq!(facing(yaw), direction, "yaw {yaw}");
    }
}

#[test]
fn generated_catalog_matches_registry_and_default_tools() {
    assert_eq!(data::STATES.len(), BLOCK_STATES.len());
    for &(key, id) in BLOCK_STATES {
        assert_eq!(metadata(id as u16).name, key.split('[').next().unwrap());
    }
    let stone = named("minecraft:stone").unwrap();
    assert_eq!(stone.hardness, 1.5);
    assert!(stone.requires_tool);
    assert!(stone.tools.iter().any(|t| t.speed == 2.0 && t.correct));
    assert!(sturdy(stone.default, 1));
    assert!(!reserves_use(stone.default));
    assert!(reserves_use(named("minecraft:chest").unwrap().default));
}

#[test]
fn static_sign_text_preserves_styles_but_removes_actions_and_bounds_content() {
    let line = DiskNbt::Compound(vec![
        ("text".to_owned(), DiskNbt::Str("Welcome".to_owned())),
        ("bold".to_owned(), DiskNbt::Byte(1)),
        ("click_event".to_owned(), DiskNbt::Compound(vec![])),
    ]);
    let sign = Sign {
        front: SignFace { messages: std::array::from_fn(|_| line.clone()), ..Default::default() },
        ..Default::default()
    }
    .sanitized()
    .unwrap();
    let DiskNbt::Compound(fields) = &sign.front.messages[0] else {
        panic!("component")
    };
    assert!(fields.iter().any(|(key, _)| key == "bold"));
    assert!(!fields.iter().any(|(key, _)| key == "click_event"));
    let sign = Sign {
        front: SignFace::plain(std::array::from_fn(|_| "x".repeat(MAX_SIGN_BYTES))),
        ..Default::default()
    };
    assert!(sign.sanitized().is_none());
}
