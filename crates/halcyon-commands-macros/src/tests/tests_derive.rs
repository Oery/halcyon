use super::*;

#[test]
fn invalid_argument_layouts_and_command_shapes_have_specific_diagnostics() {
    for (input, expected) in [
        (
            syn::parse_quote!(
                struct Bad {
                    optional: Option<i32>,
                    required: i32,
                }
            ),
            "required arguments cannot follow optional arguments",
        ),
        (
            syn::parse_quote!(
                enum Bad {}
            ),
            "at least one alternative",
        ),
        (
            syn::parse_quote!(
                enum Bad {
                    Empty,
                }
            ),
            "one CommandArgs type",
        ),
        (
            syn::parse_quote!(
                enum Bad {
                    Pair(A, B),
                }
            ),
            "exactly one CommandArgs type",
        ),
        (
            syn::parse_quote!(
                struct Bad(i32);
            ),
            "named fields",
        ),
        (
            syn::parse_quote!(
                struct Bad<T> {
                    value: T,
                }
            ),
            "no type or const parameters",
        ),
        (
            syn::parse_quote!(
                struct Bad {
                    #[arg(unknown = "x")]
                    value: i32,
                }
            ),
            "expected name",
        ),
        (
            syn::parse_quote!(
                struct Bad {
                    #[arg(name = "same")]
                    a: i32,
                    #[arg(name = "same")]
                    b: i32,
                }
            ),
            "duplicate",
        ),
    ] {
        assert!(expand_args(&input).unwrap_err().to_string().contains(expected));
    }
    for input in [
        syn::parse_quote!(
            enum Bad {
                Empty,
            }
        ),
        syn::parse_quote!(
            enum Bad {
                Pair(A, B),
            }
        ),
        syn::parse_quote!(
            enum Bad {
                #[command(name = "same")]
                A(A),
                #[command(name = "same")]
                B(B),
            }
        ),
    ] {
        assert!(expand_set(&input).is_err());
    }
}

#[test]
fn invalid_values_reject_ambiguous_aliases_and_valid_expansions_are_rust_items() {
    for input in [
        syn::parse_quote!(
            #[value(into = First, into = Second)]
            enum Bad {
                A,
            }
        ),
        syn::parse_quote!(
            #[value(name = "wrong_scope")]
            enum Bad {
                A,
            }
        ),
        syn::parse_quote!(
            #[value(into = 7)]
            enum Bad {
                A,
            }
        ),
        syn::parse_quote!(
            enum Bad {
                #[value(alias = "b")]
                A,
                B,
            }
        ),
        syn::parse_quote!(
            enum Bad {
                #[value(alias = "0", alias = "0")]
                A,
            }
        ),
        syn::parse_quote!(
            enum Bad {
                #[value(name = "two words")]
                A,
            }
        ),
        syn::parse_quote!(
            enum Bad {
                A(i32),
            }
        ),
        syn::parse_quote!(
            enum Bad {}
        ),
    ] {
        assert!(expand_value(&input).is_err());
    }
    for expansion in [
        expand_args(&syn::parse_quote!(
            struct Args<'p> {
                name: &'p str,
                limit: Option<i32>,
            }
        ))
        .unwrap(),
        expand_args(&syn::parse_quote!(
            struct Empty;
        ))
        .unwrap(),
        expand_args(&syn::parse_quote!(
            enum Forms<'p> {
                Named(Args<'p>),
                Empty(Empty),
            }
        ))
        .unwrap(),
        expand_set(&syn::parse_quote!(
            enum Command<'p> {
                #[command(name = "test")]
                Test(Args<'p>),
            }
        ))
        .unwrap(),
        expand_value(&syn::parse_quote!(
            enum Mode {
                #[value(alias = "0")]
                Survival,
                Creative,
            }
        ))
        .unwrap(),
    ] {
        syn::parse2::<syn::ItemImpl>(expansion).unwrap();
    }
}
