use super::*;

#[test]
fn documentation_does_not_hide_a_field_format() {
    let attrs = vec![
        syn::parse_quote!(#[doc="An optional entity ID"]),
        syn::parse_quote!(#[format="varint"]),
    ];
    assert!(matches!(Format::from(&attrs), Format::VarInt));
}
