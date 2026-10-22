# Unit Test Layout

- Put unit tests in this directory, named `tests_<feature>.rs`, such as `tests_inventory.rs` or `tests_movement_physics.rs`.
- Extend the existing feature test file when possible. Keep tests small, deterministic and focused on behavior.
- Declare each test file from the production module it exercises using `#[cfg(test)]`, a relative `#[path = "tests/tests_<feature>.rs"]` attribute and `mod tests;`. Adjust the relative path for nested modules.
- Keep `use super::*;` and the existing logical module hierarchy so tests can access private internals. Do not broaden production visibility just to relocate tests.
- Do not add inline test blocks or test files beside production modules.
- Keep opt-in benchmarks under `src/bench/`.
