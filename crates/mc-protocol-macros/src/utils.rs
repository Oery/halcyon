pub fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        match c.is_uppercase() {
            true => {
                if i != 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            }
            false => out.push(c),
        };
    }
    out
}
