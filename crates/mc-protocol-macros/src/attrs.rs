use syn::parse::{Parse, ParseStream};

use crate::Origin;
use crate::State;

pub struct PacketAttributes {
    pub id: i32,
    pub origin: Origin,
    pub state: State,
}

impl Parse for PacketAttributes {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let state_ident = input.parse::<syn::Ident>()?;
        let state = match state_ident.to_string().as_str() {
            "Handshake" => State::Handshake,
            "Status" => State::Status,
            "Login" => State::Login,
            "Config" => State::Config,
            "Play" => State::Play,
            _ => panic!("Invalid State"),
        };

        input.parse::<syn::Token![,]>()?;

        let id_lit = input.parse::<syn::LitInt>()?;
        let id = id_lit.base10_parse::<i32>()?;

        input.parse::<syn::Token![,]>()?;

        let origin_ident = input.parse::<syn::Ident>()?;
        let origin = match origin_ident.to_string().as_str() {
            "Client" => Origin::Client,
            "Server" => Origin::Server,
            _ => panic!("Invalid Origin"),
        };

        Ok(PacketAttributes { id, origin, state })
    }
}
