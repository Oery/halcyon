//! Derives generate syntax and metadata; they never generate gameplay execution.
use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as Tokens};
use quote::quote;
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Fields, GenericArgument, GenericParam, Generics, Lifetime,
    LifetimeParam, LitStr, PathArguments, Type, parse_macro_input,
};

#[proc_macro_derive(CommandSet, attributes(command))]
pub fn command_set(input: TokenStream) -> TokenStream {
    expand_set(&parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(CommandArgs, attributes(arg))]
pub fn command_args(input: TokenStream) -> TokenStream {
    expand_args(&parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(CommandValue, attributes(value))]
pub fn command_value(input: TokenStream) -> TokenStream {
    expand_value(&parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn names(
    attrs: &[Attribute],
    namespace: &str,
    default: &str,
    aliases: bool,
) -> syn::Result<Vec<LitStr>> {
    let mut name = None;
    let mut alternatives = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident(namespace)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                if name.is_some() {
                    return Err(meta.error("duplicate name"));
                }
                name = Some(meta.value()?.parse::<LitStr>()?);
            } else if aliases && meta.path.is_ident("alias") {
                alternatives.push(meta.value()?.parse::<LitStr>()?);
            } else {
                return Err(meta.error(if aliases {
                    "expected name or alias"
                } else {
                    "expected name"
                }));
            }
            Ok(())
        })?;
    }
    let mut result = vec![name.unwrap_or_else(|| LitStr::new(default, Span::call_site()))];
    result.extend(alternatives);
    for value in &result {
        let text = value.value();
        if text.is_empty()
            || !text
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+'))
        {
            return Err(syn::Error::new_spanned(
                value,
                "names must be nonempty unquoted ASCII words",
            ));
        }
    }
    Ok(result)
}

fn unique(names: &[LitStr], seen: &mut HashSet<String>) -> syn::Result<()> {
    for name in names {
        if !seen.insert(name.value()) {
            return Err(syn::Error::new_spanned(
                name,
                "duplicate command name, argument name or value alias",
            ));
        }
    }
    Ok(())
}

/// A single borrowing lifetime covers zero-copy arguments without introducing
/// implicit bounds on arbitrary user generics.
fn generics(input: &DeriveInput) -> syn::Result<(Generics, Lifetime)> {
    if input.generics.params.iter().any(|param| !matches!(param, GenericParam::Lifetime(_)))
        || input.generics.lifetimes().count() > 1
    {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "commands support at most one lifetime and no type or const parameters",
        ));
    }
    let mut generics = input.generics.clone();
    let existing = generics.lifetimes().next().map(|param| param.lifetime.clone());
    let lifetime = if let Some(lifetime) = existing {
        lifetime
    } else {
        let lifetime = Lifetime::new("'__command", Span::call_site());
        generics.params.push(GenericParam::Lifetime(LifetimeParam::new(lifetime.clone())));
        lifetime
    };
    Ok((generics, lifetime))
}

fn optional(ty: &Type) -> syn::Result<Option<&Type>> {
    let Type::Path(path) = ty else { return Ok(None) };
    let Some(segment) = path.path.segments.last().filter(|segment| segment.ident == "Option")
    else {
        return Ok(None);
    };
    if let PathArguments::AngleBracketed(arguments) = &segment.arguments
        && arguments.args.len() == 1
        && let GenericArgument::Type(inner) = &arguments.args[0]
    {
        return Ok(Some(inner));
    }
    Err(syn::Error::new_spanned(ty, "expected Option<ArgumentType>"))
}

fn expand_args(input: &DeriveInput) -> syn::Result<Tokens> {
    if matches!(input.data, Data::Enum(_)) {
        return expand_alternatives(input);
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "CommandArgs requires a struct or enum"));
    };
    if matches!(data.fields, Fields::Unnamed(_)) {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "CommandArgs requires named fields or a unit struct",
        ));
    }
    let (generics, lifetime) = generics(input)?;
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let (_, type_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let mut specs = Vec::new();
    let mut parse = Vec::new();
    let mut suggest = Vec::new();
    let mut optional_seen = false;
    let mut seen = HashSet::new();
    for field in &data.fields {
        let ident = field.ident.as_ref().expect("named fields checked");
        let name = names(&field.attrs, "arg", ident.to_string().trim_start_matches("r#"), false)?;
        unique(&name, &mut seen)?;
        let name = &name[0];
        let inner = optional(&field.ty)?;
        let is_optional = inner.is_some();
        if optional_seen && !is_optional {
            return Err(syn::Error::new(
                field.span(),
                "required arguments cannot follow optional arguments",
            ));
        }
        optional_seen |= is_optional;
        let ty = inner.unwrap_or(&field.ty);
        specs.push(quote! {
            ::halcyon_commands::ArgumentSpec {
                name: #name, optional: #is_optional,
                kind: <#ty as ::halcyon_commands::CommandArgument<#lifetime>>::KIND,
            }
        });
        let value = quote! {
            <#ty as ::halcyon_commands::CommandArgument<#lifetime>>::parse_remaining(__word, __words)
                .map_err(|message| ::halcyon_commands::ArgumentError::Invalid { name: #name, message })?
        };
        parse.push(if is_optional {
            quote! {
                #ident: match __words.next() {
                    ::core::option::Option::Some(__word) => ::core::option::Option::Some(#value),
                    ::core::option::Option::None => ::core::option::Option::None,
                }
            }
        } else {
            quote! {
                #ident: {
                    let __word = __words.next().ok_or(::halcyon_commands::ArgumentError::Missing(#name))?;
                    #value
                }
            }
        });
        suggest.push(quote! {
            match __words.next() {
                ::core::option::Option::Some(__word) => {
                    if let ::core::result::Result::Err(__matches) = <#ty as ::halcyon_commands::CommandArgument<#lifetime>>::suggest_remaining(__word, &mut __words, __prefix, __context) {
                        return __matches;
                    }
                }
                ::core::option::Option::None => {
                    return <#ty as ::halcyon_commands::CommandArgument<#lifetime>>::suggest(__prefix, __context);
                }
            }
        });
    }
    let construct = if matches!(data.fields, Fields::Unit) {
        quote!(Self)
    } else {
        quote!(Self { #(#parse),* })
    };
    let mutable = if data.fields.is_empty() { quote!() } else { quote!(mut) };
    Ok(quote! {
        impl #impl_generics ::halcyon_commands::CommandArgs<#lifetime> for #ident #type_generics #where_clause {
            const SYNTAX: ::halcyon_commands::ArgumentSyntax = ::halcyon_commands::ArgumentSyntax::Sequence(&[#(#specs),*]);

            fn parse(__words: &mut ::std::str::SplitWhitespace<#lifetime>) -> ::core::result::Result<Self, ::halcyon_commands::ArgumentError> {
                let __value = #construct;
                if __words.next().is_some() {
                    return ::core::result::Result::Err(::halcyon_commands::ArgumentError::Extra);
                }
                ::core::result::Result::Ok(__value)
            }

            fn suggest(#mutable __words: ::std::str::SplitWhitespace<#lifetime>, __prefix: &str, __context: &dyn ::halcyon_commands::CompletionContext) -> ::std::vec::Vec<::std::string::String> {
                #(#suggest)*
                ::std::vec::Vec::new()
            }
        }
    })
}

fn expand_alternatives(input: &DeriveInput) -> syn::Result<Tokens> {
    let Data::Enum(data) = &input.data else { unreachable!() };
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "CommandArgs requires at least one alternative",
        ));
    }
    let (generics, lifetime) = generics(input)?;
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let (_, type_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let mut syntax = Vec::new();
    let mut parse = Vec::new();
    let mut suggest = Vec::new();
    for variant in &data.variants {
        let Fields::Unnamed(fields) = &variant.fields else {
            return Err(syn::Error::new_spanned(
                variant,
                "each alternative must contain one CommandArgs type",
            ));
        };
        if fields.unnamed.len() != 1 {
            return Err(syn::Error::new_spanned(
                fields,
                "each alternative must contain exactly one CommandArgs type",
            ));
        }
        let ty = &fields.unnamed[0].ty;
        let variant = &variant.ident;
        syntax.push(quote!(<#ty as ::halcyon_commands::CommandArgs<#lifetime>>::SYNTAX));
        parse.push(quote! {
            let mut __trial = __words.clone();
            match <#ty as ::halcyon_commands::CommandArgs<#lifetime>>::parse(&mut __trial) {
                ::core::result::Result::Ok(__value) => {
                    *__words = __trial;
                    return ::core::result::Result::Ok(Self::#variant(__value));
                }
                ::core::result::Result::Err(__failure) => {
                    let __left = __trial.clone().count();
                    if __left < __remaining {
                        __remaining = __left;
                        __error = __failure;
                        __best = __trial;
                    }
                }
            }
        });
        suggest.push(quote! {
            __matches.extend(<#ty as ::halcyon_commands::CommandArgs<#lifetime>>::suggest(__words.clone(), __prefix, __context));
        });
    }
    Ok(quote! {
        impl #impl_generics ::halcyon_commands::CommandArgs<#lifetime> for #ident #type_generics #where_clause {
            const SYNTAX: ::halcyon_commands::ArgumentSyntax = ::halcyon_commands::ArgumentSyntax::Alternatives(&[#(#syntax),*]);

            fn parse(__words: &mut ::std::str::SplitWhitespace<#lifetime>) -> ::core::result::Result<Self, ::halcyon_commands::ArgumentError> {
                let mut __best = __words.clone();
                let mut __remaining = ::core::primitive::usize::MAX;
                let mut __error = ::halcyon_commands::ArgumentError::Extra;
                #(#parse)*
                *__words = __best;
                ::core::result::Result::Err(__error)
            }

            fn suggest(__words: ::std::str::SplitWhitespace<#lifetime>, __prefix: &str, __context: &dyn ::halcyon_commands::CompletionContext) -> ::std::vec::Vec<::std::string::String> {
                let mut __matches = ::std::vec::Vec::new();
                #(#suggest)*
                __matches
            }
        }
    })
}

fn expand_set(input: &DeriveInput) -> syn::Result<Tokens> {
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "CommandSet requires an enum"));
    };
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "a command set must contain at least one command",
        ));
    }
    let (generics, lifetime) = generics(input)?;
    let (impl_generics, _, where_clause) = generics.split_for_impl();
    let (_, type_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let mut specs = Vec::new();
    let mut parse = Vec::new();
    let mut suggest = Vec::new();
    let mut seen = HashSet::new();
    for variant in &data.variants {
        let Fields::Unnamed(fields) = &variant.fields else {
            return Err(syn::Error::new_spanned(
                variant,
                "each command variant must contain one CommandArgs type",
            ));
        };
        if fields.unnamed.len() != 1 {
            return Err(syn::Error::new_spanned(
                fields,
                "each command variant must contain exactly one CommandArgs type",
            ));
        }
        let name = names(
            &variant.attrs,
            "command",
            &variant.ident.to_string().trim_start_matches("r#").to_ascii_lowercase(),
            false,
        )?;
        unique(&name, &mut seen)?;
        let name = &name[0];
        let variant = &variant.ident;
        let ty = &fields.unnamed[0].ty;
        let spec = quote! {
            ::halcyon_commands::CommandSpec {
                name: #name,
                syntax: <#ty as ::halcyon_commands::CommandArgs<#lifetime>>::SYNTAX,
            }
        };
        specs.push(spec.clone());
        parse.push(quote! {
            ::core::option::Option::Some(#name) => {
                <#ty as ::halcyon_commands::CommandArgs<#lifetime>>::parse(&mut __words)
                    .map(Self::#variant)
                    .map_err(|error| ::halcyon_commands::ParseError::Arguments { command: #spec, error })
            }
        });
        suggest.push(quote! {
            #name => <#ty as ::halcyon_commands::CommandArgs<#lifetime>>::suggest(__words, __prefix, __context)
        });
    }
    Ok(quote! {
        impl #impl_generics ::halcyon_commands::CommandSet<#lifetime> for #ident #type_generics #where_clause {
            const COMMANDS: &'static [::halcyon_commands::CommandSpec] = &[#(#specs),*];

            fn parse(__text: &#lifetime str) -> ::core::result::Result<Self, ::halcyon_commands::ParseError> {
                let mut __words = __text.split_whitespace();
                match __words.next() {
                    #(#parse),*,
                    _ => ::core::result::Result::Err(::halcyon_commands::ParseError::UnknownCommand),
                }
            }

            fn suggest(__command: &str, __words: ::std::str::SplitWhitespace<#lifetime>, __prefix: &str, __context: &dyn ::halcyon_commands::CompletionContext) -> ::std::vec::Vec<::std::string::String> {
                match __command {
                    #(#suggest),*,
                    _ => ::std::vec::Vec::new(),
                }
            }
        }
    })
}

fn expand_value(input: &DeriveInput) -> syn::Result<Tokens> {
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "CommandValue requires an enum"));
    };
    if !input.generics.params.is_empty() || data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "CommandValue requires a nonempty enum without generics",
        ));
    }
    let mut target = None;
    for attr in input.attrs.iter().filter(|attr| attr.path().is_ident("value")) {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("into") {
                return Err(meta.error("expected into on the CommandValue enum"));
            }
            if target.is_some() {
                return Err(meta.error("duplicate conversion target"));
            }
            target = Some(meta.value()?.parse::<syn::Path>()?);
            Ok(())
        })?;
    }
    let ident = &input.ident;
    let mut choices = Vec::new();
    let mut parse = Vec::new();
    let mut seen = HashSet::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "CommandValue variants must not contain fields",
            ));
        }
        let names = names(
            &variant.attrs,
            "value",
            &variant.ident.to_string().trim_start_matches("r#").to_ascii_lowercase(),
            true,
        )?;
        unique(&names, &mut seen)?;
        choices.extend(names.clone());
        let variant = &variant.ident;
        parse.push(quote! { #(#names)|* => ::core::result::Result::Ok(Self::#variant) });
    }
    let conversion = target.map(|target| {
        let variants = data.variants.iter().map(|variant| &variant.ident);
        quote! {
            impl ::core::convert::From<#ident> for #target {
                fn from(__value: #ident) -> Self {
                    match __value {
                        #(#ident::#variants => Self::#variants),*
                    }
                }
            }
        }
    });
    let message = LitStr::new(
        &format!(
            "Expected one of: {}.",
            choices.iter().map(LitStr::value).collect::<Vec<_>>().join(", ")
        ),
        ident.span(),
    );
    Ok(quote! {
        impl<'__command> ::halcyon_commands::CommandArgument<'__command> for #ident {
            const KIND: ::halcyon_commands::ArgumentKind = ::halcyon_commands::ArgumentKind::Choices(&[#(#choices),*]);

            fn parse(__value: &'__command str) -> ::core::result::Result<Self, &'static str> {
                match __value {
                    #(#parse),*,
                    _ => ::core::result::Result::Err(#message),
                }
            }
        }
        #conversion
    })
}

#[cfg(test)]
#[path = "tests/tests_derive.rs"]
mod tests;
