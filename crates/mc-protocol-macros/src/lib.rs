use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Field, Fields, Type, parse_macro_input};

mod attrs;
mod format;
mod utils;

use crate::attrs::PacketAttributes;
use crate::format::Format;

enum Origin {
    Client,
    Server,
}

enum State {
    Handshake,
    Status,
    Login,
    Config,
    Play,
}

fn get_field_decode_fn(field: &Field) -> proc_macro2::TokenStream {
    let ident = &field.ident;

    let path = match &field.ty {
        Type::Path(type_path) => &type_path.path,
        Type::Reference(r) => {
            let t = &r.elem;
            return quote! { let #ident: &#t = <&#t>::read(buf)?;};
        }
        _ => return quote! {},
    };

    // last segment: Vec<T>, Option<T>, String, etc
    let segment = path.segments.last().unwrap();
    let type_ident = &segment.ident;

    match Format::from(&field.attrs) {
        Format::Standard => quote! { let #ident = #type_ident::read(buf)?; },
        Format::Json => quote! {
            let json: &str = <&str>::read(buf)?;
            let #ident: #type_ident = serde_json::from_str(json)?;
        },
        Format::VarInt => match type_ident.to_string().as_str() {
            "usize" => quote! { let #ident = usize::try_from(VarInt::read(buf)?.0)?; },
            "i32" => quote! { let #ident = VarInt::read(buf)?.0; },
            _ => quote! { let #ident = VarInt::read(buf)?.0.try_into()?; },
        },
    }
}

fn impl_decode_payload(data: &DataStruct) -> proc_macro2::TokenStream {
    let fields = match &data.fields {
        Fields::Named(fields) => fields,
        Fields::Unit => return quote! { Ok(Self) },
        _ => panic!("decode_payload: Fields should be named"),
    };

    let fields_names = fields.named.iter().map(|f| f.ident.as_ref().unwrap());
    let fields_decode = fields.named.iter().map(get_field_decode_fn);

    quote! {
        #( #fields_decode )*
        Ok(Self { #( #fields_names ),* })
    }
}

fn get_field_write_fn(field: &Field) -> proc_macro2::TokenStream {
    let ident = &field.ident;

    match Format::from(&field.attrs) {
        Format::Standard => quote! { self.#ident.write(w).await?; },
        Format::VarInt => quote! { VarInt(self.#ident as i32).write(w).await?; },
        Format::Json => quote! {
            let json = serde_json::to_string(&self.#ident).unwrap();
            json.write(w).await?;
        },
    }
}

fn impl_write(data: &DataStruct) -> proc_macro2::TokenStream {
    let fields = match &data.fields {
        Fields::Named(fields) => fields,
        Fields::Unit => return quote! {},
        _ => panic!("write: Fields should be named"),
    };

    let fields_write = fields.named.iter().map(get_field_write_fn);

    quote! { #( #fields_write )* }
}

fn impl_payload_len(data: &DataStruct) -> proc_macro2::TokenStream {
    let fields = match &data.fields {
        Fields::Named(fields) => fields,
        Fields::Unit => return quote! {},
        _ => panic!("payload_len: Fields should be named"),
    };

    let field_lengths = fields.named.iter().map(|field| {
        let ident = &field.ident;

        let ident = match Format::from(&field.attrs) {
            Format::Standard => quote! { self.#ident },
            Format::VarInt => quote! { VarInt(self.#ident as i32) },
            Format::Json => quote! {
                serde_json::to_string(&self.#ident).unwrap().as_str()
            },
        };

        quote! { length += #ident.body_len(); }
    });

    quote! { #( #field_lengths )* }
}

// FIXME: state field
#[proc_macro_attribute]
pub fn packet(attr: TokenStream, input: TokenStream) -> TokenStream {
    let PacketAttributes { id, origin: _, state } = parse_macro_input!(attr as PacketAttributes);
    let item = parse_macro_input!(input as DeriveInput);
    let name = &item.ident;

    let state = match state {
        State::Handshake => quote!(Handshake),
        State::Status => quote!(Status),
        State::Login => quote!(Login),
        State::Config => quote!(Config),
        State::Play => quote!(Play),
    };

    let (_, ty_generics, _) = &item.generics.split_for_impl();

    let Data::Struct(data) = &item.data else {
        unreachable!();
    };

    let payload_len = impl_payload_len(data);
    let decode_payload = impl_decode_payload(data);
    let write = impl_write(data);

    TokenStream::from(quote! {
        #[derive(Payload, Debug)]
        #item

        impl<'p> Payload<'p> for #name #ty_generics {
            const ID: VarInt = VarInt(#id);
            const STATE: State = State::#state;

            fn payload_len(&self) -> usize {
                let mut length = 0;
                #payload_len
                length
            }

            fn decode_payload(buf: &mut &'p [u8]) -> DecodeResult<Self> {
                #decode_payload
            }

            async fn write_payload<W>(&self, w: &mut W) -> EncodeResult
            where
                W: AsyncWrite + Unpin,
            {
                #write

                Ok(())
            }
        }
    })
}

/// Dummy macro for the attribute helper to resolve
#[proc_macro_derive(Payload, attributes(format))]
pub fn derive_payload(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_derive(InnerPacket)]
pub fn derive_inner_packet(input: TokenStream) -> TokenStream {
    let item = syn::parse_macro_input!(input as syn::ItemEnum);
    let ident = &item.ident;

    let variants: Vec<(&proc_macro2::Ident, &syn::Type)> = item
        .variants
        .iter()
        .map(|v| {
            let Fields::Unnamed(fields) = &v.fields else {
                panic!("field should be unnamed");
            };
            let field = fields.unnamed.first().unwrap();
            (&v.ident, &field.ty)
        })
        .collect();

    let id_cases: Vec<proc_macro2::TokenStream> = variants
        .iter()
        .map(|(v, t)| quote! { Packets::#v(_) => <#t as Payload<'p>>::ID.0 })
        .collect();

    let state_cases: Vec<proc_macro2::TokenStream> = variants
        .iter()
        .map(|(v, t)| quote! { Packets::#v(_) => <#t as Payload<'p>>::STATE })
        .collect();

    TokenStream::from(quote! {
        impl<'p> InnerPacket for #ident<'p> {
            fn id(&self) -> i32 {
                match self {
                    #( #id_cases ),*
                }
            }

            fn state(&self) -> State {
                match self {
                    #( #state_cases ),*
                }
            }
        }
    })
}
